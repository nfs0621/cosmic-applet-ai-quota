#!/usr/bin/env bash
# Install the cross-desktop system-tray version of the AI Quota indicator.
# Works in any desktop with a StatusNotifierItem host: KDE Plasma, COSMIC,
# GNOME (with the AppIndicator extension), XFCE, etc. No sudo needed.
set -euo pipefail

cd "$(dirname "$0")"

APP_ID="dev.thorsteinson.AiQuotaTray"
BIN="ai-quota-tray"
BIN_DIR="$HOME/.local/bin"
APP_DIR="$HOME/.local/share/applications"
AUTOSTART_DIR="$HOME/.config/autostart"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

cargo build --release -p "$BIN"

install -Dm755 "target/release/$BIN" "$BIN_DIR/$BIN"
install -Dm644 "resources/dev.thorsteinson.AiQuotaApplet.svg" "$ICON_DIR/$APP_ID.svg"

# Generate the desktop entry with an absolute Exec path so it launches even if
# ~/.local/bin is not on the session PATH. Used both as a launcher entry and as
# an autostart entry.
desktop_entry() {
  cat <<EOF
[Desktop Entry]
Type=Application
Name=AI Quota (tray)
Comment=Claude, Gemini and OpenAI subscription quota in the system tray
Exec=$BIN_DIR/$BIN
Icon=$APP_ID
Terminal=false
Categories=Utility;
Keywords=AI;Quota;Claude;Gemini;OpenAI;Codex;
X-GNOME-Autostart-enabled=true
EOF
}

install -d "$APP_DIR" "$AUTOSTART_DIR"
desktop_entry > "$APP_DIR/$APP_ID.desktop"
desktop_entry > "$AUTOSTART_DIR/$APP_ID.desktop"

# Restart any running instance so the freshly built binary takes over.
pkill -x "$BIN" 2>/dev/null || true
sleep 1
( setsid "$BIN_DIR/$BIN" >/dev/null 2>&1 & ) || true

echo "Installed $BIN to $BIN_DIR and set it to autostart on login."
echo "It is now running in the system tray (click the icon for details)."
echo "Works in KDE Plasma, COSMIC, GNOME (AppIndicator ext), XFCE, etc."
