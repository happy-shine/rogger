use crate::io::Stdout;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use regex::Regex;
use ssh2::Session;
use std::io::{self, BufRead, BufReader};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tui::layout::Direction as LayoutDirection;
use tui::Frame;
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
mod config;
use std::path::Path;

struct LogWindow {
    name: String,
    content: Arc<Mutex<Vec<String>>>,
    formatter: Arc<LogFormatter>,
    scroll_position: usize,
    max_history: usize,
}

struct AppState {
    log_windows: Vec<LogWindow>,
    selected_window: usize,
    is_maximized: bool,
    is_scrolling: bool,
}

struct MatchRule {
    regex: Regex,
    style: Style,
}

struct LogFormatter {
    rules: Vec<MatchRule>,
}

enum ScrollDirection {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

fn main() -> io::Result<()> {
    let config = config::read_config("~/.rogger/config.toml").expect("Failed to read config");
    let mut log_windows = Vec::new();

    for log in config.logs {
        let content = Arc::new(Mutex::new(Vec::new()));
        let formatter = Arc::new(create_log_formatter());
        let max_history = 10000; // 或者从配置文件中读取

        let log_window = LogWindow {
            name: log.name.clone(),
            content: Arc::clone(&content),
            formatter: Arc::clone(&formatter),
            scroll_position: 0,
            max_history,
        };

        log_windows.push(log_window);

        let content_clone = Arc::clone(&content);
        thread::spawn(move || {
            if let Err(e) = connect_and_tail(&log, content_clone, max_history) {
                eprintln!("Error in connection to {}: {}", log.host, e);
            }
        });
    }

    let mut app_state = AppState {
        log_windows,
        selected_window: 0,
        is_maximized: false,
        is_scrolling: false,
    };

    run_ui(&mut app_state)?;

    Ok(())
}

impl LogFormatter {
    fn new() -> Self {
        LogFormatter { rules: Vec::new() }
    }

    // 添加新的匹配规则
    fn add_rule(&mut self, pattern: &str, style: Style) -> Result<(), regex::Error> {
        let regex = Regex::new(pattern)?;
        self.rules.push(MatchRule { regex, style });
        Ok(())
    }

    // 格式化单行日志
    fn format_line(&self, line: &str) -> Spans {
        let mut spans = Vec::new();
        let mut last_match_end = 0;

        // 存储所有匹配结果
        let mut matches: Vec<(usize, usize, &Style)> = Vec::new();

        // 找出所有规则的所有匹配
        for rule in &self.rules {
            for cap in rule.regex.find_iter(line) {
                matches.push((cap.start(), cap.end(), &rule.style));
            }
        }

        // 按开始位置排序匹配结果
        matches.sort_by_key(|&(start, _, _)| start);

        // 处理所有匹配
        for (start, end, style) in matches {
            if start > last_match_end {
                spans.push(Span::raw(line[last_match_end..start].to_string()));
            }
            spans.push(Span::styled(line[start..end].to_string(), *style));
            last_match_end = end;
        }

        // 添加最后一个未匹配的部分
        if last_match_end < line.len() {
            spans.push(Span::raw(line[last_match_end..].to_string()));
        }

        Spans::from(spans)
    }
}

fn create_log_formatter() -> LogFormatter {
    let mut formatter = LogFormatter::new();

    // 添加各种规则
    formatter
        .add_rule(
            r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d{3})?(?:\,\d{3})?",
            Style::default().fg(Color::Green),
        )
        .unwrap();
    formatter
        .add_rule(r"WARNING|WARN", Style::default().fg(Color::Yellow))
        .unwrap();
    formatter
        .add_rule(r"ERROR|FATAL", Style::default().fg(Color::Red))
        .unwrap();
    formatter
        .add_rule(r"\{.*?\}", Style::default().fg(Color::Cyan))
        .unwrap();

    formatter
}

fn connect_and_tail(log: &config::LogConfig, content: Arc<Mutex<Vec<String>>>, max_history: usize) -> io::Result<()> {
    let tcp = TcpStream::connect(format!("{}:{}", log.host, log.port))?;
    let mut sess = Session::new().unwrap();

    sess.set_tcp_stream(tcp);
    sess.handshake().unwrap();

    if let Some(password) = &log.password {
        sess.userauth_password(log.username.as_deref().unwrap_or(""), password)?;
    } else if let Some(ssh_key) = &log.ssh_key {
        let username = log.username.as_deref().unwrap_or("");
        let key_path = Path::new(ssh_key);
        sess.userauth_pubkey_file(username, None, key_path, None)?;
    } else {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "No authentication method provided",
        ));
    }

    let mut channel = sess.channel_session()?;
    channel.exec(&format!("tail {} -n 100 -f", log.log_path))?;

    let mut reader = BufReader::new(channel);

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let mut content = content.lock().unwrap();
                content.push(line.trim().to_string());
                if content.len() > max_history {
                    content.remove(0);
                }
            }
            Err(e) => {
                eprintln!("读取错误 ({}): {}", log.host, e);
                break;
            }
        }
    }

    Ok(())
}

fn run_ui(app_state: &mut AppState) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|f| {
            if app_state.is_maximized {
                render_maximized_window(f, app_state);
            } else {
                render_normal_layout(f, app_state);
            }
        })?;

        if event::poll(Duration::from_millis(5))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Enter => {
                        app_state.is_maximized = !app_state.is_maximized;
                    }
                    KeyCode::Char('s') => {
                        app_state.is_scrolling = !app_state.is_scrolling;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Down);
                        } else {
                            move_selection(app_state, MoveDirection::Down);
                        }
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Up);
                        } else {
                            move_selection(app_state, MoveDirection::Up);
                        }
                    }
                    KeyCode::PageDown => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::PageDown);
                        }
                    }
                    KeyCode::PageUp => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::PageUp);
                        }
                    }
                    KeyCode::Home => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Top);
                        }
                    }
                    KeyCode::End => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Bottom);
                        }
                    }
                    KeyCode::Char('c') => {
                        clear_history(app_state);
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn clear_history(app_state: &mut AppState) {
    let window = &mut app_state.log_windows[app_state.selected_window];
    let mut content = window.content.lock().unwrap();
    content.clear();
    window.scroll_position = 0;
}

fn scroll_log(app_state: &mut AppState, direction: ScrollDirection) {
    let window = &mut app_state.log_windows[app_state.selected_window];
    let content_len = window.content.lock().unwrap().len();
    
    match direction {
        ScrollDirection::Up => {
            if window.scroll_position > 0 {
                window.scroll_position -= 1;
            }
        }
        ScrollDirection::Down => {
            if window.scroll_position < content_len.saturating_sub(1) {
                window.scroll_position += 1;
            }
        }
        ScrollDirection::PageUp => {
            window.scroll_position = window.scroll_position.saturating_sub(10);
        }
        ScrollDirection::PageDown => {
            window.scroll_position = (window.scroll_position + 10).min(content_len.saturating_sub(1));
        }
        ScrollDirection::Top => {
            window.scroll_position = 0;
        }
        ScrollDirection::Bottom => {
            window.scroll_position = content_len.saturating_sub(1);
        }
    }
}

fn render_maximized_window(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let selected_window = &app_state.log_windows[app_state.selected_window];
    let content = selected_window.content.lock().unwrap();

    let block = Block::default()
        .title(format!("{} (Scroll: {})", selected_window.name, selected_window.scroll_position))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let text: Vec<Spans> = content
        .iter()
        .skip(selected_window.scroll_position)
        .map(|line| selected_window.formatter.format_line(line))
        .collect();

    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black));

    f.render_widget(paragraph, f.size());
}

fn render_normal_layout(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let chunks = create_layout(f.size(), app_state.log_windows.len());

    for (i, log_window) in app_state.log_windows.iter().enumerate() {
        let content = log_window.content.lock().unwrap();
        let block = Block::default()
            .title(format!("{} (Scroll: {})", log_window.name, log_window.scroll_position))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if i == app_state.selected_window {
                Color::Yellow
            } else {
                Color::White
            }));

        let text: Vec<Spans> = content
            .iter()
            .skip(log_window.scroll_position)
            .map(|line| log_window.formatter.format_line(line))
            .collect();

        let paragraph = Paragraph::new(text)
            .block(block)
            .style(Style::default().fg(Color::White).bg(Color::Black));
        f.render_widget(paragraph, chunks[i]);
    }
}

fn create_layout(area: Rect, window_count: usize) -> Vec<Rect> {
    let mut constraints = vec![];
    let rows = (window_count as f32).sqrt().ceil() as usize;
    let cols = (window_count as f32 / rows as f32).ceil() as usize;

    for _ in 0..rows {
        constraints.push(Constraint::Percentage((100 / rows) as u16));
    }

    let row_layouts = Layout::default()
        .direction(LayoutDirection::Vertical) // 使用 LayoutDirection
        .constraints(constraints)
        .split(area);

    let mut all_chunks = vec![];
    for row in row_layouts.iter().take(rows) {
        let mut col_constraints = vec![];
        for _ in 0..cols {
            col_constraints.push(Constraint::Percentage((100 / cols) as u16));
        }
        let row_chunks = Layout::default()
            .direction(LayoutDirection::Horizontal) // 使用 LayoutDirection
            .constraints(col_constraints)
            .split(*row);
        all_chunks.extend(row_chunks);
    }

    all_chunks.truncate(window_count);
    all_chunks
}

enum MoveDirection {
    Left,
    Right,
    Up,
    Down,
}

fn move_selection(app_state: &mut AppState, direction: MoveDirection) {
    let window_count = app_state.log_windows.len();
    let rows = (window_count as f32).sqrt().ceil() as usize;
    let cols = (window_count as f32 / rows as f32).ceil() as usize;

    let current_row = app_state.selected_window / cols;
    let current_col = app_state.selected_window % cols;

    let (new_row, new_col) = match direction {
        MoveDirection::Left => (
            current_row,
            if current_col > 0 {
                current_col - 1
            } else {
                cols - 1
            },
        ),
        MoveDirection::Right => (current_row, (current_col + 1) % cols),
        MoveDirection::Up => (
            if current_row > 0 {
                current_row - 1
            } else {
                rows - 1
            },
            current_col,
        ),
        MoveDirection::Down => ((current_row + 1) % rows, current_col),
    };

    let new_index = new_row * cols + new_col;
    if new_index < window_count {
        app_state.selected_window = new_index;
    }
}
