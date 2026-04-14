#!/bin/bash
# on-startup.sh - Execute when psi-cli starts
# Use this to spawn agentgraph agents or initialize environment

echo "[psi-cli startup] Timestamp: $TIMESTAMP"
echo "[psi-cli startup] Input directories: $INPUT_DIRS"
echo "[psi-cli startup] Output directories: $OUTPUT_DIRS"

# Example: Start an agentgraph agent
# ag spawn coder agents/coder --config config.yaml

# Example: Log startup event
# echo "psi-cli started at $TIMESTAMP" >> /var/log/psi-cli.log
