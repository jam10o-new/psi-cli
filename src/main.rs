mod cli;
mod models;
mod scriptlet;
mod tui;
mod watcher;

use anyhow::Result;
use chrono::Local;
use cli::Cli;
use clap::Parser;
use models::ScriptletContext;
use scriptlet::ScriptletRunner;
use std::sync::Arc;
use tokio::sync::Mutex;
use tui::App;
use watcher::FsWatcher;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize app
    let app = Arc::new(Mutex::new(App::new()));

    // Setup directories and scriptlet paths
    {
        let mut app_guard = app.lock().await;
        app_guard.input_dirs = cli.input_dirs.clone();
        app_guard.output_dirs = cli.output_dirs.clone();
        app_guard.system_dirs = cli.system_dirs.clone();
        app_guard.active_input_dir = cli.input_dirs.first().cloned();
        app_guard.active_output_dir = cli.active_output.or_else(|| {
            cli.output_dirs.first().cloned()
        });
        app_guard.on_submit_script = cli.on_submit.clone();
        app_guard.on_output_script = cli.on_output.clone();
    }

    // Handle "agent" subcommand
    if let Some(cli::Commands::Agent { agent_dir }) = &cli.command {
        let mut app_guard = app.lock().await;
        let input_dir = agent_dir.join("input");
        let output_dir = agent_dir.join("output");
        let system_dir = agent_dir.join("system");
        if input_dir.exists() {
            app_guard.input_dirs.push(input_dir.clone());
            app_guard.active_input_dir = Some(input_dir);
        }
        if output_dir.exists() {
            app_guard.output_dirs.push(output_dir.clone());
            app_guard.active_output_dir = Some(output_dir);
        }
        if system_dir.exists() {
            app_guard.system_dirs.push(system_dir);
        }
    }

    // Snapshot config for scriptlet contexts
    let app_guard = app.lock().await;
    let input_dirs = app_guard.input_dirs.clone();
    let output_dirs = app_guard.output_dirs.clone();
    let active_input = app_guard.active_input_dir.clone();
    let active_output = app_guard.active_output_dir.clone();
    let on_submit = app_guard.on_submit_script.clone();
    let on_output = app_guard.on_output_script.clone();
    drop(app_guard);

    // For the on-output handler (needs its own clones for the async closure)
    let on_output_clone = on_output.clone();
    let input_dirs_clone = input_dirs.clone();
    let output_dirs_clone = output_dirs.clone();
    let active_input_clone = active_input.clone();
    let active_output_clone = active_output.clone();

    // Setup display watcher
    let (file_tx, mut file_rx) = tokio::sync::mpsc::channel(100);
    let mut fs_watcher = FsWatcher::new(file_tx);
    {
        let app_guard = app.lock().await;
        for dir in &app_guard.input_dirs {
            fs_watcher.add_directory(dir.clone());
        }
        for dir in &app_guard.output_dirs {
            fs_watcher.add_directory(dir.clone());
        }
        for dir in &app_guard.system_dirs {
            fs_watcher.add_directory(dir.clone());
        }
    }
    fs_watcher.scan_all()?;
    let _display_watcher = fs_watcher.start_watching()?;

    // Spawn file display updater
    let app_clone = Arc::clone(&app);
    tokio::spawn(async move {
        while let Some(entries) = file_rx.recv().await {
            let mut app_guard = app_clone.lock().await;
            app_guard.update_messages(entries);
        }
    });

    // Setup close-write watcher for on-output scriptlet
    let (close_tx, mut close_rx) = tokio::sync::mpsc::channel(100);
    let (close_dummy_tx, _) = tokio::sync::mpsc::channel(1);
    let mut close_fs_watcher = FsWatcher::new(close_dummy_tx);
    for dir in &output_dirs {
        close_fs_watcher.add_directory(dir.clone());
    }
    let _close_watcher = close_fs_watcher.start_watching_close(close_tx)?;

    // Spawn on-output handler
    tokio::spawn(async move {
        while let Some(file_path) = close_rx.recv().await {
            if let Some(ref script) = on_output_clone {
                let context = ScriptletContext {
                    latest_input_file: None,
                    latest_output_file: Some(file_path.clone()),
                    active_input_dir: active_input_clone.clone(),
                    active_output_dir: active_output_clone.clone(),
                    input_dirs: input_dirs_clone.clone(),
                    output_dirs: output_dirs_clone.clone(),
                    timestamp: Local::now(),
                    user_message: None,
                    agent_response: None,
                };
                let _ = ScriptletRunner::execute_on_output(script, &context).await;
            }
        }
    });

    // Run startup scriptlet
    if let Some(ref script) = cli.on_startup {
        let context = ScriptletContext {
            latest_input_file: None,
            latest_output_file: None,
            active_input_dir: active_input.clone(),
            active_output_dir: active_output.clone(),
            input_dirs: input_dirs.clone(),
            output_dirs: output_dirs.clone(),
            timestamp: Local::now(),
            user_message: None,
            agent_response: None,
        };
        let _ = ScriptletRunner::execute_on_startup(script, &context).await;
    }

    // Run TUI with submit handler
    tui::run_tui(
        app,
        on_submit,
        input_dirs,
        output_dirs,
        active_input,
        active_output,
    )
    .await?;

    Ok(())
}
