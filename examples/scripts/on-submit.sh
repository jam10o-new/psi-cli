#!/bin/bash
# on-submit.sh - Execute after user submits input in psi-cli
# The user's message has already been written to $LATEST_INPUT_FILE
# Use this for extra logic like triggering agent turns or logging

echo "[psi-cli submit] Timestamp: $TIMESTAMP"
echo "[psi-cli submit] Input written to: $LATEST_INPUT_FILE"
echo "[psi-cli submit] User message: $USER_MESSAGE"

# Example: Trigger agent turn with volatile context
# ag run coder "$USER_MESSAGE"

# Example: Log the submission
# echo "[$TIMESTAMP] $USER_MESSAGE" >> /var/log/psi-cli.log
