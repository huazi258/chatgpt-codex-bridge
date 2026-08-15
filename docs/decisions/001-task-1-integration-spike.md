# 001 — Task 1 integration-spike result

- Status: Completed
- Date: 2026-08-15
- Decision: Do not begin Task 2 until both external integration paths are demonstrated on this machine.

## Evidence

### Codex App Server

The development shell resolves `codex` only to the executable bundled with the Microsoft Store Codex desktop package:

```text
C:\Program Files\WindowsApps\OpenAI.Codex_26.810.4967.0_x64__2p2nqsd0c76g0\app\resources\codex.exe
```

The Microsoft Store package executable initially failed with Windows `Access denied`. A standalone Codex CLI was then installed through npm at `D:\node\global`; it reports version `0.147.0`.

The test script `spikes/app-server-smoke.mjs` successfully used stdio App Server to initialize a connection, create thread `01a005e1-6b10-7740-b304-262a11a4a8ca`, start a harmless turn, and receive `turn/completed`. The agent returned the expected `APP_SERVER_SMOKE_OK` response. Startup emitted non-fatal plugin-icon and PowerShell shell-snapshot warnings.

Result: **Passed**.

### ChatGPT browser adapter

The browser-control runtime reports both Chrome and Edge unavailable. That runtime is not a product dependency: it would control the user's browser for this development session, whereas the product needs its own extension.

`spikes/chatgpt-extension` is a custom Manifest V3 extension that targets only `chatgpt.com`. It sends one harmless test prompt and waits for a newly-created assistant message to stabilize after generation ends. Its JavaScript and manifest parse successfully.

The first live run failed with `Could not establish connection. Receiving end does not exist.`, which isolated the failure to missing content-script injection in the already-open ChatGPT tab. The popup detects that error once, dynamically injects `content.js` with Chrome's `scripting` API, and retries the same message. This adds only the `scripting` permission; it does not broaden the extension's `chatgpt.com` host scope.

The second live run reached the composer but failed with `Illegal invocation` while treating `#prompt-textarea` as a native textarea. The extension now supports both native textarea and `contenteditable` composer elements. A later run could not find the send button, so its selector search was broadened and given a short wait for the UI to become ready.

The live test passed on 2026-08-15: the extension popup and the signed-in ChatGPT conversation both showed `CHATGPT_EXTENSION_SMOKE_OK`.

Result: **Passed**.

## Re-run steps

1. In Google Chrome, load `spikes/chatgpt-extension` through `chrome://extensions` with Developer mode enabled.
2. Open and sign in to a dedicated `chatgpt.com` conversation, then run the extension's harmless smoke test.
3. Record the browser version, extension permissions, and whether the documented selectors work. If it fails, capture the error shown in the extension popup.

## Rationale

The App Server is the correct integration surface because it exposes threads, turns, approvals, and streamed events to custom clients. The official documentation describes the stdio transport as the default local mode; the MVP must not work around this by automating the Codex desktop UI or exposing an unauthenticated network listener. [Codex App Server documentation](https://learn.chatgpt.com/docs/app-server)
