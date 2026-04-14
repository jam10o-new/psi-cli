use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "psi-cli", version, about = "Terminal UI for agentgraph agents", long_about = None)]
pub struct Cli {
    /// Input directories to watch (agent inputs)
    #[arg(short = 'i', long = "input", value_name = "DIR")]
    pub input_dirs: Vec<PathBuf>,

    /// Output directories to watch (agent outputs)
    #[arg(short = 'o', long = "output", value_name = "DIR")]
    pub output_dirs: Vec<PathBuf>,

    /// System prompt directories
    #[arg(short = 's', long = "system", value_name = "DIR")]
    pub system_dirs: Vec<PathBuf>,

    /// Script to run on startup
    #[arg(long = "on-startup", value_name = "SCRIPT")]
    pub on_startup: Option<PathBuf>,

    /// Script to run when user submits input
    #[arg(long = "on-submit", value_name = "SCRIPT")]
    pub on_submit: Option<PathBuf>,

    /// Script to run when agent output completes
    #[arg(long = "on-output", value_name = "SCRIPT")]
    pub on_output: Option<PathBuf>,

    /// Active output directory for writing user messages (defaults to first output dir)
    #[arg(short = 'a', long = "active-output", value_name = "DIR")]
    pub active_output: Option<PathBuf>,

    /// History limit for displayed messages (0 = all)
    #[arg(long = "history-limit", default_value = "0")]
    pub history_limit: usize,

    /// Use subcommand
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Quick start with an agentgraph agent directory structure
    Agent {
        /// Base agent directory (e.g., agents/coder)
        #[arg(value_name = "AGENT_DIR")]
        agent_dir: PathBuf,
    },
}
