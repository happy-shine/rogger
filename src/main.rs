use ssh2::Session;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread;
mod config;
use std::path::Path;

fn main() -> io::Result<()> {
    let config = config::read_config("~/.rogger/config.toml").expect("Failed to read config");
    
    let mut handles = vec![];

    for log in config.logs {
        let handle = thread::spawn(move || {
            if let Err(e) = connect_and_tail(&log) {
                eprintln!("Error in connection to {}: {}", log.host, e);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    Ok(())
}

fn connect_and_tail(log: &config::LogConfig) -> io::Result<()> {
    // 连接到远程服务器
    let tcp = TcpStream::connect(format!("{}:{}", log.host, log.port))?;
    let mut sess = Session::new().unwrap();
    
    sess.set_tcp_stream(tcp);
    sess.handshake().unwrap();

    // 根据配置选择认证方式
    if let Some(password) = &log.password {
        sess.userauth_password(
            log.username.as_deref().unwrap_or(""), 
            password
        )?;
    } else if let Some(ssh_key) = &log.ssh_key {
        let username = log.username.as_deref().unwrap_or("");
        let key_path = Path::new(ssh_key);
        sess.userauth_pubkey_file(
            username,
            None,
            key_path,
            None
        )?;
    } else {
        return Err(io::Error::new(io::ErrorKind::Other, "No authentication method provided"));
    }

    // 执行远程命令
    let mut channel = sess.channel_session()?;
    channel.exec(&format!("tail {} -n 100 -f", log.log_path))?;

    // 创建一个缓冲读取器
    let mut reader = BufReader::new(channel);

    // 持续读取输出
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                print!("[{}] {}", log.name, line); // 打印每一行，包含主机信息
                io::stdout().flush()?; // 确保立即显示输出
            }
            Err(e) => {
                eprintln!("读取错误 ({}): {}", log.host, e);
                break;
            }
        }
    }

    Ok(())
}