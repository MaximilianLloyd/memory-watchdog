#!/bin/bash
# Stop and remove memory-watchdog. Leaves your config in ~/.config/memory-watchdog.
set -uo pipefail
LABEL=local.memory-watchdog
launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null
rm -f "$HOME/Library/LaunchAgents/$LABEL.plist" "$HOME/.local/bin/memory-watchdog"
rm -rf "$HOME/Applications/Memory Watchdog.app"
echo "memory-watchdog removed. Config left in ${XDG_CONFIG_HOME:-$HOME/.config}/memory-watchdog."
