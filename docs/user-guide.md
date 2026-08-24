# User Guide

This document explains how to operate the current Conversation Relay V2 interface.

## Mental model

Bridge connects four roles:

| Role | Responsibility |
| --- | --- |
| User | Chooses the project, prepares project instructions and ChatGPT context, and makes final acceptance or recovery decisions. |
| ChatGPT | Plans and reviews engineering work and writes every Codex prompt. |
| Codex | Executes work in the selected working directory and returns final text. |
| Bridge | Relays messages, serializes delivery, maintains the active Codex thread, records state, applies budgets, and stops for explicit recovery when required. |

Bridge is intentionally not an autonomous engineering manager. It transports and controls the conversation between the two agents.

## Workspace layout

The current desktop interface has:

- a left sidebar containing modules and ChatGPT connection state;
- a ChatGPT browser connection card;
- global channel status;
- module status and budgets;
- Codex communication cycles;
- the resident ChatGPT conversation;
- recovery or acceptance panels when human action is required.

## ChatGPT browser connection

Bridge communicates with one signed-in ChatGPT tab through the unpacked Chrome extension.

The connection card shows:

- local endpoint;
- one-time pairing secret;
- current browser bridge detail;
- a **刷新连接状态** action.

The pairing secret is not a permanent credential. A desktop-app restart creates a new one.

## Module

A **module** is one user-visible work phase.

A module contains:

- a name;
- one Codex working directory;
- one new or resumed Codex thread target;
- a cycle budget;
- a runtime budget;
- message history;
- Codex relay cycles;
- lifecycle and recovery state.

A module can contain many engineering tasks and many Codex turns.

### Creating a module

Choose **新建模块** and enter the required fields.

#### New Codex conversation

Choose **新建 Codex 对话** when the module should start with a fresh Codex thread.

The thread is created lazily when the first valid Codex prompt arrives.

#### Continue an existing Codex conversation

Choose **继续现有 Codex 对话** when the new module should continue an eligible existing thread.

After entering the working directory:

1. click **刷新对话**;
2. select one eligible thread;
3. create the module.

Changing the working directory invalidates the previous thread list, so refresh it again.

Bridge never silently takes over a thread that is active elsewhere.

## Manual chat

Choose **手动聊天** for ordinary messages to ChatGPT.

Manual replies are displayed in Bridge but are never interpreted as automation instructions.

Use manual chat for:

- discussing requirements;
- asking ChatGPT questions;
- adding context;
- checking reasoning;
- talking about a blocked situation before deciding what to automate.

A control-looking string inside a manual reply does not start Codex work.

## Automation request

Choose **发送自动化请求** only when ChatGPT's matching reply should be eligible to drive the relay.

A valid automation reply ends with exactly one of three control blocks.

### `CODEX_PROMPT`

```text
@@@CODEX_PROMPT@@@
<verbatim prompt for Codex>
@@@END_CODEX_PROMPT@@@
```

Bridge passes the body to Codex unchanged.

It does not append Git, testing, completion, or repository instructions. Those belong in the prompt or in the project's own instructions.

### `MODULE_DONE`

```text
@@@MODULE_DONE@@@
```

This moves the module to **WAITING_FOR_ACCEPTANCE**.

It is a request for user acceptance, not final completion.

### `BLOCKED`

```text
@@@BLOCKED@@@
<reason and the information or decision needed from the user>
@@@END_BLOCKED@@@
```

This means automatic progress should stop for human intervention.

## Module acceptance

When a module reaches **WAITING_FOR_ACCEPTANCE**, the acceptance panel lets the user choose what happens next.

### Accept

Accepting marks the module complete. Its history remains visible, but the module will no longer send messages or start Codex.

### Send feedback and continue

If the work is not ready, enter feedback in the acceptance panel.

That feedback is sent into the ChatGPT automation queue so ChatGPT can review it and produce another valid control reply.

### Terminate

A module can also be terminated.

If termination is requested while Codex is already running, Bridge allows the active turn to finish but does not continue the automation chain afterward.

## Codex communication cycles

The Codex communication area visualizes the actual relay cycles associated with the selected module.

A cycle represents one valid ChatGPT `CODEX_PROMPT` and its corresponding Codex execution/result lifecycle.

The module's **已开始循环** count increases only when a Codex turn actually starts.

## Runtime and cycle budgets

Each module has two explicit limits:

- maximum started Codex cycles;
- maximum total module runtime after the first Codex turn starts.

These are runtime safety limits, not planning instructions.

Manual chat and protocol retries do not consume a Codex cycle.

When a budget is reached while a Codex turn is already running, the active turn is allowed to finish before the module stops progressing.

## Delivery states

Outbound relay messages expose real delivery state rather than pretending every send succeeded.

Typical states include:

- `QUEUED` — persisted and waiting to send;
- `SENT` — handed off for delivery;
- `DELIVERED` — delivery was confirmed;
- `FAILED` — delivery failed;
- `UNKNOWN` — Bridge cannot safely determine whether the message was delivered.

## `UNKNOWN` recovery

`UNKNOWN` is intentionally a human-intervention state.

It can occur after a restart, connection loss, or another interruption where automatically resending could duplicate a message that may already have reached ChatGPT.

When one or more unknown messages exist, Bridge keeps later messages safely blocked until every unknown is resolved.

For each unknown message, choose exactly one action.

### 明确重发这条消息

Use this only when you intentionally want the old message sent again.

### 不重发并继续

Use this when you believe the original message may already have been delivered, or when sending it again would be unsafe or unnecessary.

Bridge resumes queue dispatch only after all unknown messages have an explicit user decision.

## Codex thread recovery

A module may enter **RECOVERY_REQUIRED** if Bridge cannot prove that the configured Codex thread is safely owned or resumable after an interruption.

The recovery panel exposes the actions the backend considers valid for the recorded state.

Do not work around this by starting duplicate App Server operations manually unless you understand the thread-ownership consequences. The recovery state exists specifically to avoid guessing about side effects.

## Existing Codex threads

Bridge discovers existing threads for the selected working directory.

Current product rules intentionally distinguish between selectable and non-selectable thread states:

- idle/not-loaded threads can be candidates for resume;
- active threads remain visible but cannot be taken over;
- system-error or uncertain states may require recovery instead of immediate reuse.

The working directory is an environment selection, not a repository-management feature.

## Protocol retry

If an automation reply does not end in one valid control block, Bridge uses the configured retry template once.

A second invalid automation reply stops automatic progress and surfaces an actionable error.

Manual-chat replies never enter this retry path.

## Terminal modules

Two terminal states are visible in the current interface.

### `COMPLETED`

The user explicitly accepted the module.

History remains visible; no further messages or Codex work are started.

### `STOPPED`

The user terminated the module or execution stopped according to the module lifecycle.

History remains visible; the module is no longer active.

## Recommended operating pattern

A practical workflow is:

1. use manual chat to build or clarify project context;
2. create a module for one coherent work phase;
3. send an automation request asking ChatGPT to determine the next action;
4. let ChatGPT and Codex exchange as many cycles as required;
5. intervene on `BLOCKED`, `UNKNOWN`, or recovery states;
6. review `MODULE_DONE`;
7. accept only after you are satisfied with the repository state.

For first-time setup, see [Getting Started](getting-started.md).
