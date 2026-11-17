#!/bin/bash
# Build and install memory-watchdog: the daemon, its config, the notification app
# and the launchd agent. Safe to re-run; it keeps an existing config.
set -euo pipefail
cd "$(dirname "$0")"
LABEL=local.memory-watchdog
DOMAIN="gui/$(id -u)"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/memory-watchdog"
APP_SRC="target/Memory Watchdog.app"
APP="$HOME/Applications/Memory Watchdog.app"
NOTIFY="$APP/Contents/MacOS/memory-watchdog-notify"

echo "==> Building"
cargo build --release --quiet
./notifier/build-app.sh >/dev/null

echo "==> Installing"
mkdir -p "$HOME/.local/bin" "$HOME/Library/LaunchAgents" "$HOME/Library/Logs" "$HOME/Applications" "$CONFIG_DIR"
install -m 755 target/release/memory-watchdog "$HOME/.local/bin/memory-watchdog"

if [ -f "$CONFIG_DIR/config.toml" ]; then
  echo "    keeping existing $CONFIG_DIR/config.toml"
else
  cp config.toml "$CONFIG_DIR/config.toml"
  echo "    wrote $CONFIG_DIR/config.toml"
fi
"$HOME/.local/bin/memory-watchdog" check >/dev/null || { echo "config is invalid, fix it and re-run"; exit 1; }

# Replace the app only when it changed, so macOS keeps its notification permission.
if ! diff -rq "$APP_SRC" "$APP" >/dev/null 2>&1; then
  rm -rf "$APP"
  cp -R "$APP_SRC" "$APP"
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" || true
  echo "    installed $APP"
fi

if ! "$NOTIFY" --status >/dev/null 2>&1; then
  echo "==> Asking for notification permission (click Allow)"
  open -W -a "$APP" --args --authorize || true
  echo "    notifications: $("$NOTIFY" --status || true)"
fi

echo "==> Starting launchd agent"
sed "s#__HOME__#$HOME#g" "$LABEL.plist" > "$PLIST"
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
launchctl enable "$DOMAIN/$LABEL"
launchctl bootstrap "$DOMAIN" "$PLIST"
echo "Done. Log: ~/Library/Logs/memory-watchdog.log"
