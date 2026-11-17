#!/bin/bash
# Install memory-watchdog as a launchd user agent and (re)start it.
set -euo pipefail
cd "$(dirname "$0")"
LABEL=local.memory-watchdog
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

mkdir -p "$HOME/.local/bin" "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
install -m 755 memory-watchdog "$HOME/.local/bin/memory-watchdog"
sed "s#__HOME__#$HOME#g" "$LABEL.plist" > "$PLIST"

launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl enable "$DOMAIN/$LABEL"
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "memory-watchdog installed and running. Log: ~/Library/Logs/memory-watchdog.log"
