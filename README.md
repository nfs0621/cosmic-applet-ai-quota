# cosmic-applet-ai-quota

COSMIC panel applet for Pop!_OS showing how much of your AI coding
subscription quota is used — like the battery indicator, but for Claude,
OpenAI Codex and Gemini. Inspired by [quotio](https://github.com/nguyenphutrong/quotio)
(macOS, MIT), reimplemented natively for COSMIC in Rust.

## What it shows

- **Panel**: a gauge icon plus the worst provider's usage, e.g. `◢ 72%`.
  A `~` suffix means the value is stale (expired/revoked token).
- **Popup** (click): per provider, every quota window with a progress bar,
  percent used, and reset time. Providers without local credentials show
  "not configured".

| Provider | Credentials read | Panel metric |
|---|---|---|
| Claude (Claude Code) | `~/.claude/.credentials.json` | 5-hour session window |
| OpenAI Codex | `~/.codex/auth.json` | 5-hour (primary) window |
| Gemini CLI | `~/.gemini/oauth_creds.json` | worst quota bucket |

## Design notes

- **Read-only credentials.** The applet never writes or refreshes tokens.
  If a token is expired or revoked it shows the last known data marked
  stale; the value recovers automatically the next time you use that CLI
  (which refreshes its own token).
- Quota endpoints are the same undocumented ones quotio uses
  (`api.anthropic.com/api/oauth/usage`, `chatgpt.com/backend-api/wham/usage`,
  `cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota`). Parsers are
  tolerant: a shape change degrades to an error row, never a crash.
- Refresh every 120 s, plus a manual "Refresh now" button in the popup.
- Workspace split: `quota-providers` (pure parsing + fetch, unit-tested,
  no UI deps) and `applet` (libcosmic UI).

## Build & install

```bash
# prerequisites: rustup stable toolchain
./install.sh            # builds release + installs to ~/.local
pkill cosmic-panel      # panel respawns automatically
```

Then COSMIC Settings → Desktop → Panel → Configure panel applets → add
"AI Quota".

## Development

```bash
cargo test -p quota-providers           # parser + merge unit tests
cargo run -p quota-providers --example smoke   # live fetch, prints percentages only
cargo build --release -p cosmic-applet-ai-quota
```

Tokens are never logged or printed by any code path, including the smoke
example.

## License

MIT
