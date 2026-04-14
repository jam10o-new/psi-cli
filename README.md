# psi-cli

An interactive terminal UI for filesystem-based workflows and automation. Watch directories in real-time, view file contents in a unified timeline, edit files in-place, and trigger scripted actions on filesystem events.

Designed for no-nonsense, keyboard-driven operation over SSH or tmux.

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

If no input directories are specified, psi-cli defaults to the current working directory (`./`) as the input source.

### Agentgraph Integration

psi-cli was designed with [agentgraph](https://github.com/jam10o-new/agentgraph) in mind. The `agent` subcommand sets up a standard directory structure:

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
| `Enter` | Submit input |
| `Ctrl+J` | Insert newline |
| `Ctrl+↑` / `Ctrl+↓` | Navigate input history |
| `Tab` | Enter **Select mode** |
| `Ctrl+F` | Enter **Import mode** |
| `Ctrl+R` / `Ctrl+O` | Rotate active input / output directory |
| `Alt+I` / `Alt+O` | Add input / output directory |
| `PgUp` / `PgDn` | Scroll chat log (10 lines) |
| `End` | Scroll to bottom of chat |
| `↑` / `↓` / `←` / `→` | Move cursor in input box |
| `Ctrl+C` | Quit |

### Select Mode (navigating chat log)

| Shortcut | Action |
|----------|--------|
| `↑` / `↓` / `j` / `k` | Navigate between messages |
| `PageUp` / `PageDown` | Fast navigation (10 messages) |
| `Enter` | Open selected file in **Edit mode** |
| `Ctrl+R` | Add input directory |
| `Ctrl+O` | Add output directory |
| `Ctrl+D` | Delete selected file from disk |
| `Esc` | Return to Normal mode |

### Edit Mode (editing a file in-place)

Edit mode reuses the normal input box — same navigation, word wrapping, and newline support:

| Shortcut | Action |
|----------|--------|
| Type normally | Edit file content |
| `Enter` | Save and exit to Normal mode |
| `Ctrl+S` | Save in-place, stay in Edit mode |
| `Ctrl+J` | Insert newline |
| `Esc` | Cancel (discard changes) |

### Import / Add Directory Mode

| Shortcut | Action |
|----------|--------|
| Type a path | Enter file or directory path (`~` expands to home) |
| `Tab` | Cycle through completions |
| `Enter` | Confirm |
| `Esc` | Cancel |

## Scriptlet System

Scriptlets are shell scripts executed at specific lifecycle events. They receive context through environment variables. All scriptlets receive the same full set of variables.

### Events

| Event | Trigger |
|-------|---------|
| `--on-startup` | psi-cli starts (once, before TUI) |
| `--on-submit` | User submits input (after auto-write) |
| `--on-output` | A file is closed after writing in an output directory |

### Available Environment Variables

| Variable | Description |
|----------|-------------|
| `TIMESTAMP` | ISO 8601 timestamp |
| `INPUT_DIRS` | All watched input directories (colon-separated) |
| `OUTPUT_DIRS` | All watched output directories (colon-separated) |
| `SYSTEM_DIRS` | All watched system directories (colon-separated) |
| `ACTIVE_INPUT_DIR` | Currently active input directory |
| `ACTIVE_OUTPUT_DIR` | Currently active output directory |
| `LATEST_INPUT_FILE` | Path to the most recent input file |
| `LATEST_OUTPUT_FILE` | Path to the most recent output file |
| `USER_MESSAGE` | Text the user submitted |
| `AGENT_RESPONSE` | Latest agent output text |

### Example: On-Submit

```bash
#!/bin/bash
echo "[$TIMESTAMP] User submitted: $USER_MESSAGE"
echo "  File: $LATEST_INPUT_FILE"
ag run coder "$USER_MESSAGE"
```

### Example: On-Output

```bash
#!/bin/bash
echo "[$TIMESTAMP] Agent produced: $LATEST_OUTPUT_FILE"
if grep -q "ERROR" "$LATEST_OUTPUT_FILE"; then
    echo "Error in output!" >> /var/log/agent-errors.log
fi
```
