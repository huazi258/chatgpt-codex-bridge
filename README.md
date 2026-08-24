# ChatGPT Codex Bridge

**ChatGPT thinks. Codex builds. Bridge connects them.**

ChatGPT Codex Bridge is a local desktop relay that connects one ChatGPT conversation with one Codex thread for a controlled coding workflow.

ChatGPT is responsible for planning, reviewing, and deciding what Codex should do next. Codex works in the selected local project directory. Bridge transports messages between them, keeps the relay state visible, applies runtime and cycle budgets, and pauses when user intervention is required.

> This project is an independent open-source project and is not affiliated with or endorsed by OpenAI.

## What problem does it solve?

When ChatGPT is used for engineering planning and Codex is used for repository execution, the user normally has to copy prompts and results back and forth manually.

Bridge turns that hand-off into a visible, stateful relay:

```text
You
 │
 ▼
ChatGPT ── planning / review
 │
 │  CODEX_PROMPT
 ▼
Bridge
 │
 ▼
Codex ── reads and changes the selected project
 │
 │  final result
 ▼
Bridge
 │
 ▼
ChatGPT ── review / next step
 │
 ├─ CODEX_PROMPT → continue
 ├─ BLOCKED      → ask the user
 └─ MODULE_DONE  → wait for user acceptance
```

Bridge does **not** decide how to implement a feature, manage Git policy, invent prompts, or silently repair failed work. Its job is reliable local message relay, state, recovery, and user-visible control.

## Current workflow

A Bridge **module** is one user-visible work phase bound to:

- one ChatGPT conversation;
- one Codex working directory;
- one new or explicitly resumed Codex thread;
- a maximum relay-cycle budget;
- a maximum runtime budget.

A module may contain many ChatGPT ↔ Codex cycles. It is not the same thing as one Codex request.

The resident ChatGPT view supports two message types:

- **Manual chat** — the reply is displayed only and never parsed as an automation command.
- **Automation request** — only the matching reply may drive the relay protocol.

## Requirements

The current workflow assumes that the repository is run locally by a developer.

- Windows 11 or another Windows environment with WebView2 available.
- Node.js 22.11 or newer.
- Rust stable toolchain with Cargo on `PATH`.
- Codex available locally and usable for the target project directory.
- Google Chrome.
- A signed-in ChatGPT session on `https://chatgpt.com`.

## Quick start

### 1. Clone the repository

```powershell
git clone https://github.com/huazi258/chatgpt-codex-bridge.git
cd chatgpt-codex-bridge
```

### 2. Install dependencies

```powershell
npm install --registry=https://registry.npmjs.org
```

### 3. Start the desktop app

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

The desktop app should open a Chinese relay workspace and show a local ChatGPT pairing endpoint and one-time pairing secret.

### 4. Load the Chrome extension

1. Open `chrome://extensions`.
2. Enable **Developer mode**.
3. Choose **Load unpacked**.
4. Select `spikes/chatgpt-extension` from this repository.
5. Open or refresh the ChatGPT conversation you want Bridge to use.

### 5. Pair the ChatGPT conversation

1. Keep the desktop app running.
2. Open the extension popup in the target ChatGPT tab.
3. Paste the one-time pairing secret shown by Bridge.
4. Pair the active ChatGPT tab.
5. Confirm that Bridge shows **ChatGPT 已连接**.

The pairing secret is regenerated when the desktop app restarts.

### 6. Create a module

In Bridge:

1. Choose **新建模块**.
2. Enter a module name.
3. Enter the Codex working directory.
4. Choose either **新建 Codex 对话** or **继续现有 Codex 对话**.
5. Set the maximum automatic cycles and runtime if needed.
6. Create the module.

For a new Codex conversation, the thread is created lazily when the first valid Codex prompt arrives. For an existing conversation, Bridge first lists eligible threads for the selected working directory and resumes the chosen thread only when the first valid prompt arrives.

### 7. Start the relay

Use **发送自动化请求** when you want ChatGPT's next reply to participate in the automation protocol.

A valid automation reply ends with exactly one supported control block:

```text
@@@CODEX_PROMPT@@@
<verbatim prompt for Codex>
@@@END_CODEX_PROMPT@@@
```

or:

```text
@@@MODULE_DONE@@@
```

or:

```text
@@@BLOCKED@@@
<reason and the information or decision needed from the user>
@@@END_BLOCKED@@@
```

Bridge sends a valid `CODEX_PROMPT` body to Codex unchanged. The Codex result is then queued back to ChatGPT, where ChatGPT can review it and choose the next step.

For a complete walkthrough, see [Getting Started](docs/getting-started.md).

## Important concepts

### Manual chat vs automation request

Use **手动聊天** for ordinary conversation with ChatGPT. Even if the reply contains text that looks like a control block, Bridge does not treat it as automation.

Use **发送自动化请求** only when the reply should be eligible to trigger Codex or another control action.

### `MODULE_DONE`

`MODULE_DONE` does not silently finish the module. Bridge enters an acceptance state and waits for the user to either accept the module, send feedback back to ChatGPT and continue, or terminate the module.

### `BLOCKED`

`BLOCKED` means the workflow needs a human decision or missing information. Bridge stops automatic progress and presents the reason to the user.

### `UNKNOWN` delivery

If a restart or connection problem leaves an outbound message with an uncertain delivery result, Bridge does **not** resend it automatically.

The user must explicitly choose either to resend the message or continue without resending it. This prevents duplicate side effects caused by guessing whether a message was delivered.

## Documentation

If you are new to the project, read these in order:

- [Getting Started](docs/getting-started.md) — clone to first successful relay cycle.
- [User Guide](docs/user-guide.md) — modules, messages, threads, states, acceptance, and recovery.
- [Troubleshooting](docs/troubleshooting.md) — common startup, pairing, Codex, and delivery problems.
- [Documentation Index](docs/README.md) — user, contributor, and maintainer documentation map.

For contributors and maintainers:

- [Local development](docs/development/local-development.md)
- [Conversation Relay V2 decision](docs/decisions/004-conversation-relay-v2.md)
- [Architecture](docs/architecture/)
- [Protocols](docs/protocols/)
- [Reliability](docs/reliability/)
- [Security](docs/security/)

## Development checks

```powershell
npm run build
npm test -- --run
Set-Location src-tauri
cargo test
cargo check
```

## Current limitations

The project currently depends on a local Chrome extension that targets `https://chatgpt.com/*` and on ChatGPT DOM selectors that may change over time.

The relay is intentionally conservative around uncertain delivery and Codex thread ownership. It will stop for explicit recovery instead of guessing or automatically retrying operations that may have side effects.

See [Troubleshooting](docs/troubleshooting.md) for recovery guidance and the active V2 decision record for the precise product contract.
