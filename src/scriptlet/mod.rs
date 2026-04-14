use crate::models::ScriptletContext;
use anyhow::Result;
use chrono::Local;
use std::path::PathBuf;
use std::process::Command;

pub struct ScriptletRunner;

impl ScriptletRunner {
    pub async fn execute_script(
        script_path: &PathBuf,
        context: &ScriptletContext,
    ) -> Result<(String, String)> {
        if !script_path.exists() {
            return Err(anyhow::anyhow!("Script not found: {:?}", script_path));
        }

        let mut cmd = Command::new("bash");
        cmd.arg(script_path);

        // Set environment variables
        cmd.env("TIMESTAMP", Local::now().to_rfc3339());
        cmd.env(
            "INPUT_DIRS",
            context
                .input_dirs
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        );
        cmd.env(
            "OUTPUT_DIRS",
            context
                .output_dirs
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        );
        cmd.env(
            "SYSTEM_DIRS",
            context
                .system_dirs
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(":"),
        );

        if let Some(ref input_file) = context.latest_input_file {
            cmd.env("LATEST_INPUT_FILE", input_file.to_string_lossy().to_string());
        }

        if let Some(ref output_file) = context.latest_output_file {
            cmd.env("LATEST_OUTPUT_FILE", output_file.to_string_lossy().to_string());
        }

        if let Some(ref active_input_dir) = context.active_input_dir {
            cmd.env("ACTIVE_INPUT_DIR", active_input_dir.to_string_lossy().to_string());
        }

        if let Some(ref active_output_dir) = context.active_output_dir {
            cmd.env("ACTIVE_OUTPUT_DIR", active_output_dir.to_string_lossy().to_string());
        }

        if let Some(ref user_message) = context.user_message {
            cmd.env("USER_MESSAGE", user_message);
        }

        if let Some(ref agent_response) = context.agent_response {
            cmd.env("AGENT_RESPONSE", agent_response);
        }

        // Execute the script
        let output = cmd.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((stdout, stderr))
    }

    pub async fn execute_on_startup(script_path: &PathBuf, context: &ScriptletContext) -> Result<()> {
        println!("[scriptlet] Running on-startup script: {:?}", script_path);
        let (stdout, stderr) = Self::execute_script(script_path, context).await?;
        if !stdout.is_empty() {
            println!("[scriptlet:stdout] {}", stdout);
        }
        if !stderr.is_empty() {
            eprintln!("[scriptlet:stderr] {}", stderr);
        }
        Ok(())
    }

    pub async fn execute_on_submit(script_path: &PathBuf, context: &ScriptletContext) -> Result<()> {
        println!("[scriptlet] Running on-submit script: {:?}", script_path);
        let (stdout, stderr) = Self::execute_script(script_path, context).await?;
        if !stdout.is_empty() {
            println!("[scriptlet:stdout] {}", stdout);
        }
        if !stderr.is_empty() {
            eprintln!("[scriptlet:stderr] {}", stderr);
        }
        Ok(())
    }

    pub async fn execute_on_output(script_path: &PathBuf, context: &ScriptletContext) -> Result<()> {
        println!("[scriptlet] Running on-output script: {:?}", script_path);
        let (stdout, stderr) = Self::execute_script(script_path, context).await?;
        if !stdout.is_empty() {
            println!("[scriptlet:stdout] {}", stdout);
        }
        if !stderr.is_empty() {
            eprintln!("[scriptlet:stderr] {}", stderr);
        }
        Ok(())
    }
}
