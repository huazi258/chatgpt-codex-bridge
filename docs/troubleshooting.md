# Troubleshooting

Use this guide by symptom. Bridge intentionally fails visibly rather than guessing when delivery, browser state, or Codex ownership is uncertain.

## The desktop app does not start

Check the basic toolchain:

```powershell
node --version
npm --version
cargo --version
```

Reinstall JavaScript dependencies from the public npm registry:

```powershell
npm install --registry=https://registry.npmjs.org
```

Make sure Cargo is available to the process:

```powershell
$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"
npm run tauri -- dev
```

On Windows, WebView2 must also be available.

## Bridge cannot start or find Codex

First verify Codex directly:

```powershell
codex --version
```

Also verify that Codex can be used from the target project directory outside Bridge.

If Codex is installed but not visible to the desktop application's environment, launch Bridge from a shell where the Codex executable is on `PATH`.

The historical local-development workflow also supports configuring a full App Server executable path through `CODEX_APP_SERVER_COMMAND` when required by the environment.

## ChatGPT stays disconnected

Check all of the following:

- the Bridge desktop app is running;
- Chrome has the unpacked extension loaded from `spikes/chatgpt-extension`;
- the target tab is on `https://chatgpt.com`;
- you are signed in;
- the active pairing secret came from the current Bridge process;
- the ChatGPT tab was refreshed after loading or reloading the extension.

Then pair the active tab again.

A Bridge restart invalidates the previous pairing secret.

## The extension does not appear to run after an update

Open `chrome://extensions` and reload the unpacked extension.

Then return to the bound ChatGPT tab and refresh it once.

This removes an older content script that may still be resident in the page.

The current extension manifest version in the repository is `1.3.3`.

## Pairing fails

Use the one-time secret currently visible in the Bridge connection card.

Do not reuse a secret from a previous desktop-app session.

The local bridge uses loopback networking only. If local security software blocks WebSocket communication to `127.0.0.1:8765`, allow the local process and retry pairing.

## ChatGPT sends a normal answer but Codex does not start

Confirm that the original message was sent using **发送自动化请求**, not **手动聊天**.

Manual replies are never parsed for control blocks.

Then check that the ChatGPT reply ends with exactly one valid terminal control block.

Example:

```text
@@@CODEX_PROMPT@@@
Do the next repository task.
@@@END_CODEX_PROMPT@@@
```

Ordinary explanatory text may appear before the block, but the block must be the terminal non-whitespace content of the reply.

## Bridge reports an invalid automation reply

Bridge retries the configured protocol prompt once.

If the next reply is still invalid, automatic progress stops.

Check that ChatGPT returned exactly one of:

- `CODEX_PROMPT`;
- `MODULE_DONE`;
- `BLOCKED`.

Do not use retired or unsupported blocks such as `CODEX_INPUT`.

## A message is `UNKNOWN`

Do not treat `UNKNOWN` as a normal failure.

It means Bridge cannot prove whether the old message reached ChatGPT.

Automatic resend is intentionally disabled because it could create duplicate side effects.

Choose one of the UI actions:

- **明确重发这条消息** — intentionally send it again;
- **不重发并继续** — record that it should not be sent again.

The queue remains blocked until every unknown message has an explicit decision.

## Messages stop sending after a restart

Look for the global **待人工处理的不确定送达消息** section.

A restart can turn an unresolved outbound send into `UNKNOWN`.

Resolve every listed unknown message before expecting later queued messages to continue.

## An existing Codex thread cannot be selected

Make sure:

1. the correct Codex working directory is entered;
2. you clicked **刷新对话** after entering or changing that directory;
3. the thread is not already active elsewhere.

Bridge deliberately refuses to take over an external active turn.

Some error or uncertain thread states may be shown but disabled.

## The module enters `RECOVERY_REQUIRED`

Bridge has lost certainty about Codex thread ownership or an operation outcome.

Use the recovery panel shown for that module and follow the available action exposed by the backend.

Do not automatically repeat thread-start or resume operations outside Bridge. The recovery state is designed to prevent duplicate or conflicting side effects.

## `MODULE_DONE` appears but the module is not completed

This is expected.

`MODULE_DONE` means ChatGPT believes the work is ready for user review.

Bridge changes to `WAITING_FOR_ACCEPTANCE` and waits for you to:

- accept;
- send feedback to ChatGPT and continue;
- terminate.

Only explicit acceptance produces the completed module state.

## Termination does not immediately stop a running Codex turn

This is expected behavior.

If a turn is already running, Bridge lets that turn finish. The final result is not used to continue the automation chain after termination has been requested.

## The Chrome extension times out waiting for ChatGPT

The extension relies on ChatGPT's rendered page and waits for a completed reply associated with the current request.

Possible causes include:

- ChatGPT is still generating;
- the page was refreshed during the request;
- the extension was upgraded without refreshing the tab;
- ChatGPT changed the DOM structure used by the adapter.

Reload the extension, refresh the target ChatGPT tab, pair again if needed, and retry the user action.

## A ChatGPT UI update breaks the extension

The current adapter depends on browser selectors including:

- composer: `#prompt-textarea`;
- send button: `button[data-testid="send-button"]`;
- in-progress marker: `button[data-testid="stop-button"]`;
- assistant messages: `[data-message-author-role="assistant"]`.

If ChatGPT changes these elements, the extension is designed to report failure instead of guessing.

Check `spikes/chatgpt-extension/README.md` and the current adapter code when this happens.

## Codex works in the wrong project

The module's **Codex 工作目录** is the environment Bridge gives Codex.

Bridge does not switch Git branches, select repositories, or reinterpret the path.

Stop and verify the module's working directory before continuing.

## I am unsure whether Bridge, ChatGPT, or Codex owns a behavior

Use this rule:

- ChatGPT owns planning and review;
- Codex owns repository execution;
- Bridge owns transport, state, budgets, delivery ordering, and recovery;
- the user owns final acceptance and ambiguous human decisions.

For precise behavior, read `docs/decisions/004-conversation-relay-v2.md`.
