# Gloss

Gloss is a Windows-only reading companion. Select English text in another application, press `Ctrl+Alt+Shift+T`, then choose **Triage** or **Translate**. It reads the selection through Windows UI Automation and never simulates `Ctrl+C` or reads the clipboard.

## Current design

- Native shell: Rust + Tauri 2
- Interface: React + TypeScript
- Selection: Windows UI Automation `TextPattern`
- AI transport: ChatGPT OAuth and the Codex Responses endpoint
- Model: `gpt-5.6-luna`
- Storage: one local JSONL file per conversation
- Credentials: local app-data JSON (plaintext)
- Network: optional HTTP, HTTPS, SOCKS5, or SOCKS5H proxy
- Learning profile: conservative CEFR estimates (A1–C2), updated only after a Triage follow-up conversation—not after every Triage

The fixed Triage response shape is `Quick read`, adaptive knowledge points, and `In plain English`. Follow-up questions stay in the same lightweight agent session.

## Prompt templates

Source defaults live in [`src-tauri/prompts`](src-tauri/prompts). On first launch, Gloss copies them to:

```text
%APPDATA%\com.gloss.desktop\prompts\
```

Gloss reloads these Markdown files for every request, so local edits take effect without rebuilding:

- `triage.md`
- `translate.md`
- `learner-profile.md`

Deleting one of the runtime files and restarting Gloss restores the bundled default. Bump `PROMPT_VERSION` in `src-tauri/src/sessions.rs` when a template change should be visible in new session metadata.

## Development

Requirements: Windows 10/11, Node.js, pnpm, and the Rust toolchain required by Tauri.

```powershell
pnpm install
pnpm tauri dev
```

Checks:

```powershell
pnpm build
cargo check --manifest-path src-tauri/Cargo.toml -j 1
```

Build an NSIS installer:

```powershell
pnpm tauri build
```

## Local data

Gloss stores settings, editable prompts, the CEFR profile, optional history, and `oauth.json` below its Tauri app-data directory. OAuth tokens never enter JSONL, but `oauth.json` stores them as plaintext. Requests use only the selected text plus messages from the active Gloss conversation. There is no telemetry or cloud sync.

The app remains in the Windows notification area when its card is closed. Use the tray menu to open settings or quit.

## Proxy

Open **Settings → Network** to configure a proxy. Host-and-port shorthand such as `127.0.0.1:23458` is accepted and normalized to `http://127.0.0.1:23458`. The proxy is used for OAuth token exchange and refresh, Responses requests, follow-ups, and background CEFR updates. The browser authorization page continues to use the browser's own network configuration, and the local OAuth callback never uses the proxy.

## Attribution

The Rust OAuth implementation is a direct port of the flow exposed by `oauth-cli-kit`. See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
