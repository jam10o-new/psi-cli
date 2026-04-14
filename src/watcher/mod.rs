use crate::models::{FileEntry, FileMetadata};
use anyhow::Result;
use chrono::Local;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc as tokio_mpsc;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "yaml", "yml", "toml", "xml", "csv",
    "sh", "bash", "zsh", "py", "rs", "js", "ts", "css", "html",
    "log", "cfg", "conf", "ini", "env",
];

pub struct FsWatcher {
    watchers: Vec<PathBuf>,
    file_entries: Arc<Mutex<HashMap<PathBuf, FileEntry>>>,
    tx: tokio_mpsc::Sender<Vec<FileEntry>>,
}

impl FsWatcher {
    pub fn new(tx: tokio_mpsc::Sender<Vec<FileEntry>>) -> Self {
        Self {
            watchers: Vec::new(),
            file_entries: Arc::new(Mutex::new(HashMap::new())),
            tx,
        }
    }

    pub fn add_directory(&mut self, dir: PathBuf) {
        if !self.watchers.contains(&dir) {
            self.watchers.push(dir.clone());
        }
    }

    pub fn scan_all(&mut self) -> Result<()> {
        let dirs: Vec<PathBuf> = self.watchers.clone();
        for dir in &dirs {
            self.scan_directory_recursive(dir)?;
        }
        self.notify()?;
        Ok(())
    }

    fn scan_directory_recursive(&mut self, dir: &PathBuf) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                self.process_file(path);
            } else if path.is_dir() {
                self.scan_directory_recursive(&path)?;
            }
        }
        Ok(())
    }

    fn process_file(&mut self, path: PathBuf) {
        let entries = self.file_entries.clone();
        Self::process_file_internal(path, entries);
    }

    fn process_file_internal(path: PathBuf, entries: Arc<Mutex<HashMap<PathBuf, FileEntry>>>) {
        // Canonicalize the path so the same file is never duplicated under different representations
        let canonical = path.canonicalize().unwrap_or(path);

        if let Ok(metadata) = canonical.metadata() {
            let created = metadata.created().unwrap_or_else(|_| Local::now().into());
            let is_text = Self::is_text_file_path(&canonical);

            let content = if is_text {
                std::fs::read_to_string(&canonical).ok()
            } else {
                None
            };

            let file_entry = FileEntry {
                path: canonical.clone(),
                created_at: created.into(),
                is_text,
                content,
                metadata: FileMetadata {
                    size: metadata.len(),
                    extension: canonical
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    is_file: metadata.is_file(),
                },
            };

            if let Ok(mut map) = entries.lock() {
                map.insert(canonical, file_entry);
            }
        }
    }

    fn notify(&self) -> Result<()> {
        let entries = self.file_entries.lock().unwrap();
        let mut sorted: Vec<FileEntry> = entries.values().cloned().collect();
        sorted.sort_by_key(|e| e.created_at);
        drop(entries);

        let _ = self.tx.try_send(sorted);
        Ok(())
    }

    pub fn start_watching(
        &self,
    ) -> Result<notify::RecommendedWatcher> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(tx)?;
        let file_tx = self.tx.clone();
        let shared_entries = self.file_entries.clone();

        for dir in &self.watchers {
            if dir.exists() {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
        }

        // Spawn a thread to process file events and send updates
        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    Ok(Event { paths, kind, .. }) => {
                        // On create or modify: update display
                        if kind.is_create() || kind.is_modify() {
                            for path in paths {
                                if path.is_file() {
                                    Self::process_file_internal(
                                        path.clone(),
                                        shared_entries.clone(),
                                    );

                                    // Send full updated state for display
                                    let entries = shared_entries.lock().unwrap();
                                    let mut sorted: Vec<FileEntry> =
                                        entries.values().cloned().collect();
                                    sorted.sort_by_key(|e| e.created_at);
                                    drop(entries);

                                    let _ = file_tx.try_send(sorted);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("[psi-cli:watcher] Watch error: {:?}", e),
                }
            }
        });

        Ok(watcher)
    }

    /// Start watching for close-write events (AccessKind::Close(Write)).
    /// Sends the path of each file that was closed after writing.
    pub fn start_watching_close(
        &self,
        close_tx: tokio_mpsc::Sender<PathBuf>,
    ) -> Result<notify::RecommendedWatcher> {
        let (tx, rx) = channel();

        let mut watcher = notify::recommended_watcher(tx)?;

        for dir in &self.watchers {
            if dir.exists() {
                watcher.watch(dir, RecursiveMode::Recursive)?;
            }
        }

        std::thread::spawn(move || {
            while let Ok(event) = rx.recv() {
                match event {
                    Ok(Event { paths, kind, .. }) => {
                        // Only respond to close after write
                        if matches!(
                            kind,
                            EventKind::Access(notify::event::AccessKind::Close(
                                notify::event::AccessMode::Write
                            ))
                        ) {
                            for path in paths {
                                if path.is_file() {
                                    let _ = close_tx.try_send(path);
                                }
                            }
                        }
                    }
                    Err(e) => eprintln!("[psi-cli:watcher:close] Watch error: {:?}", e),
                }
            }
        });

        Ok(watcher)
    }

    fn is_text_file_path(path: &PathBuf) -> bool {
        if let Some(ext) = path.extension() {
            let ext_str = ext.to_string_lossy().to_lowercase();
            TEXT_EXTENSIONS.contains(&ext_str.as_str())
        } else {
            if let Ok(mut file) = std::fs::File::open(path) {
                use std::io::Read;
                let mut buffer = [0u8; 512];
                if let Ok(bytes_read) = file.read(&mut buffer) {
                    return !buffer[..bytes_read].contains(&0u8);
                }
            }
            false
        }
    }
}
