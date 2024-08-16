use ssh2::Session;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
mod config;

fn main() -> io::Result<()> {
    let config = config::read_config("config.toml").expect("Failed to read config");
    let log = config.logs.get(0).unwrap();
    // 连接到远程服务器
    let tcp = TcpStream::connect(format!("{}:{}", log.host, log.port)).unwrap();
    let mut sess = Session::new().unwrap();
    sess.set_tcp_stream(tcp);
    sess.handshake().unwrap();

    // 使用密码认证
    sess.userauth_password(log.username.clone().unwrap().as_ref(), log.password.clone().unwrap().as_ref()).unwrap();

    // 执行远程命令
    let mut channel = sess.channel_session().unwrap();
    channel.exec(format!("tail {} -n 100 -f", log.log_path).as_ref()).unwrap();

    // 创建一个缓冲读取器
    let mut reader = BufReader::new(channel);

    // 持续读取输出
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                print!("{}", line); // 打印每一行
                io::stdout().flush()?; // 确保立即显示输出
            }
            Err(e) => {
                eprintln!("读取错误: {}", e);
                break;
            }
        }
    }

    Ok(())
}