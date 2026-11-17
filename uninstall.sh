#!/bin/bash
# Stop and remove the memory-watchdog launchd agent.
set -uo pipefail
LABEL=local.memory-watchdog
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist" "$HOME/.local/bin/memory-watchdog"
echo "memory-watchdog removed."
