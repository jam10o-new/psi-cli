use crate::models::{ChatMessage, FileEntry, MessageRole};
use chrono::Local;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Terminal,
};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    /// User submitted a message; file was auto-written to input_file
    Submit { message: String, input_file: Option<PathBuf> },
    /// No action
    None,
}

const INPUT_HEADER_STYLE: Style = Style::new()
    .fg(Color::Cyan)
    .add_modifier(Modifier::BOLD);

const OUTPUT_HEADER_STYLE: Style = Style::new()
    .fg(Color::Green)
    .add_modifier(Modifier::BOLD);

const SYSTEM_HEADER_STYLE: Style = Style::new()
    .fg(Color::Yellow)
    .add_modifier(Modifier::BOLD);

const INPUT_FOOTER_STYLE: Style = Style::new()
    .fg(Color::DarkGray);

const OUTPUT_FOOTER_STYLE: Style = Style::new()
    .fg(Color::DarkGray);

const SYSTEM_FOOTER_STYLE: Style = Style::new()
    .fg(Color::DarkGray);

const SELECTED_HIGHLIGHT: Style = Style::new()
    .fg(Color::White)
    .bg(Color::DarkGray)
    .add_modifier(Modifier::BOLD);

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    /// Normal mode: typing messages into the input box
    Normal,
    /// Select mode: navigating messages in the chat log with a cursor
    Select {
        cursor_index: usize,
    },
    /// Edit mode: editing a selected file's content in-place
    EditFile {
        target_path: PathBuf,
        buffer: String,
        cursor: usize,
        scroll_offset: u16,
        dirty: bool,
    },
    /// Import mode: typing a file path to copy into active input dir
    Import {
        buffer: String,
        cursor: usize,
        completions: Vec<PathBuf>,
        completion_index: usize,
    },
}

pub struct App {
    pub messages: Vec<ChatMessage>,
    pub input_text: String,
    pub input_cursor: usize,
    pub history: Vec<String>,
    pub history_index: isize,
    pub scroll_offset: usize,
    pub scroll_to_bottom: bool,
    pub chat_area_height: u16,
    pub active_input_dir: Option<PathBuf>,
    pub active_output_dir: Option<PathBuf>,
    pub input_dirs: Vec<PathBuf>,
    pub output_dirs: Vec<PathBuf>,
    pub system_dirs: Vec<PathBuf>,
    pub should_quit: bool,
    pub status_message: Option<String>,
    pub multiline: bool,
    pub mode: AppMode,
    // Track how many lines each message occupies (for scroll calculation in select mode)
    pub message_line_offsets: Vec<usize>,
    // Scriptlet paths
    pub on_submit_script: Option<PathBuf>,
    pub on_output_script: Option<PathBuf>,
}

impl App {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            input_text: String::new(),
            input_cursor: 0,
            history: Vec::new(),
            history_index: -1,
            scroll_offset: 0,
            scroll_to_bottom: false,
            chat_area_height: 0,
            active_input_dir: None,
            active_output_dir: None,
            input_dirs: Vec::new(),
            output_dirs: Vec::new(),
            system_dirs: Vec::new(),
            should_quit: false,
            status_message: None,
            multiline: false,
            mode: AppMode::Normal,
            message_line_offsets: Vec::new(),
            on_submit_script: None,
            on_output_script: None,
        }
    }

    pub fn update_messages(&mut self, entries: Vec<FileEntry>) {
        self.messages.clear();

        for entry in entries {
            // Paths are already canonicalized in the watcher
            let role = self.determine_role(&entry.path);
            let content = if entry.is_text {
                entry.content.unwrap_or_default()
            } else {
                format!(
                    "[{}] {} ({:.2} KB)",
                    entry.metadata.extension.to_uppercase(),
                    entry.path.file_name().unwrap_or_default().to_string_lossy(),
                    entry.metadata.size as f64 / 1024.0
                )
            };

            let filename = entry
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            self.messages.push(ChatMessage {
                role,
                content,
                filepath: entry.path,
                created_at: entry.created_at,
                filename,
            });
        }

        self.messages.sort_by_key(|m| m.created_at);
        self.recalculate_line_offsets();

        // Auto-scroll to bottom when new messages arrive
        self.scroll_to_bottom = true;
    }

    /// Calculate how many display lines each message occupies
    fn recalculate_line_offsets(&mut self) {
        self.message_line_offsets.clear();
        for msg in &self.messages {
            let content_lines = msg.content.lines().count().max(1);
            // +3 for header + footer + blank line
            self.message_line_offsets.push(content_lines + 3);
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(3);
                self.scroll_to_bottom = false;
            }
            MouseEventKind::ScrollDown => {
                self.scroll_offset += 3;
                self.scroll_to_bottom = false;
            }
            _ => {}
        }
    }

    fn determine_role(&self, path: &PathBuf) -> MessageRole {
        // Canonicalize for reliable prefix matching
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        let path_str = canonical.to_string_lossy();

        for dir in &self.input_dirs {
            let dir_canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if path_str.starts_with(&dir_canonical.to_string_lossy().as_ref()) {
                return MessageRole::Input;
            }
        }

        for dir in &self.system_dirs {
            let dir_canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if path_str.starts_with(&dir_canonical.to_string_lossy().as_ref()) {
                return MessageRole::System;
            }
        }

        MessageRole::Output
    }

    /// Submit user input. Returns the submitted message and the file path it was written to.
    /// Automatically writes to the active input directory if one is set.
    pub fn submit_input(&mut self) -> Option<(String, Option<PathBuf>)> {
        if !self.input_text.trim().is_empty() {
            self.history.push(self.input_text.clone());
            self.history_index = -1;
            let submitted = self.input_text.clone();
            let timestamp_ms = Local::now().timestamp_millis();

            // Write to active input directory if available
            let written_path = if let Some(ref input_dir) = self.active_input_dir {
                let filename = format!("input-{}.txt", timestamp_ms);
                let path = input_dir.join(&filename);
                if std::fs::write(&path, &submitted).is_ok() {
                    Some(path.canonicalize().unwrap_or(path))
                } else {
                    None
                }
            } else {
                None
            };

            self.messages.push(ChatMessage {
                role: MessageRole::Input,
                content: submitted.clone(),
                filepath: written_path.clone().unwrap_or_else(|| PathBuf::from("user-input")),
                created_at: Local::now(),
                filename: format!("input-{}.txt", timestamp_ms),
            });
            self.recalculate_line_offsets();

            self.input_text.clear();
            self.input_cursor = 0;
            Some((submitted, written_path))
        } else {
            None
        }
    }

    pub fn handle_key(&mut self, key: event::KeyEvent) -> InputAction {
        match self.mode {
            AppMode::Normal => self.handle_key_normal(key),
            AppMode::Select { .. } => self.handle_key_select(key),
            AppMode::EditFile { .. } => self.handle_key_edit(key),
            AppMode::Import { .. } => self.handle_key_import(key),
        }
    }

    fn handle_key_normal(&mut self, key: event::KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.should_quit = true;
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+J for newline
                self.multiline = true;
                self.input_text.insert(self.input_cursor, '\n');
                self.input_cursor += 1;
                self.multiline = false;
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+Enter to submit
                if let Some((msg, file_path)) = self.submit_input() {
                    return InputAction::Submit { message: msg, input_file: file_path };
                }
            }
            KeyCode::Enter if !self.multiline => {
                // Regular Enter to submit (single line mode)
                if let Some((msg, file_path)) = self.submit_input() {
                    return InputAction::Submit { message: msg, input_file: file_path };
                }
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+Up: Navigate up in history
                if self.history_index < self.history.len() as isize - 1 {
                    self.history_index += 1;
                    let idx = self.history_index as usize;
                    self.input_text = self.history[self.history.len() - 1 - idx].clone();
                    self.input_cursor = self.input_text.len();
                }
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+Down: Navigate down in history
                if self.history_index > 0 {
                    self.history_index -= 1;
                    let idx = self.history_index as usize;
                    self.input_text = self.history[self.history.len() - 1 - idx].clone();
                    self.input_cursor = self.input_text.len();
                } else if self.history_index == 0 {
                    self.history_index = -1;
                    self.input_text.clear();
                    self.input_cursor = 0;
                }
            }
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+R to rotate active input dir
                self.rotate_input_dir();
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+O to rotate active output dir
                self.rotate_output_dir();
            }
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+F: enter import mode
                self.mode = AppMode::Import {
                    buffer: String::new(),
                    cursor: 0,
                    completions: Vec::new(),
                    completion_index: 0,
                };
                self.status_message = None;
            }
            KeyCode::Tab => {
                // Tab: enter select mode
                let idx = self.messages.len().saturating_sub(1);
                self.mode = AppMode::Select { cursor_index: idx };
                self.status_message = Some("Select mode: ↑/↓ navigate | Enter edit | Esc cancel".into());
            }
            KeyCode::Backspace => {
                self.status_message = None;
                if self.input_cursor > 0 {
                    self.input_cursor -= 1;
                    self.input_text.remove(self.input_cursor);
                }
            }
            KeyCode::Delete => {
                self.status_message = None;
                if self.input_cursor < self.input_text.len() {
                    self.input_text.remove(self.input_cursor);
                }
            }
            KeyCode::Left => {
                if self.input_cursor > 0 {
                    self.input_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.input_cursor < self.input_text.len() {
                    self.input_cursor += 1;
                }
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset += 10;
            }
            KeyCode::Char('j') | KeyCode::Char('J') => {
                // vim-style scroll down (only when input box is truly empty)
                if self.input_text.is_empty() {
                    self.scroll_offset += 1;
                } else {
                    self.input_text.insert(self.input_cursor, 'j');
                    self.input_cursor += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                // vim-style scroll up (only when input box is truly empty)
                if self.input_text.is_empty() {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                } else {
                    self.input_text.insert(self.input_cursor, 'k');
                    self.input_cursor += 1;
                }
            }
            KeyCode::Up => {
                // Plain Up: scroll chat up
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down => {
                // Plain Down: scroll chat down
                self.scroll_offset += 1;
            }
            KeyCode::Char(c) => {
                self.status_message = None;
                self.input_text.insert(self.input_cursor, c);
                self.input_cursor += 1;
            }
            _ => {}
        }
        InputAction::None
    }

    fn handle_key_select(&mut self, key: event::KeyEvent) -> InputAction {
        let AppMode::Select { cursor_index } = self.mode else { return InputAction::None };

        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.status_message = None;
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                if cursor_index > 0 {
                    if let AppMode::Select { cursor_index: ci } = &mut self.mode {
                        *ci -= 1;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                if self.messages.is_empty() {
                    // Nothing to navigate
                } else if cursor_index < self.messages.len() - 1 {
                    if let AppMode::Select { cursor_index: ci } = &mut self.mode {
                        *ci += 1;
                    }
                }
            }
            KeyCode::PageUp => {
                if let AppMode::Select { cursor_index: ci } = &mut self.mode {
                    *ci = ci.saturating_sub(10);
                }
            }
            KeyCode::PageDown => {
                if let AppMode::Select { cursor_index: ci } = &mut self.mode {
                    let max = self.messages.len().saturating_sub(1);
                    *ci = (*ci + 10).min(max);
                }
            }
            KeyCode::Enter => {
                // Open the selected message for editing if it has a real file path
                if let AppMode::Select { cursor_index } = &self.mode {
                    if let Some(msg) = self.messages.get(*cursor_index) {
                        // Skip virtual messages
                        if msg.filepath == PathBuf::from("user-input") || !msg.filepath.exists() {
                            if msg.filepath == PathBuf::from("user-input") {
                                self.status_message = Some("Cannot edit: message was typed inline (no file)".into());
                            } else {
                                self.status_message = Some("Cannot edit: file does not exist on disk".into());
                            }
                            return InputAction::None;
                        }

                        // Always re-read fresh from disk when entering edit mode
                        if let Ok(content) = fs::read_to_string(&msg.filepath) {
                            let content_len = content.len();
                            self.mode = AppMode::EditFile {
                                target_path: msg.filepath.clone(),
                                buffer: content,
                                cursor: content_len,
                                scroll_offset: 0,
                                dirty: false,
                            };
                            self.status_message = Some(
                                format!("Editing: {:?} | Ctrl+S save | Esc cancel", msg.filepath),
                            );
                        } else {
                            self.status_message = Some(
                                "Cannot edit: file is not a text file or cannot be read".into(),
                            );
                        }
                        return InputAction::None;
                    }
                }
            }
            _ => {}
        }
        InputAction::None
    }

    fn handle_key_edit(&mut self, key: event::KeyEvent) -> InputAction {
        let AppMode::EditFile { cursor, .. } = self.mode else { return InputAction::None };

        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Save to original file path
                if let AppMode::EditFile { target_path, buffer, dirty, .. } = &mut self.mode {
                    if target_path.exists() {
                        let path = target_path.clone();
                        if let Err(e) = fs::write(&path, buffer) {
                            self.status_message = Some(format!("Save failed: {}", e));
                        } else {
                            *dirty = false;
                            self.status_message = Some(format!("Saved: {:?}", path));
                        }
                    } else {
                        self.status_message = Some("Original file no longer exists on disk".into());
                    }
                }
            }
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.status_message = None;
            }
            KeyCode::Backspace => {
                if cursor > 0 {
                    if let AppMode::EditFile { buffer, cursor, dirty, .. } = &mut self.mode {
                        let c = *cursor;
                        buffer.remove(c - 1);
                        *cursor = c - 1;
                        *dirty = true;
                    }
                }
            }
            KeyCode::Delete => {
                if let AppMode::EditFile { buffer, cursor, dirty, .. } = &mut self.mode {
                    if *cursor < buffer.len() {
                        buffer.remove(*cursor);
                        *dirty = true;
                    }
                }
            }
            KeyCode::Left => {
                if let AppMode::EditFile { cursor, .. } = &mut self.mode {
                    if *cursor > 0 {
                        *cursor -= 1;
                    }
                }
            }
            KeyCode::Right => {
                if let AppMode::EditFile { buffer, cursor, .. } = &mut self.mode {
                    if *cursor < buffer.len() {
                        *cursor += 1;
                    }
                }
            }
            KeyCode::Up => {
                if let AppMode::EditFile { scroll_offset, .. } = &mut self.mode {
                    *scroll_offset = scroll_offset.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if let AppMode::EditFile { scroll_offset, .. } = &mut self.mode {
                    *scroll_offset += 1;
                }
            }
            KeyCode::PageUp => {
                if let AppMode::EditFile { scroll_offset, .. } = &mut self.mode {
                    *scroll_offset = scroll_offset.saturating_sub(10);
                }
            }
            KeyCode::PageDown => {
                if let AppMode::EditFile { scroll_offset, .. } = &mut self.mode {
                    *scroll_offset += 10;
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::EditFile { buffer, cursor, dirty, .. } = &mut self.mode {
                    buffer.insert(*cursor, c);
                    *cursor += 1;
                    *dirty = true;
                }
            }
            _ => {}
        }
        InputAction::None
    }

    fn handle_key_import(&mut self, key: event::KeyEvent) -> InputAction {
        let AppMode::Import { buffer, cursor, completions, completion_index } = &self.mode else { return InputAction::None };
        let buf = buffer.clone();
        let cur = *cursor;
        let comps = completions.clone();
        let ci = *completion_index;

        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.status_message = None;
            }
            KeyCode::Enter => {
                // Copy the selected file to active input dir
                let AppMode::Import { buffer, completions, completion_index, .. } = &self.mode else { return InputAction::None };
                let target = if !completions.is_empty() {
                    completions.get(*completion_index).or_else(|| completions.first())
                } else {
                    Some(&PathBuf::from(buffer.as_str()))
                };

                if let Some(src_path) = target {
                    if src_path.exists() {
                        if let Some(ref active_dir) = self.active_input_dir {
                            let dest_name = src_path.file_name().unwrap_or_default();
                            let dest = active_dir.join(dest_name);
                            match fs::copy(src_path, &dest) {
                                Ok(_) => {
                                    self.status_message = Some(format!("Copied {:?} → {:?}", src_path, dest));
                                }
                                Err(e) => {
                                    self.status_message = Some(format!("Copy failed: {}", e));
                                }
                            }
                        } else {
                            self.status_message = Some("No active input directory set".into());
                        }
                    } else {
                        self.status_message = Some("File does not exist".into());
                    }
                }
                self.mode = AppMode::Normal;
            }
            KeyCode::Tab => {
                // Cycle through completions
                if !comps.is_empty() {
                    let next = (ci + 1) % comps.len();
                    let new_path = comps[next].to_string_lossy().to_string();
                    let new_len = new_path.len();
                    if let AppMode::Import { buffer, completion_index, .. } = &mut self.mode {
                        *buffer = new_path;
                        *completion_index = next;
                    }
                    // cursor = new_len - but we need to do this in the same borrow
                    if let AppMode::Import { cursor, .. } = &mut self.mode {
                        *cursor = new_len;
                    }
                }
            }
            KeyCode::Backspace => {
                if cur > 0 {
                    let new_buf = {
                        let mut b = buf.clone();
                        b.remove(cur - 1);
                        b
                    };
                    let new_cur = cur - 1;
                    let new_completions = self.compute_path_completions(&new_buf);
                    if let AppMode::Import { buffer, cursor, completions, completion_index } = &mut self.mode {
                        *buffer = new_buf;
                        *cursor = new_cur;
                        *completions = new_completions;
                        *completion_index = 0;
                    }
                }
            }
            KeyCode::Char(c) => {
                let new_buf = {
                    let mut b = buf.clone();
                    b.insert(cur, c);
                    b
                };
                let new_cur = cur + 1;
                let new_completions = self.compute_path_completions(&new_buf);
                if let AppMode::Import { buffer, cursor, completions, completion_index } = &mut self.mode {
                    *buffer = new_buf;
                    *cursor = new_cur;
                    *completions = new_completions;
                    *completion_index = 0;
                }
            }
            _ => {}
        }
        InputAction::None
    }

    fn compute_path_completions(&self, input: &str) -> Vec<PathBuf> {
        if input.is_empty() {
            return Vec::new();
        }

        // Expand ~ to home dir
        let expanded = if input.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                home.join(&input[2..]).to_string_lossy().to_string()
            } else {
                input.to_string()
            }
        } else {
            input.to_string()
        };

        // Use glob to find matching paths
        let pattern = if expanded.ends_with('/') {
            format!("{}*", expanded)
        } else {
            format!("{}*", expanded)
        };

        let mut matches: Vec<PathBuf> = glob::glob(&pattern)
            .ok()
            .into_iter()
            .flat_map(|g| g.filter_map(|p| p.ok()))
            .collect();

        matches.sort();
        matches.truncate(50); // Cap completions
        matches
    }

    fn rotate_input_dir(&mut self) {
        if self.input_dirs.is_empty() {
            return;
        }

        let current_idx = if let Some(ref active) = self.active_input_dir {
            self.input_dirs.iter().position(|d| d == active).unwrap_or(0)
        } else {
            0
        };

        let next_idx = (current_idx + 1) % self.input_dirs.len();
        self.active_input_dir = Some(self.input_dirs[next_idx].clone());
        self.status_message = Some(format!(
            "Active input: {:?}",
            self.input_dirs[next_idx].file_name().unwrap_or_default()
        ));
    }

    fn rotate_output_dir(&mut self) {
        if self.output_dirs.is_empty() {
            return;
        }

        let current_idx = if let Some(ref active) = self.active_output_dir {
            self.output_dirs.iter().position(|d| d == active).unwrap_or(0)
        } else {
            0
        };

        let next_idx = (current_idx + 1) % self.output_dirs.len();
        self.active_output_dir = Some(self.output_dirs[next_idx].clone());
        self.status_message = Some(format!(
            "Active output: {:?}",
            self.output_dirs[next_idx].file_name().unwrap_or_default()
        ));
    }
}

pub fn ui(f: &mut ratatui::Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Min(1),
                Constraint::Max(3),
            ]
            .as_ref(),
        )
        .split(f.area());

    // Record chat area height for scroll calculations
    app.chat_area_height = chunks[0].height.saturating_sub(2);

    // Chat log area
    render_chat_log(f, chunks[0], app);

    // Input area
    match &app.mode {
        AppMode::Normal | AppMode::Select { .. } => render_input(f, chunks[1], app),
        AppMode::EditFile { .. } => render_edit_file(f, chunks[1], app),
        AppMode::Import { .. } => render_import(f, chunks[1], app),
    }
}

fn render_chat_log(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let mut lines = Vec::new();

    // Add a header
    lines.push(Line::from(Span::styled(
        "═══ Chat Log ═══",
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (i, msg) in app.messages.iter().enumerate() {
        let (header_style, footer_style, role_label) = match msg.role {
            MessageRole::Input => (
                INPUT_HEADER_STYLE,
                INPUT_FOOTER_STYLE,
                "USER INPUT",
            ),
            MessageRole::Output => (
                OUTPUT_HEADER_STYLE,
                OUTPUT_FOOTER_STYLE,
                "AGENT OUTPUT",
            ),
            MessageRole::System => (
                SYSTEM_HEADER_STYLE,
                SYSTEM_FOOTER_STYLE,
                "SYSTEM",
            ),
        };

        // In select mode, highlight the selected message
        let is_selected = if let AppMode::Select { cursor_index } = app.mode {
            i == cursor_index
        } else {
            false
        };

        let header_style = if is_selected { SELECTED_HIGHLIGHT } else { header_style };
        let role_prefix = if is_selected { "▶ " } else { "" };

        // Header
        let timestamp = msg.created_at.format("%H:%M:%S");
        lines.push(Line::from(vec![
            Span::styled(
                format!("╔══ {}[{}] ", role_prefix, role_label),
                header_style,
            ),
            Span::styled(
                format!("{}", timestamp),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(" ══", header_style),
        ]));

        // Content (word wrap)
        for line in msg.content.lines() {
            lines.push(Line::from(vec![
                Span::styled("║  ", header_style),
                Span::raw(line.to_string()),
            ]));
        }

        // Footer
        lines.push(Line::from(Span::styled(
            "╚══════════════════════════════════════════════════════════",
            footer_style,
        )));
        lines.push(Line::from(""));
    }

    // Calculate visible height (subtract borders)
    let visible_height = area.height.saturating_sub(2) as usize;

    // Clamp scroll_offset to actual content bounds
    let max_offset = lines.len().saturating_sub(visible_height);
    app.scroll_offset = app.scroll_offset.min(max_offset);

    // In select mode, auto-scroll to keep cursor visible
    if let AppMode::Select { cursor_index } = app.mode {
        // Calculate where the cursor is
        let cursor_line: usize = app.message_line_offsets.iter().take(cursor_index).sum::<usize>() + 2; // +2 for header lines
        let cursor_bottom = cursor_line + app.message_line_offsets.get(cursor_index).copied().unwrap_or(3);

        if cursor_bottom > visible_height + app.scroll_offset {
            app.scroll_offset = cursor_bottom.saturating_sub(visible_height).min(max_offset);
        }
        if cursor_line < app.scroll_offset {
            app.scroll_offset = cursor_line;
        }
    }

    // Auto-scroll: if we need to scroll to bottom, compute max offset
    let actual_scroll_offset = if app.scroll_to_bottom && !matches!(app.mode, AppMode::Select { .. }) {
        max_offset
    } else {
        app.scroll_offset
    };

    let chat = Paragraph::new(lines)
        .scroll((actual_scroll_offset as u16, 0))
        .block(Block::default().borders(Borders::ALL).title("Messages"));

    f.render_widget(chat, area);
}

fn render_input(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let input_lines = if app.multiline || app.input_text.contains('\n') {
        app.input_text.lines().map(|l| Line::from(l.to_string())).collect()
    } else {
        vec![Line::from(app.input_text.clone())]
    };

    let mode_label = match app.mode {
        AppMode::Select { .. } => "SELECT ",
        _ => "",
    };

    let input = Paragraph::new(input_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{}Input (Tab: select | Ctrl+F: import | Ctrl+J: nl | Ctrl+Enter: submit | ↑/↓: scroll | Ctrl+↑/↓: history | Ctrl+R: rotate input | Ctrl+O: rotate output)", mode_label)),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(input, area);

    if matches!(app.mode, AppMode::Normal) {
        let cursor_x = area.x + 1 + app.input_cursor as u16;
        let cursor_y = area.y + 1;
        f.set_cursor_position((cursor_x.min(area.right() - 2), cursor_y));
    }
}

fn render_edit_file(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let AppMode::EditFile { target_path, buffer, cursor, scroll_offset, dirty } = &app.mode else { return };

    let title = if *dirty {
        format!("Edit: {:?} [*]", target_path)
    } else {
        format!("Edit: {:?}", target_path)
    };

    let edit_text = Paragraph::new(buffer.as_str())
        .scroll((*scroll_offset, 0))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .style(Style::default().fg(Color::White));

    f.render_widget(edit_text, area);

    // Calculate cursor position
    let visible_height = area.height.saturating_sub(2);
    let cursor_line = buffer[..*cursor].matches('\n').count() as u16;
    let cursor_col = {
        let since_last_newline = buffer.rsplitn(*cursor + 1, |c| c == '\n').next().unwrap_or("").len();
        since_last_newline as u16
    };

    if cursor_line >= *scroll_offset && cursor_line < *scroll_offset + visible_height {
        f.set_cursor_position((
            area.x + 1 + cursor_col.min(area.width.saturating_sub(2)),
            area.y + 1 + (cursor_line - *scroll_offset),
        ));
    }
}

fn render_import(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let AppMode::Import { buffer, cursor, completions, completion_index } = &app.mode else { return };

    // Show input with path being typed
    let input = Paragraph::new(buffer.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Import file (Enter: copy | Tab: cycle | Esc: cancel)"),
        )
        .style(Style::default().fg(Color::Cyan));

    f.render_widget(input, area);

    // Set cursor position
    let cursor_x = area.x + 1 + *cursor as u16;
    let cursor_y = area.y + 1;
    f.set_cursor_position((cursor_x.min(area.right() - 2), cursor_y));

    // Show completions
    if !completions.is_empty() {
        let max_w = area.width as usize;
        let comp_lines: Vec<Line> = completions.iter().enumerate().map(|(i, p)| {
            let path_str = p.to_string_lossy().to_string();
            let truncated = if path_str.len() > max_w {
                format!("...{}", &path_str[path_str.len().saturating_sub(max_w - 3)..])
            } else {
                path_str
            };

            if i == *completion_index {
                Line::from(Span::styled(
                    format!("▶ {}", truncated),
                    SELECTED_HIGHLIGHT,
                ))
            } else {
                Line::from(Span::raw(truncated))
            }
        }).take(5).collect(); // Show max 5 completions

        let comp_block = Paragraph::new(comp_lines)
            .block(Block::default().borders(Borders::TOP).title("Completions"))
            .style(Style::default().fg(Color::DarkGray));

        f.render_widget(comp_block, area);
    }
}

pub async fn run_tui(
    app: Arc<Mutex<App>>,
    on_submit_script: Option<PathBuf>,
    input_dirs: Vec<PathBuf>,
    output_dirs: Vec<PathBuf>,
    active_input_dir: Option<PathBuf>,
    active_output_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    // Setup terminal
    let stdout = io::stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Enable raw mode and alternate screen
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    crossterm::terminal::enable_raw_mode()?;

    // Main loop
    let result = run_main_loop(
        &mut terminal,
        app,
        on_submit_script,
        input_dirs,
        output_dirs,
        active_input_dir,
        active_output_dir,
    )
    .await;

    // Restore terminal
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_main_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: Arc<Mutex<App>>,
    on_submit_script: Option<PathBuf>,
    input_dirs: Vec<PathBuf>,
    output_dirs: Vec<PathBuf>,
    active_input_dir: Option<PathBuf>,
    active_output_dir: Option<PathBuf>,
) -> anyhow::Result<()> {
    use crate::models::ScriptletContext;
    use crate::scriptlet::ScriptletRunner;
    use chrono::Local;

    loop {
        {
            let mut app_guard = app.lock().await;
            if app_guard.should_quit {
                return Ok(());
            }

            terminal.draw(|f| {
                ui(f, &mut app_guard);
            })?;
        }

        // Handle input events
        if crossterm::event::poll(std::time::Duration::from_millis(50))? {
            match crossterm::event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        let mut app_guard = app.lock().await;
                        let action = app_guard.handle_key(key);

                        // Dispatch submit scriptlet
                        if let InputAction::Submit { message, input_file } = action {
                            if let Some(ref script) = on_submit_script {
                                // Release the lock before calling scriptlet
                                drop(app_guard);
                                let context = ScriptletContext {
                                    latest_input_file: input_file.clone(),
                                    latest_output_file: None,
                                    active_input_dir: active_input_dir.clone(),
                                    active_output_dir: active_output_dir.clone(),
                                    input_dirs: input_dirs.clone(),
                                    output_dirs: output_dirs.clone(),
                                    timestamp: Local::now(),
                                    user_message: Some(message),
                                    agent_response: None,
                                };
                                let _ = ScriptletRunner::execute_on_submit(script, &context).await;
                            }
                        }
                    }
                }
                Event::Mouse(mouse) => {
                    let mut app_guard = app.lock().await;
                    app_guard.handle_mouse(mouse);
                }
                _ => {}
            }
        }
    }
}
