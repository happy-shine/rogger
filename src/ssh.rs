use ssh2::Session;
use std::io::{self, BufRead, BufReader};
use std::net::TcpStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config;

pub enum ConnectionStatus {
    Connected,
    Error(String),
}

pub fn connect_and_tail(
    log: &config::LogConfig,
    content: Arc<Mutex<Vec<String>>>,
    max_history: usize,
    scroll_position: Arc<Mutex<usize>>,
    is_maximized: Arc<Mutex<bool>>,
    connection_status: Arc<Mutex<ConnectionStatus>>,
) -> io::Result<()> {
    let tcp = TcpStream::connect(format!("{}:{}", log.host, log.port)).map_err(|e| {
        let _ = update_connection_status(
            &connection_status,
            ConnectionStatus::Error(format!("Connect Err: {}", e)),
        );
        e
    })?;

    tcp.set_read_timeout(Some(Duration::from_secs(30)))?;

    let mut sess = Session::new().map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    sess.set_tcp_stream(tcp);

    sess.handshake().map_err(|e| {
        let _ = update_connection_status(
            &connection_status,
            ConnectionStatus::Error(format!("Handshake Err: {}", e)),
        );
        io::Error::new(io::ErrorKind::Other, e)
    })?;

    authenticate(&sess, log)?;

    let mut channel = sess.channel_session()?;
    channel.exec(&format!("tail {} -n 100 -f", log.log_path))?;

    let mut reader = BufReader::new(channel);

    let _ = update_connection_status(&connection_status, ConnectionStatus::Connected);

    {
        let mut scroll_pos = scroll_position.lock().unwrap();
        let content = content.lock().unwrap();
        *scroll_pos = content.len().saturating_sub(1);
    }

    process_log_stream(
        &mut reader,
        content,
        max_history,
        scroll_position,
        is_maximized,
        connection_status,
        &log.host,
    )
}

fn authenticate(sess: &Session, log: &config::LogConfig) -> io::Result<()> {
    let username = log.username.as_deref().unwrap_or("");
    let result = if let Some(password) = &log.password {
        sess.userauth_password(username, password)
    } else if let Some(ssh_key) = &log.ssh_key {
        let key_path = Path::new(ssh_key);
        sess.userauth_pubkey_file(username, None, key_path, None)
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "No authentication method provided",
        ));
    };

    result.map_err(|e| io::Error::new(io::ErrorKind::Other, e))
}

fn process_log_stream(
    reader: &mut BufReader<ssh2::Channel>,
    content: Arc<Mutex<Vec<String>>>,
    max_history: usize,
    scroll_position: Arc<Mutex<usize>>,
    is_maximized: Arc<Mutex<bool>>,
    connection_status: Arc<Mutex<ConnectionStatus>>,
    host: &str,
) -> io::Result<()> {
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => update_content(&content, max_history, &scroll_position, &is_maximized, line),
            Err(e) => {
                let _ = update_connection_status(
                    &connection_status,
                    ConnectionStatus::Error(format!("Read Err ({}): {}", host, e)),
                );
                break;
            }
        }
    }
    Ok(())
}

fn update_content(
    content: &Arc<Mutex<Vec<String>>>,
    max_history: usize,
    scroll_position: &Arc<Mutex<usize>>,
    is_maximized: &Arc<Mutex<bool>>,
    line: String,
) {
    let mut content = content.lock().unwrap();
    content.push(line);

    while content.len() > max_history {
        content.remove(0);
    }

    let mut scroll_pos = scroll_position.lock().unwrap();
    let is_max = *is_maximized.lock().unwrap();

    if !is_max {
        *scroll_pos = content.len().saturating_sub(1);
    } else {
        if *scroll_pos == content.len().saturating_sub(2) {
            *scroll_pos = content.len().saturating_sub(1);
        }
    }
}

fn update_connection_status(
    connection_status: &Mutex<ConnectionStatus>,
    status: ConnectionStatus,
) -> io::Result<()> {
    let mut status_lock = connection_status
        .lock()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "Failed to lock connection status"))?;
    *status_lock = status;
    Ok(())
}
