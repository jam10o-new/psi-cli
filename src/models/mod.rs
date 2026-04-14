use chrono::{DateTime, Local};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    Input,
    Output,
    System,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub filepath: PathBuf,
    pub created_at: DateTime<Local>,
    pub filename: String,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub path: PathBuf,
    pub created_at: DateTime<Local>,
    pub is_text: bool,
    pub content: Option<String>,
    pub metadata: FileMetadata,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub size: u64,
    pub extension: String,
    pub is_file: bool,
}

#[derive(Debug, Clone)]
pub struct ScriptletContext {
    pub latest_input_file: Option<PathBuf>,
    pub latest_output_file: Option<PathBuf>,
    pub active_input_dir: Option<PathBuf>,
    pub active_output_dir: Option<PathBuf>,
    pub input_dirs: Vec<PathBuf>,
    pub output_dirs: Vec<PathBuf>,
    pub system_dirs: Vec<PathBuf>,
    #[allow(dead_code)]
    pub timestamp: chrono::DateTime<Local>,
    pub user_message: Option<String>,
    pub agent_response: Option<String>,
}
