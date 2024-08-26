use crate::io::Stdout;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers,
    },
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
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph},
    Terminal,
};
use unicode_segmentation::UnicodeSegmentation;
use std::path::Path;

mod config;

enum MoveDirection {
    Left,
    Right,
    Up,
    Down,
}

enum ConnectionStatus {
    Connected,
    Error(String),
}

struct LogWindow {
    name: String,
    content: Arc<Mutex<Vec<String>>>,
    formatter: Arc<LogFormatter>,
    scroll_position: Arc<Mutex<usize>>,
    connection_status: Arc<Mutex<ConnectionStatus>>,
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

    let is_scrolling = Arc::new(Mutex::new(false));

    for log in config.logs {
        let content = Arc::new(Mutex::new(Vec::new()));
        let formatter = Arc::new(create_log_formatter());
        let max_history = log.max_history.unwrap_or(10000); 
        let scroll_position = Arc::new(Mutex::new(0));
        let connection_status = Arc::new(Mutex::new(ConnectionStatus::Connected));

        let log_window = LogWindow {
            name: log.name.clone(),
            content: Arc::clone(&content),
            formatter: Arc::clone(&formatter),
            scroll_position: Arc::clone(&scroll_position),
            connection_status: Arc::clone(&connection_status),
        };

        log_windows.push(log_window);

        let content_clone = Arc::clone(&content);
        let scroll_position_clone = Arc::clone(&scroll_position);
        let is_scrolling_clone = Arc::clone(&is_scrolling);
        let connection_status_clone = Arc::clone(&connection_status);
        thread::spawn(move || {
            connect_and_tail(
                &log,
                content_clone,
                max_history,
                scroll_position_clone,
                is_scrolling_clone,
                connection_status_clone,
            )
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

    fn add_rule(&mut self, pattern: &str, style: Style) -> Result<(), regex::Error> {
        let regex = Regex::new(pattern)?;
        self.rules.push(MatchRule { regex, style });
        Ok(())
    }

    fn format_line(&self, line: &str) -> Spans {
        let mut spans = Vec::new();
        let mut last_match_end = 0;

        let mut matches: Vec<(usize, usize, &Style)> = Vec::new();

        for rule in &self.rules {
            for cap in rule.regex.find_iter(line) {
                matches.push((cap.start(), cap.end(), &rule.style));
            }
        }

        matches.sort_by_key(|&(start, _, _)| start);

        for (start, end, style) in matches {
            if start > last_match_end {
                spans.push(Span::raw(line[last_match_end..start].to_string()));
            }
            spans.push(Span::styled(line[start..end].to_string(), *style));
            last_match_end = end;
        }

        if last_match_end < line.len() {
            spans.push(Span::raw(line[last_match_end..].to_string()));
        }

        Spans::from(spans)
    }
}

fn create_log_formatter() -> LogFormatter {
    let mut formatter = LogFormatter::new();

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
        .add_rule(r"INFO", Style::default().fg(Color::Blue))
        .unwrap();

    formatter
}

fn connect_and_tail(
    log: &config::LogConfig,
    content: Arc<Mutex<Vec<String>>>,
    max_history: usize,
    scroll_position: Arc<Mutex<usize>>,
    is_scrolling: Arc<Mutex<bool>>,
    connection_status: Arc<Mutex<ConnectionStatus>>,
) -> io::Result<()> {
    let tcp = match TcpStream::connect(format!("{}:{}", log.host, log.port)) {
        Ok(stream) => stream,
        Err(e) => {
            *connection_status.lock().unwrap() = ConnectionStatus::Error(format!("Connect Err: {}", e));
            return Err(e);
        }
    };
    let mut sess = Session::new().unwrap();

    sess.set_tcp_stream(tcp);

    if let Err(e) = sess.handshake() {
        *connection_status.lock().unwrap() = ConnectionStatus::Error(format!("Connect Err: {}", e));
        return Err(io::Error::new(io::ErrorKind::Other, e));
    }

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
    channel.exec(&format!("tail {} -n 5 -f", log.log_path))?;

    let mut reader = BufReader::new(channel);

    *connection_status.lock().unwrap() = ConnectionStatus::Connected;

    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let mut content = content.lock().unwrap();
                content.push(line.trim().to_string());
                
                while content.len() > max_history {
                    content.remove(0);
                }

                let mut scroll_pos = scroll_position.lock().unwrap();
                if !*is_scrolling.lock().unwrap() {
                    *scroll_pos = content.len().saturating_sub(1);
                } else {
                    *scroll_pos = scroll_pos.saturating_sub(1);
                }
            }
            Err(e) => {
                *connection_status.lock().unwrap() = ConnectionStatus::Error(format!("Read Err: ({}): {}", log.host, e));
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
        let window_height = terminal.size()?.height as usize;

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
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Enter | KeyCode::Char('m') => {
                        app_state.is_maximized = !app_state.is_maximized;
                        app_state.is_scrolling = !app_state.is_scrolling;
                        let window = &mut app_state.log_windows[app_state.selected_window];
                        let content_len = window.content.lock().unwrap().len();
                        let mut scroll_position = window.scroll_position.lock().unwrap();
                        *scroll_position = content_len.saturating_sub(1);
                    }
                    // KeyCode::Char('s') => {
                    //     // 保存到本地文件逻辑
                    //     todo!()
                    // }
                    // KeyCode::Char('h') => {
                    //     // Help
                    //     todo!()
                    // }
                    // KeyCode::Char('c') => {
                    //     // Copy
                    //     todo!()
                    // }
                    KeyCode::Down => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Down, window_height);
                        } else {
                            move_selection(app_state, MoveDirection::Down);
                        }
                    }
                    KeyCode::Up => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Up, window_height);
                        } else {
                            move_selection(app_state, MoveDirection::Up);
                        }
                    }
                    KeyCode::Left => {
                        if app_state.is_scrolling {
                        } else {
                            move_selection(app_state, MoveDirection::Left);
                        }
                    }
                    KeyCode::Right => {
                        if app_state.is_scrolling {
                        } else {
                            move_selection(app_state, MoveDirection::Right);
                        }
                    }
                    KeyCode::PageDown => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::PageDown, window_height);
                        }
                    }
                    KeyCode::PageUp => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::PageUp, window_height);
                        }
                    }
                    KeyCode::Home => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Top, window_height);
                        }
                    }
                    KeyCode::End => {
                        if app_state.is_scrolling {
                            scroll_log(app_state, ScrollDirection::Bottom, window_height);
                        }
                    }
                    KeyCode::Char('r') => {
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
    window.scroll_position = Arc::new(Mutex::new(0));
}

fn scroll_log(app_state: &mut AppState, direction: ScrollDirection, window_height: usize) {
    if !app_state.is_scrolling {
        return;
    }

    let window = &mut app_state.log_windows[app_state.selected_window];
    let content_len = window.content.lock().unwrap().len();
    let mut scroll_position = window.scroll_position.lock().unwrap();

    // 计算每页的行数，减去2是为了考虑边框
    let page_size = window_height.saturating_sub(2);

    match direction {
        ScrollDirection::Up => {
            if *scroll_position > 0 {
                *scroll_position -= 1;
            }
        }
        ScrollDirection::Down => {
            if *scroll_position < content_len.saturating_sub(1) {
                *scroll_position += 1;
            }
        }
        ScrollDirection::PageUp => {
            *scroll_position = scroll_position.saturating_sub(page_size);
        }
        ScrollDirection::PageDown => {
            *scroll_position = (*scroll_position + page_size).min(content_len.saturating_sub(1));
        }
        ScrollDirection::Top => {
            *scroll_position = 0;
        }
        ScrollDirection::Bottom => {
            *scroll_position = content_len.saturating_sub(1);
        }
    }
}

fn wrap_line(line: &str, max_width: usize) -> Vec<String> {
    let mut wrapped = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for grapheme in line.graphemes(true) {
        let grapheme_width = unicode_width::UnicodeWidthStr::width(grapheme);
        
        if current_width + grapheme_width > max_width {
            if !current_line.is_empty() {
                wrapped.push(current_line);
                current_line = String::new();
                current_width = 0;
            }
            if grapheme_width > max_width {
                wrapped.push(grapheme.to_string());
            } else {
                current_line.push_str(grapheme);
                current_width = grapheme_width;
            }
        } else {
            current_line.push_str(grapheme);
            current_width += grapheme_width;
        }
    }

    if !current_line.is_empty() {
        wrapped.push(current_line);
    }

    wrapped
}

fn render_maximized_window(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let selected_window = &app_state.log_windows[app_state.selected_window];
    let content = selected_window.content.lock().unwrap();
    let scroll_position = *selected_window.scroll_position.lock().unwrap();
    let connection_status = selected_window.connection_status.lock().unwrap();

    let block = Block::default()
        .title(format!(
            "{} (Scroll: {})",
            selected_window.name, scroll_position
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner_width = f.size().width as usize - 2;
    let height = f.size().height as usize - 2;

    let mut wrapped_content: Vec<Spans> = Vec::new();
    let mut total_lines = 0;

    for line in content.iter() {
        let wrapped = wrap_line(line, inner_width);
        for wrapped_line in wrapped {
            wrapped_content.push(selected_window.formatter.format_line(&wrapped_line));
            total_lines += 1;
            if total_lines >= scroll_position + height {
                break;
            }
        }
        if total_lines >= scroll_position + height {
            break;
        }
    }

    let start = if app_state.is_scrolling {
        total_lines.saturating_sub(height).min(scroll_position)
    } else {
        total_lines.saturating_sub(height)
    };

    let mut text: Vec<Spans> = wrapped_content
        .into_iter()
        .skip(start)
        .take(height)
        .collect();

    if let ConnectionStatus::Error(err_msg) = &*connection_status {
        if text.len() < height {
            text.push(Spans::from(Span::styled(
                err_msg,
                Style::default().fg(Color::Red),
            )));
        } else {
            text[height - 1] = Spans::from(Span::styled(
                err_msg,
                Style::default().fg(Color::Red),
            ));
        }
    }

    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black));

    f.render_widget(paragraph, f.size());
}

fn render_normal_layout(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let chunks = create_layout(f.size(), app_state.log_windows.len());

    for (i, log_window) in app_state.log_windows.iter().enumerate() {
        let content = log_window.content.lock().unwrap();
        let scroll_position = *log_window.scroll_position.lock().unwrap();
        let connection_status = log_window.connection_status.lock().unwrap();

        let block = Block::default()
            .title(format!("{} (Scroll: {})", log_window.name, scroll_position))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if i == app_state.selected_window {
                Color::Yellow
            } else {
                Color::White
            }));

        let inner_width = chunks[i].width as usize - 2;
        let height = chunks[i].height as usize - 2;

        let mut wrapped_content: Vec<Spans> = Vec::new();
        let mut total_lines = 0;

        for line in content.iter() {
            let wrapped = wrap_line(line, inner_width);
            for wrapped_line in wrapped {
                wrapped_content.push(log_window.formatter.format_line(&wrapped_line));
                total_lines += 1;
                if total_lines >= scroll_position + height {
                    break;
                }
            }
            if total_lines >= scroll_position + height {
                break;
            }
        }

        let start = if app_state.is_scrolling {
            total_lines.saturating_sub(height).min(scroll_position)
        } else {
            total_lines.saturating_sub(height)
        };

        let mut text: Vec<Spans> = wrapped_content
            .into_iter()
            .skip(start)
            .take(height)
            .collect();

        if let ConnectionStatus::Error(err_msg) = &*connection_status {
            if text.len() < height {
                text.push(Spans::from(Span::styled(
                    err_msg,
                    Style::default().fg(Color::Red),
                )));
            } else {
                text[height - 1] = Spans::from(Span::styled(
                    err_msg,
                    Style::default().fg(Color::Red),
                ));
            }
        }

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
        .direction(LayoutDirection::Vertical) 
        .constraints(constraints)
        .split(area);

    let mut all_chunks = vec![];
    for row in row_layouts.iter().take(rows) {
        let mut col_constraints = vec![];
        for _ in 0..cols {
            col_constraints.push(Constraint::Percentage((100 / cols) as u16));
        }
        let row_chunks = Layout::default()
            .direction(LayoutDirection::Horizontal) 
            .constraints(col_constraints)
            .split(*row);
        all_chunks.extend(row_chunks);
    }

    all_chunks.truncate(window_count);
    all_chunks
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
