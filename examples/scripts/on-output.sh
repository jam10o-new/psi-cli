#!/bin/bash
# on-output.sh - Execute when agent produces output
# Use this for validation, logging, or triggering follow-up actions

echo "[psi-cli output] Timestamp: $TIMESTAMP"
echo "[psi-cli output] Latest output file: $LATEST_OUTPUT_FILE"

# Example: Read and display agent output
if [ -f "$LATEST_OUTPUT_FILE" ]; then
    echo "[psi-cli output] Content: $(cat "$LATEST_OUTPUT_FILE")"
fi

# Example: Validate output
# if grep -q "ERROR" "$LATEST_OUTPUT_FILE"; then
#     echo "Agent produced an error!" >> /var/log/psi-cli-errors.log
# fi

# Example: Trigger next agent in colony
# ag run researcher "Review the latest output and provide feedback"
