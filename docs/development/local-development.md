# Local development

## Prerequisites

- Node.js 22.11 or newer.
- Rust stable toolchain with Cargo available on `PATH`.
- Windows WebView2 Runtime (normally included with Windows 11 / Microsoft Edge).

The configured npm mirror on this machine does not serve the required `@tauri-apps/*` packages. Install dependencies through the public npm registry:

```powershell
npm install --registry=https://registry.npmjs.org
```

## Run and verify

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

The application creates its SQLite database in Tauri's Windows application-data directory. The Task 2 screen can create, update, reopen, and delete only `INACTIVE` modules; it performs no browser, Codex, Git, or repository action.

For the Task 3 smoke check, create an `INACTIVE` module using this repository directory, then leave the prefilled **Codex App Server 受控回合** prompt unchanged and select **运行受控 Codex 回合**. A successful result ends with `CODEX_ADAPTER_SMOKE_OK`; it must not inspect or change the repository. If `codex` is not on the desktop application's `PATH`, set `CODEX_APP_SERVER_COMMAND` to the full executable path before launching it.

For Task 4, reload the unpacked extension in `spikes/chatgpt-extension`, then follow its [pairing procedure](../../spikes/chatgpt-extension/README.md). Confirm that the desktop app reports `PAIRED`, then `VALID_PROTOCOL` after **发送协议引导**. The bridge uses only `127.0.0.1:8765` and rejects a bad pairing secret.

```powershell
npm run build
Set-Location src-tauri
cargo test
cargo check
```
