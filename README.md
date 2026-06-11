# ai-quota-indicator

How much of your AI coding subscription quota is used, shown right in your
desktop — like the battery indicator, but for Claude, OpenAI Codex and
Gemini. Inspired by [quotio](https://github.com/nguyenphutrong/quotio)
(macOS, MIT), reimplemented natively in Rust.

Two frontends share one quota engine:

- **COSMIC panel applet** — a gauge icon plus the worst provider's usage in
  the COSMIC panel, with a click-through popup.
- **System tray app** — a StatusNotifierItem icon that works in any desktop
  with a tray host: KDE Plasma, GNOME (AppIndicator), XFCE, COSMIC, …

## What it shows

- The worst provider's usage at a glance, e.g. `72%` (a stale marker means
  the value is last-known: the token expired or was revoked).
- On click: per provider, every quota window with a progress bar, percent
  used, and reset time. Providers without local credentials show
  "not configured".

| Provider | Credentials read | Metric |
|---|---|---|
| Claude (Claude Code) | `~/.claude/.credentials.json` | 5-hour session window |
| OpenAI Codex | `~/.codex/auth.json` | 5-hour (primary) window |
| Gemini CLI | `~/.gemini/oauth_creds.json` | worst quota bucket |

## Install

Prerequisite: a `rustup` stable toolchain.

### COSMIC panel applet

```bash
./install.sh            # builds release + installs to ~/.local
pkill cosmic-panel      # panel respawns automatically
```

Then COSMIC Settings → Desktop → Panel → Configure panel applets → add
"AI Quota".

### System tray (KDE Plasma, GNOME, XFCE, …)

```bash
./install-tray.sh       # builds release + installs to ~/.local + autostart
```

This installs `ai-quota-tray` to `~/.local/bin`, adds a `~/.config/autostart`
entry so it starts on login, and launches it immediately. It works anywhere
there is an SNI host: KDE Plasma (built in), COSMIC, GNOME (with the
AppIndicator/KStatusNotifierItem extension), XFCE, etc.

- **Tray icon**: a rounded square colour-coded by the worst provider's usage
  (green < 50 %, amber < 80 %, red ≥ 80 %, dimmed when stale) with the
  percentage drawn in the middle.
- **Left click / menu**: per-provider rows with a text progress bar, percent,
  and reset time, plus "Refresh now" and "Quit". 120 s auto-refresh.
- **Tooltip**: worst-usage summary and a one-line-per-provider breakdown.

> In KDE Plasma, new tray items may start in the collapsed "hidden" area —
> click the tray's up-arrow, or System Tray Settings → Entries → set
> "AI Quota" to **Shown** to pin it.

## Design notes

- **Read-only credentials.** Tokens are never written or refreshed. An
  expired/revoked token shows the last known data marked stale; it recovers
  automatically the next time you use that CLI (which refreshes its own
  token).
- Quota endpoints are the same undocumented ones quotio uses
  (`api.anthropic.com/api/oauth/usage`, `chatgpt.com/backend-api/wham/usage`,
  `cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`). Parsers are
  tolerant: a shape change degrades to an error row, never a crash.
- Workspace split: `quota-providers` (pure parsing + fetch, unit-tested, no
  UI deps), `applet` (libcosmic panel UI), and `tray` (ksni StatusNotifierItem
  UI). Both frontends reuse the same core, so the numbers always agree.

## Development

```bash
cargo test -p quota-providers                     # parser + merge unit tests
cargo run -p quota-providers --example smoke      # live fetch, percentages only
cargo build --release -p cosmic-applet-ai-quota   # COSMIC applet
cargo build --release -p ai-quota-tray            # system tray
```

Tokens are never logged or printed by any code path, including the smoke
example.

[StatusNotifierItem]: https://www.freedesktop.org/wiki/Specifications/StatusNotifierItem/

## License

MIT
