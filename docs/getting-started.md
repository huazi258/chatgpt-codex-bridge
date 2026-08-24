# Getting Started

This guide takes you from a fresh clone of ChatGPT Codex Bridge to the first successful ChatGPT → Codex → ChatGPT relay cycle.

If you already have Bridge running and want to understand the interface and states, read the [User Guide](user-guide.md).

## 1. Prepare the environment

You need:

- Windows with WebView2 available;
- Node.js 22.11 or newer;
- the Rust stable toolchain and Cargo;
- a local Codex installation that can work in the target project;
- Google Chrome;
- a signed-in ChatGPT session.

Verify the main command-line dependencies:

```powershell
node --version
npm --version
cargo --version
codex --version
```

If `cargo` is installed but not found by the desktop process, make sure `%USERPROFILE%\.cargo\bin` is on `PATH`.

## 2. Clone Bridge

```powershell
git clone https://github.com/huazi258/chatgpt-codex-bridge.git
cd chatgpt-codex-bridge
```

## 3. Install JavaScript dependencies

```powershell
npm install --registry=https://registry.npmjs.org
```

The explicit public registry is used because some configured npm mirrors do not serve all required `@tauri-apps/*` packages.

## 4. Start the desktop app

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

A successful startup opens the desktop relay workspace.

At the top of the workspace you should see the **ChatGPT 浏览器连接** card with:

- a local endpoint;
- a one-time pairing secret;
- a connection status.

Do not create a module yet if ChatGPT is still disconnected.

## 5. Load the ChatGPT Chrome extension

Bridge currently uses the unpacked extension in `spikes/chatgpt-extension`.

1. Open Chrome.
2. Navigate to `chrome://extensions`.
3. Enable **Developer mode**.
4. Click **Load unpacked**.
5. Select the repository folder `spikes/chatgpt-extension`.
6. Open a signed-in `https://chatgpt.com` conversation.
7. Refresh that ChatGPT tab after loading or reloading the extension.

The extension only targets `https://chatgpt.com/*` and the local Bridge endpoint.

## 6. Pair one ChatGPT conversation

In the Bridge desktop app, copy the one-time pairing secret.

Then:

1. switch to the ChatGPT conversation that should be controlled by this Bridge session;
2. open the extension popup;
3. paste the pairing secret;
4. pair the active ChatGPT tab;
5. return to Bridge.

The sidebar should change to **ChatGPT 已连接**.

A pairing secret is session-scoped. Restarting the desktop app invalidates the old secret, so pair again after a restart.

## 7. Prepare the project that Codex will work in

Bridge does not choose or manage your repository structure for you. The **Codex 工作目录** is the local directory Codex will receive as its working environment.

Before creating a module, make sure:

- the directory exists;
- Codex can open and work in it;
- any project instructions such as `AGENTS.md` are already present;
- the repository is in the Git state you expect;
- secrets or credentials are not being placed into Bridge prompts unnecessarily.

Codex discovers project-level instructions from the selected working directory.

## 8. Create a module

Choose **新建模块**.

Fill in:

### 模块名称

A human-readable name for this phase of work, for example:

```text
README user onboarding
```

### Codex 工作目录

Enter the absolute path of the project Codex should work in.

### Codex 对话

Choose one of two modes.

**新建 Codex 对话**

Bridge does not create the thread immediately. The first valid `CODEX_PROMPT` starts the local App Server and creates the thread lazily.

**继续现有 Codex 对话**

1. select this mode;
2. click **刷新对话**;
3. choose one eligible thread for the selected working directory;
4. create the module.

Bridge stores the selection when the module is created. It attempts to resume the thread only when the first valid Codex prompt arrives.

Threads that are already active elsewhere are not taken over automatically.

### Budgets

The defaults shown by the current UI are:

- maximum automatic cycles: `12`;
- maximum runtime: `240` minutes.

A cycle is consumed only when a Codex turn actually starts.

## 9. Establish the ChatGPT control convention

Bridge automation is deliberately explicit.

Only a reply to a message sent using **发送自动化请求** can be parsed as an automation instruction.

A valid reply must end with exactly one supported control block.

### Send work to Codex

```text
@@@CODEX_PROMPT@@@
<the exact prompt Codex should receive>
@@@END_CODEX_PROMPT@@@
```

### Ask the user to accept the module

```text
@@@MODULE_DONE@@@
```

### Ask for human intervention

```text
@@@BLOCKED@@@
<reason and the information or decision needed from the user>
@@@END_BLOCKED@@@
```

The control block must be the terminal non-whitespace content of the automation reply.

## 10. Send the first automation request

In the selected module:

1. choose **发送自动化请求**;
2. send a clear instruction to ChatGPT that asks it to analyze the current goal and either produce the next Codex prompt, request user input with `BLOCKED`, or finish with `MODULE_DONE`.

Example intent:

```text
请根据当前项目上下文规划下一步工作。
如果需要 Codex 执行，请在回复末尾给出一个有效的 CODEX_PROMPT 控制块；
如果需要我决定，请使用 BLOCKED；
如果本模块已经可以验收，请使用 MODULE_DONE。
```

The exact engineering instructions are up to you and ChatGPT. Bridge does not invent them.

## 11. Observe the first relay cycle

When ChatGPT returns a valid `CODEX_PROMPT`:

1. Bridge persists a pending Codex cycle;
2. Bridge starts or resumes the configured Codex thread;
3. once the Codex turn starts, the module cycle counter advances;
4. Codex executes in the selected working directory;
5. the final Codex text appears in Bridge;
6. that result is queued back to the resident ChatGPT conversation;
7. ChatGPT can review it and decide the next step.

A module can repeat this many times until a budget is reached, the workflow is blocked, the user terminates it, or ChatGPT returns `MODULE_DONE`.

## 12. Know what success looks like

For a first successful smoke workflow, you should be able to observe:

- **ChatGPT 已连接**;
- a created module;
- one automation request sent to ChatGPT;
- a valid `CODEX_PROMPT`;
- a Codex cycle that progresses through execution;
- the Codex final result returning to ChatGPT;
- ChatGPT producing either another Codex prompt, `BLOCKED`, or `MODULE_DONE`.

At `MODULE_DONE`, Bridge must wait for your explicit acceptance. It does not silently mark the work complete.

## Next steps

- Read the [User Guide](user-guide.md) for the meaning of UI states and recovery actions.
- Read [Troubleshooting](troubleshooting.md) when pairing, Codex startup, delivery, or thread recovery does not behave as expected.
- Contributors should also read [Local development](development/local-development.md).
