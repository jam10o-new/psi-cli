# psi-cli

An interactive terminal UI for filesystem-based workflows and automation. Watch directories in real-time, view file contents in a unified timeline, edit files in-place, import files from anywhere on the filesystem, and trigger scripted actions on filesystem events.

Designed for no-nonsense, keyboard-driven operation over SSH or tmux.

## Features

- **Real-time Directory Watching**: Monitor any number of directories for file creation and modification using filesystem events
- **Unified Timeline**: View all watched files in a single chronological interface, sorted by creation time
- **Role-Based Display**: Files from different directories are styled distinctly, making it easy to distinguish sources
- **Text & Binary Handling**: Text files display their content; non-text files show metadata (type, size)
- **In-Place File Editing**: Select any file in the timeline, edit it directly, and save changes back to disk
- **File Import**: Quickly copy files from anywhere on the filesystem into your active working directory, with tab-completion for paths
- **Scriptlet System**: Execute custom scripts on filesystem events — startup, user input submission, or file write completion
- **Scrollable Chat Log**: Mouse wheel, arrow keys, vim-style `j/k`, and `PageUp/PageDown` for navigation
- **Multiple Directory Rotation**: Cycle between multiple input/output directories with keyboard shortcuts

## Installation

```bash
cd psi-cli
cargo build --release
```

The binary is at `target/release/psi-cli`.

## Quick Start

### Watch Specific Directories

```bash
psi-cli --input ./inbox --output ./outbox --system ./config
```

### Agentgraph Integration

psi-cli was designed with [agentgraph](https://github.com/your-org/agentgraph) in mind — a filesystem-based multi-agent orchestration system where agents communicate via directory structures. The `agent` subcommand provides a quick shortcut:

```bash
psi-cli agent agents/coder
```

This automatically watches:
- `agents/coder/input/` — displayed as **USER INPUT** (cyan)
- `agents/coder/output/` — displayed as **AGENT OUTPUT** (green)
- `agents/coder/system/` — displayed as **SYSTEM** (yellow)

### With Scriptlets

```bash
psi-cli agent agents/coder \
  --on-startup scripts/startup.sh \
  --on-submit scripts/submit.sh \
  --on-output scripts/on-output.sh
```

## Keyboard Reference

### Normal Mode

| Shortcut | Action |
|----------|--------|
| `Enter` / `Ctrl+Enter` | Submit input |
| `Ctrl+J` | Insert newline |
| `Ctrl+↑` / `Ctrl+↓` | Navigate input history |
| `Tab` | Enter **Select mode** (navigate chat log) |
| `Ctrl+F` | Enter **Import mode** (copy a file) |
| `Ctrl+R` | Rotate active input directory |
| `Ctrl+O` | Rotate active output directory |
| `↑` / `↓` | Scroll chat log |
| `j` / `k` (empty input) | Scroll chat (vim-style) |
| `PageUp` / `PageDown` | Fast scroll (10 lines) |
| `Ctrl+C` | Quit |

### Select Mode (navigating chat log)

| Shortcut | Action |
|----------|--------|
| `↑` / `↓` / `j` / `k` | Navigate between messages |
| `PageUp` / `PageDown` | Fast navigation (10 messages) |
| `Enter` | Open selected file in **Edit mode** |
| `Esc` | Return to Normal mode |

### Edit Mode (editing a file in-place)

| Shortcut | Action |
|----------|--------|
| Type normally | Edit file content |
| `Ctrl+S` | Save changes to original file path on disk |
| `↑` / `↓` / `PageUp` / `PageDown` | Scroll view |
| `Esc` | Close without saving |

### Import Mode (copying a file)

| Shortcut | Action |
|----------|--------|
| Type a path | Enter file path (`~` expands to home) |
| `Tab` | Cycle through path completions |
| `Enter` | Copy file to active input directory |
| `Esc` | Cancel |

## Scriptlet System

Scriptlets are shell scripts executed at specific lifecycle events. They receive context through environment variables.

### Events

| Event | Trigger | When |
|-------|---------|------|
| `--on-startup` | psi-cli starts | Once, before the TUI launches |
| `--on-submit` | User submits input | Each time a message is entered and submitted (after the file is automatically written) |
| `--on-output` | A file is closed after writing | Each time a watched output file finishes being written (`AccessKind::Close(Write)`) |

### Default Behavior

When you submit a message (Enter / Ctrl+Enter) and an active input directory is set, psi-cli automatically writes the message as a timestamped file (`input-{timestamp_ms}.txt`) in the active input directory. No scriptlet required — it works out of the box.

The on-submit scriptlet, if configured, runs **after** this write and receives the path via `LATEST_INPUT_FILE` and `USER_MESSAGE`, allowing you to add extra logic like triggering an agent turn or logging.

### Available Environment Variables

| Variable | Description | Available In |
|----------|-------------|--------------|
| `TIMESTAMP` | ISO 8601 timestamp | All scriptlets |
| `INPUT_DIRS` | Colon-separated list of watched input directories | All scriptlets |
| `OUTPUT_DIRS` | Colon-separated list of watched output directories | All scriptlets |
| `ACTIVE_INPUT_DIR` | Currently active input directory | All scriptlets |
| `ACTIVE_OUTPUT_DIR` | Currently active output directory | All scriptlets |
| `LATEST_INPUT_FILE` | Path to the most recent input file | on-submit, on-output |
| `LATEST_OUTPUT_FILE` | Path to the most recent output file | on-output |
| `USER_MESSAGE` | Text the user submitted | on-submit |
| `AGENT_RESPONSE` | Agent output text | on-output |

### Example: On-Startup

```bash
#!/bin/bash
# Spawn an agentgraph agent when psi-cli starts
ag spawn coder agents/coder --config config.yaml
```

### Example: On-Submit

```bash
#!/bin/bash
# Trigger agent turn after user input is written
# (The file has already been written to ACTIVE_INPUT_DIR by psi-cli)
echo "[$TIMESTAMP] User submitted: $LATEST_INPUT_FILE"
ag run coder "$USER_MESSAGE"
```

### Example: On-Output

```bash
#!/bin/bash
# Validate or log agent output
echo "[$TIMESTAMP] Agent produced: $LATEST_OUTPUT_FILE"
if grep -q "ERROR" "$LATEST_OUTPUT_FILE"; then
    echo "Error in output!" >> /var/log/agent-errors.log
fi
```

## Architecture

```
psi-cli
├── CLI (clap)            → Parse args, setup directories
├── FsWatcher (notify)    → Recursive directory watching
│   ├── Display watcher   → Create/modify events → timeline updates
│   └── Close watcher     → Close(Write) events  → on-output scriptlet
├── TUI (ratatui)         → Multi-mode interface: normal, select, edit, import
├── Scriptlet Runner      → Execute shell scripts with env context
└── Path Completion       → Glob-based path completion for file import
```

### How It Works

1. **Directory Scanning**: On startup, all watched directories are scanned recursively and files are loaded into the timeline
2. **Real-time Watching**: The `notify` crate detects file creation/modification events; the timeline updates automatically
3. **Close-Write Detection**: A separate watcher listens for `AccessKind::Close(Write)` events on output directories, triggering the on-output scriptlet only when a file is fully written (important for streaming output)
4. **Role Classification**: Files are classified as input/output/system based on which watched directory they reside in
5. **User Input**: Messages typed in the input box appear in the timeline; on-submit scriptlets handle writing to the filesystem
6. **File Editing**: Any file in the timeline can be selected, edited, and saved back to its original path
7. **File Import**: Files from anywhere on the filesystem can be quickly copied into the active input directory

## Use Cases

- **Agent Orchestration**: Monitor and interact with filesystem-based agent systems
- **Log Monitoring**: Watch log directories for new entries in a readable format
- **Drop-Box Workflows**: Monitor input directories, review incoming files, respond via scriptlets
- **Collaborative File Review**: View a timeline of files from multiple sources, edit in-place
- **Remote Operations**: Run over SSH via tmux for reliable remote filesystem management
