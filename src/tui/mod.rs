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
    /// User saved an edit in-place (no scriptlet dispatch)
    Saved { path: PathBuf },
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

/// Compute the display line (row offset from top) and column for a byte offset
/// in text with word wrapping at the given max inner width.
fn wrapped_cursor(text: &str, byte_offset: usize, max_inner_width: u16) -> (usize, usize) {
    if max_inner_width == 0 {
        return (0, 0);
    }
    let max_w = max_inner_width as usize;

    fn char_display_width(c: char) -> usize {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }

    // We walk through the text up to byte_offset, tracking which display
    // line and column the cursor ends up on.
    let text_up_to = &text[..byte_offset.min(text.len())];

    let mut display_line = 0usize;
    let mut current_line_width = 0usize;

    for ch in text_up_to.chars() {
        if ch == '\n' {
            // Explicit newline: move to next display line
            display_line += 1;
            current_line_width = 0;
        } else {
            let w = char_display_width(ch);
            if current_line_width + w > max_w && current_line_width > 0 {
                // Wrap to next display line
                display_line += 1;
                current_line_width = w;
            } else {
                current_line_width += w;
            }
        }
    }

    (display_line, current_line_width)
}

/// Compute how many display lines the text will occupy when wrapped.
fn wrapped_line_count(text: &str, max_inner_width: u16) -> usize {
    if max_inner_width == 0 {
        return 1;
    }
    let max_w = max_inner_width as usize;

    fn char_display_width(c: char) -> usize {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }

    if text.is_empty() {
        return 1;
    }

    let mut count = 1usize;
    let mut current_line_width = 0usize;

    for ch in text.chars() {
        if ch == '\n' {
            count += 1;
            current_line_width = 0;
        } else {
            let w = char_display_width(ch);
            if current_line_width + w > max_w && current_line_width > 0 {
                count += 1;
                current_line_width = w;
            } else {
                current_line_width += w;
            }
        }
    }

    count.max(1)
}

/// Build a list of byte offsets where each display line starts.
/// Returns [0, line2_start_byte, line3_start_byte, ...]
fn wrapped_line_starts(text: &str, max_inner_width: u16) -> Vec<usize> {
    if max_inner_width == 0 {
        return vec![0];
    }
    let max_w = max_inner_width as usize;

    fn char_display_width(c: char) -> usize {
        unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
    }

    let mut starts = vec![0usize];
    let mut current_line_width = 0usize;
    let mut byte_offset = 0usize;

    for ch in text.chars() {
        let ch_len = ch.len_utf8();
        if ch == '\n' {
            // Next display line starts after the newline
            byte_offset += ch_len;
            starts.push(byte_offset);
            current_line_width = 0;
        } else {
            let w = char_display_width(ch);
            if current_line_width + w > max_w && current_line_width > 0 {
                // Wrap: this char starts a new display line
                starts.push(byte_offset);
                current_line_width = w;
                byte_offset += ch_len;
            } else {
                current_line_width += w;
                byte_offset += ch_len;
            }
        }
    }

    if starts.is_empty() {
        starts.push(0);
    }
    starts
}

/// Move the cursor up or down by one display line in wrapped text.
/// Returns the new byte offset.
fn move_cursor_vertical(text: &str, current_offset: usize, max_inner_width: u16, direction: i32) -> usize {
    if text.is_empty() {
        return 0;
    }
    let line_starts = wrapped_line_starts(text, max_inner_width);

    // Find which display line the cursor is currently on
    let mut current_display_line = 0;
    let mut cursor_col = 0;
    for (i, &start) in line_starts.iter().enumerate() {
        let end = if i + 1 < line_starts.len() { line_starts[i + 1] } else { text.len() };
        if current_offset >= start && current_offset <= end {
            current_display_line = i;
            // Calculate column position within this line
            let prefix = &text[start..current_offset.min(text.len())];
            let mut w = 0;
            for c in prefix.chars() {
                w += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            }
            cursor_col = w;
            break;
        }
    }

    let target_line = if direction < 0 {
        current_display_line.saturating_sub(1)
    } else {
        (current_display_line + 1).min(line_starts.len() - 1)
    };

    if target_line == current_display_line {
        return current_offset; // Already at top/bottom
    }

    // Place cursor at the same column on the target line
    let target_start = line_starts[target_line];
    let target_end = if target_line + 1 < line_starts.len() {
        line_starts[target_line + 1]
    } else {
        text.len()
    };

    // Walk through target line chars to find position at cursor_col
    let mut col = 0;
    let mut byte_pos = target_start;
    let segment = &text[target_start..target_end];
    for ch in segment.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > cursor_col {
            break;
        }
        col += w;
        byte_pos += ch.len_utf8();
    }

    byte_pos.min(text.len())
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    /// Normal mode: typing messages into the input box
    Normal,
    /// Select mode: navigating messages in the chat log with a cursor
    Select {
        cursor_index: usize,
    },
    /// Edit mode: editing a selected file's content in-place using the shared input box
    EditFile {
        target_path: PathBuf,
        dirty: bool,
    },
    /// Import mode: typing a file path to copy into active input dir
    Import {
        buffer: String,
        cursor: usize,
        completions: Vec<PathBuf>,
        completion_index: usize,
    },
    /// Add input directory: type path to add to input_dirs
    AddInputDir {
        buffer: String,
        cursor: usize,
        completions: Vec<PathBuf>,
        completion_index: usize,
    },
    /// Add output directory: type path to add to output_dirs
    AddOutputDir {
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
    /// Cached inner width of the input area (excluding borders), used for wrapped cursor nav
    pub input_inner_width: u16,
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
            input_inner_width: 80,
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
            AppMode::AddInputDir { .. } => self.handle_key_add_dir(key, true),
            AppMode::AddOutputDir { .. } => self.handle_key_add_dir(key, false),
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
            KeyCode::Char('i') if key.modifiers.contains(KeyModifiers::ALT) => {
                // Alt+I: add input directory
                self.mode = AppMode::AddInputDir {
                    buffer: String::new(),
                    cursor: 0,
                    completions: Vec::new(),
                    completion_index: 0,
                };
                self.status_message = Some("Add input directory (Enter: add | Tab: cycle | Esc cancel)".into());
            }
            KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::ALT) => {
                // Alt+O: add output directory
                self.mode = AppMode::AddOutputDir {
                    buffer: String::new(),
                    cursor: 0,
                    completions: Vec::new(),
                    completion_index: 0,
                };
                self.status_message = Some("Add output directory (Enter: add | Tab: cycle | Esc cancel)".into());
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
                // PageUp: scroll chat up
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
                self.scroll_to_bottom = false;
            }
            KeyCode::PageDown => {
                // PageDown: scroll chat down
                self.scroll_offset += 10;
                self.scroll_to_bottom = false;
            }
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+End: scroll to bottom of chat
                self.scroll_to_bottom = true;
            }
            KeyCode::End => {
                // End: also scrolls to bottom of chat
                self.scroll_to_bottom = true;
            }
            KeyCode::Up => {
                // Up: move cursor up one display line in input (across wrapped lines)
                let w = self.input_inner_width.max(1);
                self.input_cursor = move_cursor_vertical(&self.input_text, self.input_cursor, w, -1);
            }
            KeyCode::Down => {
                // Down: move cursor down one display line in input (across wrapped lines)
                let w = self.input_inner_width.max(1);
                self.input_cursor = move_cursor_vertical(&self.input_text, self.input_cursor, w, 1);
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
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+D: delete selected file
                if let AppMode::Select { cursor_index } = &self.mode {
                    if let Some(msg) = self.messages.get(*cursor_index) {
                        if msg.filepath == PathBuf::from("user-input") {
                            self.status_message = Some("Cannot delete: message was typed inline (no file)".into());
                            return InputAction::None;
                        }
                        if !msg.filepath.exists() {
                            self.status_message = Some("Cannot delete: file does not exist on disk".into());
                            return InputAction::None;
                        }
                        let path = msg.filepath.clone();
                        self.messages.remove(*cursor_index);
                        self.recalculate_line_offsets();
                        if let Err(e) = fs::remove_file(&path) {
                            self.status_message = Some(format!("Delete failed: {}", e));
                        } else {
                            self.status_message = Some(format!("Deleted: {:?}", path));
                        }
                        if let AppMode::Select { cursor_index: ci } = &mut self.mode {
                            *ci = (*ci).min(self.messages.len().saturating_sub(1));
                        }
                    }
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
                            // Populate the shared input box with file content
                            self.input_text = content;
                            self.input_cursor = self.input_text.len();
                            self.mode = AppMode::EditFile {
                                target_path: msg.filepath.clone(),
                                dirty: false,
                            };
                            self.status_message = Some(
                                format!("Editing: {:?} | Ctrl+S save | Ctrl+Enter save+exit | Esc cancel", msg.filepath),
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
        let AppMode::EditFile { target_path, dirty: _ } = &self.mode else { return InputAction::None };

        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+S: save to original file path, stay in edit mode
                let target_path = target_path.clone();
                if target_path.exists() {
                    if let Err(e) = fs::write(&target_path, &self.input_text) {
                        self.status_message = Some(format!("Save failed: {}", e));
                    } else {
                        if let AppMode::EditFile { dirty, .. } = &mut self.mode {
                            *dirty = false;
                        }
                    }
                } else {
                    self.status_message = Some("Original file no longer exists on disk".into());
                }
                // Restore edit status message so it's not lost
                if let AppMode::EditFile { target_path: tp, dirty } = &self.mode {
                    let marker = if *dirty { " [*]" } else { "" };
                    self.status_message = Some(
                        format!("Editing: {:?}{} | Ctrl+S save | Enter:save+exit | Esc cancel", tp, marker),
                    );
                }
            }
            KeyCode::Esc => {
                // Esc: cancel edit, return to normal mode
                self.mode = AppMode::Normal;
                self.status_message = None;
                return InputAction::None;
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Ctrl+Enter: save and exit (same as plain Enter in edit mode)
                return self.handle_edit_save_and_exit();
            }
            KeyCode::Enter => {
                // Enter: save and exit back to select mode
                return self.handle_edit_save_and_exit();
            }
            _ => {
                // Delegate all other keys to the normal handler
                // (typing, cursor nav, Ctrl+J newline, etc.)
                let len_before = self.input_text.len();
                self.handle_key_normal(key);
                // Mark dirty if the normal handler modified input
                if self.input_text.len() != len_before {
                    if let AppMode::EditFile { dirty, .. } = &mut self.mode {
                        *dirty = true;
                    }
                }
                // Restore edit status after normal handler may have cleared it
                if let AppMode::EditFile { target_path: tp, dirty } = &self.mode {
                    let marker = if *dirty { " [*]" } else { "" };
                    self.status_message = Some(
                        format!("Editing: {:?}{} | Ctrl+S save | Enter:save+exit | Esc cancel", tp, marker),
                    );
                }
                return InputAction::None;
            }
        }
        InputAction::None
    }

    /// Common save-and-exit logic for edit mode (called by Enter / Ctrl+Enter).
    fn handle_edit_save_and_exit(&mut self) -> InputAction {
        let AppMode::EditFile { target_path, .. } = &self.mode else {
            return InputAction::None;
        };
        let target_path = target_path.clone();
        if target_path.exists() {
            if let Err(e) = fs::write(&target_path, &self.input_text) {
                self.status_message = Some(format!("Save failed: {}", e));
                return InputAction::None;
            }
        }
        self.mode = AppMode::Normal;
        self.status_message = None;
        InputAction::Saved { path: target_path }
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

    /// Generic handler for "add directory" modes (AddInputDir / AddOutputDir)
    fn handle_key_add_dir(&mut self, key: event::KeyEvent, is_input: bool) -> InputAction {
        let (buf, cur, comps, ci) = if is_input {
            match &self.mode {
                AppMode::AddInputDir { buffer, cursor, completions, completion_index } => {
                    (buffer.clone(), *cursor, completions.clone(), *completion_index)
                }
                _ => return InputAction::None,
            }
        } else {
            match &self.mode {
                AppMode::AddOutputDir { buffer, cursor, completions, completion_index } => {
                    (buffer.clone(), *cursor, completions.clone(), *completion_index)
                }
                _ => return InputAction::None,
            }
        };

        match key.code {
            KeyCode::Esc => {
                self.mode = AppMode::Normal;
                self.status_message = None;
            }
            KeyCode::Enter => {
                // Add the selected/completed directory
                let target = if !comps.is_empty() {
                    comps.get(ci).or_else(|| comps.first())
                } else {
                    Some(&PathBuf::from(buf.as_str()))
                };

                if let Some(dir_path) = target {
                    if dir_path.is_dir() {
                        if is_input {
                            if let AppMode::AddInputDir { .. } = &mut self.mode {
                                if !self.input_dirs.contains(dir_path) {
                                    self.input_dirs.push(dir_path.clone());
                                    self.active_input_dir = Some(dir_path.clone());
                                    self.status_message = Some(format!("Added input: {:?}", dir_path));
                                } else {
                                    self.status_message = Some("Already in input dirs".into());
                                }
                            }
                        } else {
                            if let AppMode::AddOutputDir { .. } = &mut self.mode {
                                if !self.output_dirs.contains(dir_path) {
                                    self.output_dirs.push(dir_path.clone());
                                    self.active_output_dir = Some(dir_path.clone());
                                    self.status_message = Some(format!("Added output: {:?}", dir_path));
                                } else {
                                    self.status_message = Some("Already in output dirs".into());
                                }
                            }
                        }
                    } else {
                        self.status_message = Some("Not a directory".into());
                    }
                }
                self.mode = AppMode::Normal;
            }
            KeyCode::Tab => {
                if !comps.is_empty() {
                    let next = (ci + 1) % comps.len();
                    let new_path = comps[next].to_string_lossy().to_string();
                    let new_len = new_path.len();
                    if is_input {
                        if let AppMode::AddInputDir { buffer, completion_index, cursor, .. } = &mut self.mode {
                            *buffer = new_path;
                            *completion_index = next;
                            *cursor = new_len;
                        }
                    } else {
                        if let AppMode::AddOutputDir { buffer, completion_index, cursor, .. } = &mut self.mode {
                            *buffer = new_path;
                            *completion_index = next;
                            *cursor = new_len;
                        }
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
                    if is_input {
                        if let AppMode::AddInputDir { buffer, cursor, completions, completion_index } = &mut self.mode {
                            *buffer = new_buf;
                            *cursor = new_cur;
                            *completions = new_completions;
                            *completion_index = 0;
                        }
                    } else {
                        if let AppMode::AddOutputDir { buffer, cursor, completions, completion_index } = &mut self.mode {
                            *buffer = new_buf;
                            *cursor = new_cur;
                            *completions = new_completions;
                            *completion_index = 0;
                        }
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
                if is_input {
                    if let AppMode::AddInputDir { buffer, cursor, completions, completion_index } = &mut self.mode {
                        *buffer = new_buf;
                        *cursor = new_cur;
                        *completions = new_completions;
                        *completion_index = 0;
                    }
                } else {
                    if let AppMode::AddOutputDir { buffer, cursor, completions, completion_index } = &mut self.mode {
                        *buffer = new_buf;
                        *cursor = new_cur;
                        *completions = new_completions;
                        *completion_index = 0;
                    }
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
    let inner_width = f.area().width.saturating_sub(4); // margins + borders
    let content_lines = wrapped_line_count(&app.input_text, inner_width);
    // Input area: 5 instruction lines + optional status line + content lines + borders
    let instruction_count = 5u16;
    let status_count = if app.status_message.is_some() { 1 } else { 0 };
    let total_needed = instruction_count + status_count + content_lines as u16 + 2;
    let input_height = total_needed.min(20).max(9 + status_count);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints(
            [
                Constraint::Min(1),
                Constraint::Length(input_height),
            ]
            .as_ref(),
        )
        .split(f.area());

    // Record chat area height and input width for scroll/cursor calculations
    app.chat_area_height = chunks[0].height.saturating_sub(2);
    app.input_inner_width = chunks[1].width.saturating_sub(2).max(1);

    // Chat log area
    render_chat_log(f, chunks[0], app);

    // Input area (Normal, Select, and EditFile all share the same input box)
    match &app.mode {
        AppMode::Normal | AppMode::Select { .. } | AppMode::EditFile { .. } => render_input(f, chunks[1], app),
        AppMode::Import { .. } => render_import(f, chunks[1], app),
        AppMode::AddInputDir { .. } => render_add_dir(f, chunks[1], app, true),
        AppMode::AddOutputDir { .. } => render_add_dir(f, chunks[1], app, false),
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
    let max_offset = lines.len().saturating_sub(visible_height);

    // In select mode, auto-scroll to keep cursor visible
    if let AppMode::Select { cursor_index } = app.mode {
        let cursor_line: usize = app.message_line_offsets.iter().take(cursor_index).sum::<usize>() + 2;
        let cursor_bottom = cursor_line + app.message_line_offsets.get(cursor_index).copied().unwrap_or(3);

        if cursor_bottom > visible_height + app.scroll_offset {
            app.scroll_offset = cursor_bottom.saturating_sub(visible_height).min(max_offset);
        }
        if cursor_line < app.scroll_offset {
            app.scroll_offset = cursor_line;
        }
    }

    // Auto-scroll to bottom on new content or explicit request, then clear
    // the flag so manual scrolling is not immediately overridden.
    let actual_scroll_offset = if app.scroll_to_bottom && !matches!(app.mode, AppMode::Select { .. }) {
        app.scroll_to_bottom = false;
        app.scroll_offset = max_offset;
        max_offset
    } else {
        app.scroll_offset.min(max_offset)
    };

    let chat = Paragraph::new(lines)
        .scroll((actual_scroll_offset as u16, 0))
        .block(Block::default().borders(Borders::ALL).title("Messages"));

    f.render_widget(chat, area);
}

fn render_input(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    let inner_width = area.width.saturating_sub(2); // 2 for borders

    let is_edit = matches!(app.mode, AppMode::EditFile { .. });

    // Build instruction lines — edit mode gets save/exit shortcuts
    let instruction_lines = if is_edit {
        let edit_info = if let AppMode::EditFile { target_path, dirty } = &app.mode {
            let marker = if *dirty { " [*]" } else { "" };
            Some(format!("Editing: {:?}{}", target_path, marker))
        } else {
            None
        };
        vec![
            Line::from(vec![
                Span::styled("Edit", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::styled(
                    edit_info.map(|s| format!(" — {}", s)).unwrap_or_default(),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
            Line::from(vec![
                Span::styled("Enter:save+exit  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl+S:save  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl+J:nl", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Esc:cancel  ", Style::default().fg(Color::DarkGray)),
                Span::styled("PgUp/PgDn:scroll", Style::default().fg(Color::DarkGray)),
            ]),
        ]
    } else {
        let mode_label = match app.mode {
            AppMode::Select { .. } => "SELECT ",
            _ => "",
        };
        vec![
            Line::from(vec![
                Span::styled(
                    format!("{}Input", mode_label),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Tab:select  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl+F:import  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl+J:nl", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Enter:submit  ", Style::default().fg(Color::DarkGray)),
                Span::styled("PgUp/PgDn:scroll", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+↑/↓:history  ", Style::default().fg(Color::DarkGray)),
                Span::styled("End:scroll bottom", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Ctrl+R:rotate in  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Ctrl+O:rotate out", Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Alt+I:add input  ", Style::default().fg(Color::DarkGray)),
                Span::styled("Alt+O:add output", Style::default().fg(Color::DarkGray)),
            ]),
        ]
    };
    let instruction_count = instruction_lines.len() as u16;

    // In edit mode, always show the status line with the edit info
    // In normal mode, show transient status (e.g. "Active input: ...", "Saved: ...")
    let status_para = if let Some(ref msg) = app.status_message {
        Some(Paragraph::new(Line::from(Span::styled(
            format!(" {}", msg),
            Style::default().fg(Color::Yellow),
        ))))
    } else {
        None
    };
    let status_count = status_para.is_some() as u16;

    // Split the area: instruction header → optional status → bordered input box
    let input_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints({
            let mut cs = vec![Constraint::Length(instruction_count)];
            if status_count > 0 {
                cs.push(Constraint::Length(1));
            }
            cs.push(Constraint::Min(3));
            cs
        })
        .split(area);

    // Render instruction header
    let instructions = Paragraph::new(instruction_lines)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(instructions, input_chunks[0]);

    // Render status message if present
    let input_box_idx = if status_count > 0 {
        if let Some(ref sp) = status_para {
            f.render_widget(sp.clone(), input_chunks[1]);
        }
        2
    } else {
        1
    };

    // Render the input content with a border
    let input_text = app.input_text.clone();
    let input_para = if input_text.is_empty() {
        Paragraph::new(vec![Line::from(Span::styled(
            "...",
            Style::default().fg(Color::DarkGray),
        ))])
    } else {
        Paragraph::new(input_text)
    };

    let input_box = input_para
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded),
        );

    f.render_widget(input_box, input_chunks[input_box_idx]);

    // Cursor position (show cursor in Normal and EditFile modes)
    if matches!(app.mode, AppMode::Normal | AppMode::EditFile { .. }) {
        let (cursor_display_line, cursor_col) = wrapped_cursor(&app.input_text, app.input_cursor, inner_width);
        let cursor_x = input_chunks[input_box_idx].x + 1 + cursor_col as u16;
        let cursor_y = input_chunks[input_box_idx].y + 1 + cursor_display_line as u16;

        let clamped_x = cursor_x.min(input_chunks[input_box_idx].right().saturating_sub(1));
        let clamped_y = cursor_y.min(input_chunks[input_box_idx].bottom().saturating_sub(1));
        f.set_cursor_position((clamped_x, clamped_y));
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

fn render_add_dir(f: &mut ratatui::Frame, area: Rect, app: &mut App, is_input: bool) {
    let (buffer, cursor, completions, completion_index) = if is_input {
        match &app.mode {
            AppMode::AddInputDir { buffer, cursor, completions, completion_index } => {
                (buffer, *cursor, completions, *completion_index)
            }
            _ => return,
        }
    } else {
        match &app.mode {
            AppMode::AddOutputDir { buffer, cursor, completions, completion_index } => {
                (buffer, *cursor, completions, *completion_index)
            }
            _ => return,
        }
    };

    let title = if is_input {
        "Add input directory (Enter: add | Tab: cycle | Esc: cancel)"
    } else {
        "Add output directory (Enter: add | Tab: cycle | Esc: cancel)"
    };

    let input = Paragraph::new(buffer.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        )
        .style(Style::default().fg(Color::Yellow));

    f.render_widget(input, area);

    let cursor_x = area.x + 1 + cursor as u16;
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

            if i == completion_index {
                Line::from(Span::styled(
                    format!("▶ {}", truncated),
                    SELECTED_HIGHLIGHT,
                ))
            } else {
                Line::from(Span::raw(truncated))
            }
        }).take(5).collect();

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

    // Snapshot system_dirs from app
    let system_dirs = {
        let app_guard = app.lock().await;
        app_guard.system_dirs.clone()
    };

    // Main loop
    let result = run_main_loop(
        &mut terminal,
        app,
        on_submit_script,
        input_dirs,
        output_dirs,
        system_dirs,
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
    system_dirs: Vec<PathBuf>,
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
                                // Snapshot latest output for full context
                                let latest_output = app_guard.messages.iter()
                                    .rev()
                                    .find(|m| matches!(m.role, MessageRole::Output))
                                    .map(|m| m.filepath.clone());
                                drop(app_guard);
                                let context = ScriptletContext {
                                    latest_input_file: input_file.clone(),
                                    latest_output_file: latest_output,
                                    active_input_dir: active_input_dir.clone(),
                                    active_output_dir: active_output_dir.clone(),
                                    input_dirs: input_dirs.clone(),
                                    output_dirs: output_dirs.clone(),
                                    system_dirs: system_dirs.clone(),
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
