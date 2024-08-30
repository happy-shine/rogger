mod config;
mod ssh;
mod ui;

use std::io;
use std::sync::{Arc, Mutex};
use std::thread;

use ssh::{connect_and_tail, ConnectionStatus};
use ui::{create_log_formatter, AppState, LogWindow, run_ui};

fn log_window(log_config: config::LogConfig) -> LogWindow {
    let content = Arc::new(Mutex::new(Vec::new()));
    let formatter = Arc::new(create_log_formatter());
    let max_history = log_config.max_history.unwrap_or(10000);
    let scroll_position = Arc::new(Mutex::new(0));
    let connection_status = Arc::new(Mutex::new(ConnectionStatus::Connected));

    let log_window = LogWindow {
        name: log_config.name.clone(),
        content: Arc::clone(&content),
        formatter: Arc::clone(&formatter),
        scroll_position: Arc::clone(&scroll_position),
        connection_status: Arc::clone(&connection_status),
    };

    let is_maximized = Arc::new(Mutex::new(false));
    thread::spawn(move || {
        connect_and_tail(
            &log_config,
            content,
            max_history,
            scroll_position,
            is_maximized,
            connection_status,
        )
    });

    log_window
}

fn main() -> io::Result<()> {
    // TODO: File Err Handle
    // TODO: Input File Path
    let config = config::read_config("~/.rogger/config.toml").expect("File Not Found Err: ~/.rogger/config.toml");
    
    let log_windows: Vec<LogWindow> = config.logs
        .into_iter()
        .map(log_window)
        .collect();

    let mut app_state = AppState {
        log_windows,
        selected_window: 0,
        is_maximized: false,
        has_scrolled: false,
    };

    run_ui(&mut app_state)
}