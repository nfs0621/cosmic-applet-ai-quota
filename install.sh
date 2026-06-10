#!/usr/bin/env bash
# Install the AI Quota applet for the current user (no sudo needed).
set -euo pipefail

cd "$(dirname "$0")"

APP_ID="dev.thorsteinson.AiQuotaApplet"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

cargo build --release -p cosmic-applet-ai-quota

install -Dm755 "target/release/cosmic-applet-ai-quota" "$BIN_DIR/cosmic-applet-ai-quota"
install -Dm644 "resources/$APP_ID.desktop" "$APP_DIR/$APP_ID.desktop"
install -Dm644 "resources/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"

echo "Installed. Restart the panel (pkill cosmic-panel) and add the applet via"
echo "COSMIC Settings -> Desktop -> Panel -> Configure panel applets."
