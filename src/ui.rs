use crate::{io::Stdout, ssh::ConnectionStatus};
use regex::Regex;
use tui::layout::Direction as LayoutDirection;
use unicode_segmentation::UnicodeSegmentation;

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Span, Spans},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use once_cell::sync::Lazy;
use std::sync::Once;

static INIT: Lazy<Once> = Lazy::new(|| Once::new());

pub struct AppState {
    pub log_windows: Vec<LogWindow>,
    pub selected_window: usize,
    pub is_maximized: bool,
    pub has_scrolled: bool,
}

pub struct LogWindow {
    pub name: String,
    pub content: Arc<Mutex<Vec<String>>>,
    pub formatter: Arc<LogFormatter>,
    pub scroll_position: Arc<Mutex<usize>>,
    pub connection_status: Arc<Mutex<ConnectionStatus>>,
}

pub fn run_ui(app_state: &mut AppState) -> io::Result<()> {
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
        
        // 鬼知道为什么第一次进入最大化时无法暂停自动滚动
        INIT.call_once(|| {
            clear_history(app_state);
        });

        if event::poll(Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Enter | KeyCode::Char('m') => {
                        app_state.is_maximized = !app_state.is_maximized;
                        app_state.has_scrolled = false;
                        let window = &mut app_state.log_windows[app_state.selected_window];
                        let content_len = window.content.lock().unwrap().len();
                        let mut scroll_position = window.scroll_position.lock().unwrap();
                        *scroll_position = content_len.saturating_sub(1);
                    }
                    // KeyCode::Char('s') => {
                    //     // Save log
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
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::Down, window_height);
                        } else {
                            move_selection(app_state, MoveDirection::Down);
                        }
                    }
                    KeyCode::Up => {
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::Up, window_height);
                        } else {
                            move_selection(app_state, MoveDirection::Up);
                        }
                    }
                    KeyCode::Left => {
                        if app_state.is_maximized {
                        } else {
                            move_selection(app_state, MoveDirection::Left);
                        }
                    }
                    KeyCode::Right => {
                        if app_state.is_maximized {
                        } else {
                            move_selection(app_state, MoveDirection::Right);
                        }
                    }
                    KeyCode::PageDown => {
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::PageDown, window_height);
                        }
                    }
                    KeyCode::PageUp => {
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::PageUp, window_height);
                        }
                    }
                    KeyCode::Home => {
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::Top, window_height);
                        }
                    }
                    KeyCode::End => {
                        if app_state.is_maximized {
                            scroll_log(app_state, ScrollDirection::Bottom, window_height);
                        }
                    }
                    KeyCode::Char('r') => {
                        clear_history(app_state);
                        app_state.has_scrolled = false;
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

fn render_window(
    f: &mut Frame<CrosstermBackend<Stdout>>,
    window: &LogWindow,
    area: Rect,
    is_selected: bool,
    is_maximized: bool,
    has_scrolled: bool,
) {
    let content = window.content.lock().unwrap();
    let mut scroll_position = window.scroll_position.lock().unwrap();
    let connection_status = window.connection_status.lock().unwrap();

    let block = Block::default()
        .title(format!("{} (Scroll: {})", window.name, *scroll_position))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if is_selected {
            Color::Yellow
        } else {
            Color::White
        }));

    let inner_width = area.width as usize - 2;
    let height = area.height as usize - 2;

    let mut wrapped_content: Vec<Spans> = Vec::new();
    let mut total_lines: usize = 0;

    for line in content.iter() {
        let wrapped = wrap_line(line, inner_width);
        for wrapped_line in wrapped {
            wrapped_content.push(window.formatter.format_line(&wrapped_line));
            total_lines += 1;
        }
    }

    if !is_maximized || !has_scrolled {
        *scroll_position = total_lines.saturating_sub(height);
    } else {
        *scroll_position = (*scroll_position).min(total_lines.saturating_sub(height));
    }

    let start = *scroll_position;
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
            text[height - 1] = Spans::from(Span::styled(err_msg, Style::default().fg(Color::Red)));
        }
    }

    let paragraph = Paragraph::new(text)
        .block(block)
        .style(Style::default().fg(Color::White).bg(Color::Black));

    f.render_widget(paragraph, area);
}

fn render_maximized_window(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let selected_window = &app_state.log_windows[app_state.selected_window];
    render_window(
        f,
        selected_window,
        f.size(),
        true,
        app_state.is_maximized,
        app_state.has_scrolled,
    );
}

fn render_normal_layout(f: &mut Frame<CrosstermBackend<Stdout>>, app_state: &AppState) {
    let chunks = create_layout(f.size(), app_state.log_windows.len());

    for (i, log_window) in app_state.log_windows.iter().enumerate() {
        render_window(
            f,
            log_window,
            chunks[i],
            i == app_state.selected_window,
            app_state.is_maximized,
            app_state.has_scrolled,
        );
    }
}

fn create_layout(area: Rect, window_count: usize) -> Vec<Rect> {
    let constraints: Vec<Constraint> = (0..window_count)
        .map(|_| Constraint::Percentage((100 / window_count) as u16))
        .collect();

    Layout::default()
        .direction(LayoutDirection::Vertical)
        .constraints(constraints)
        .split(area)
}

fn clear_history(app_state: &mut AppState) {
    let window = &mut app_state.log_windows[app_state.selected_window];
    let mut content = window.content.lock().unwrap();
    content.clear();
    window.scroll_position = Arc::new(Mutex::new(0));
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

fn scroll_log(app_state: &mut AppState, direction: ScrollDirection, window_height: usize) {
    if !app_state.is_maximized {
        return;
    }

    let window = &mut app_state.log_windows[app_state.selected_window];
    let content_len = window.content.lock().unwrap().len();
    let mut scroll_position = window.scroll_position.lock().unwrap();

    // 计算每页的行数，减去2是为了考虑边框
    let page_size = window_height.saturating_sub(2);

    let old_scroll_position = *scroll_position;

    match direction {
        ScrollDirection::Up => {
            if *scroll_position > 0 {
                *scroll_position -= 1;
            }
        }
        ScrollDirection::Down => {
            if *scroll_position < content_len.saturating_sub(page_size) {
                *scroll_position += 1;
            }
        }
        ScrollDirection::PageUp => {
            *scroll_position = scroll_position.saturating_sub(page_size);
        }
        ScrollDirection::PageDown => {
            *scroll_position =
                (*scroll_position + page_size).min(content_len.saturating_sub(page_size));
        }
        ScrollDirection::Top => {
            *scroll_position = 0;
        }
        ScrollDirection::Bottom => {
            *scroll_position = content_len.saturating_sub(page_size);
        }
    }

    if *scroll_position != old_scroll_position {
        app_state.has_scrolled = true;
    }
}

fn move_selection(app_state: &mut AppState, direction: MoveDirection) {
    let window_count = app_state.log_windows.len();

    match direction {
        MoveDirection::Up => {
            if app_state.selected_window > 0 {
                app_state.selected_window -= 1;
            }
        }
        MoveDirection::Down => {
            if app_state.selected_window < window_count - 1 {
                app_state.selected_window += 1;
            }
        }
        MoveDirection::Left | MoveDirection::Right => {
            // 在单列布局中,左右移动不做任何操作
        }
    }
}

pub fn create_log_formatter() -> LogFormatter {
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
        .add_rule(r"ERROR|FATAL|FAILURE", Style::default().fg(Color::Red))
        .unwrap();
    formatter
        .add_rule(r"\{.*?\}", Style::default().fg(Color::Cyan))
        .unwrap();
    formatter
        .add_rule(r"INFO", Style::default().fg(Color::Blue))
        .unwrap();
    formatter
        .add_rule(
            r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
            Style::default().fg(Color::Magenta),
        )
        .unwrap();

    formatter
}

enum MoveDirection {
    Left,
    Right,
    Up,
    Down,
}

enum ScrollDirection {
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
}

struct MatchRule {
    regex: Regex,
    style: Style,
}

pub struct LogFormatter {
    rules: Vec<MatchRule>,
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
