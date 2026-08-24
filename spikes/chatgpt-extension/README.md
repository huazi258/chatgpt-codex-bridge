# ChatGPT extension adapter

This Task 4 extension targets only `https://chatgpt.com/*` and the desktop app's loopback bridge at `ws://127.0.0.1:8765`. It binds exactly one signed-in ChatGPT tab and sends only completed reply text to the local desktop middleware.

## Pairing and smoke test

1. In Google Chrome, open `chrome://extensions` and enable **Developer mode**.
2. Select **Reload** for this unpacked extension (or load this folder if it is not installed), then return to the paired ChatGPT tab and press `Ctrl+R` once. This removes an older content script that may still be resident in that tab.
3. Start the desktop app. In **ChatGPT 专用标签页配对**, copy the one-time pairing secret.
4. Open a signed-in, idle `https://chatgpt.com` conversation, open the extension popup, paste the secret, then choose **Pair active ChatGPT tab**.
5. Return to the desktop app and confirm the state becomes `PAIRED`.
6. Choose **发送协议引导**. A valid reply changes the desktop state to `VALID_PROTOCOL` with protocol state `PAUSE`.
7. The independent smoke button remains available and should report `CHATGPT_EXTENSION_SMOKE_OK`.

The pairing secret becomes invalid whenever the desktop app restarts. It is stored only in extension local storage to allow reconnection attempts during the same app session; the desktop app does not persist it.

While paired, the extension sends a local keepalive every 15 seconds and reconnects after an unexpected local bridge disconnect. Before dispatching a protocol task, the background worker verifies that the selected ChatGPT tab has adapter version `1.3.3` and injects it when required. The content adapter initializes idempotently, so automatic manifest injection and recovery injection cannot leave multiple active message listeners in one tab. Version 1.3 uses isolated V3 adapter messages, which older resident adapters ignore after an extension reload. It records a pre-send text and JSON baseline for every assistant node. A structured reply is accepted when it comes from a node created or changed by the current request, or when its parsed JSON differs from every pre-send protocol object; this handles ChatGPT virtual-list DOM reuse without accepting an old identical reply. Plain-text Relay replies use the same fresh-node evidence, so replacing an assistant node without changing the virtual-list count cannot make the adapter time out or accept an old baseline reply. A visible incomplete Relay control marker keeps the adapter waiting for a later DOM reconciliation instead of returning partial text. A protocol reply normally has 90 seconds to produce structured JSON. If the current request has already created or changed a candidate JSON/code-block node, the adapter grants one bounded 90-second grace period for final DOM rendering; it otherwise fails at the normal deadline. If this times out, the desktop displays a non-sensitive summary of the assistant nodes observed by the extension. Use **Check current adapter** before protocol dispatch; it confirms both versions and whether the latest visible reply yielded structured JSON. Reload the extension and the ChatGPT tab once after upgrading to this version.

## Known unstable selectors

- Composer: `#prompt-textarea`
- Send button: `button[data-testid="send-button"]`
- In-progress marker: `button[data-testid="stop-button"]`
- Assistant message: `[data-message-author-role="assistant"]`

If any selector changes, the extension reports a failure rather than guessing. Pairing and protocol errors are reported to the desktop app as a blocked state.

ChatGPT renders code blocks without literal ```json markers in normal visible text. The extension restores the one JSON code block before sending the reply to the desktop validator. If a ChatGPT DOM update hides its code-block tag from the selector, the extension safely recovers one trailing protocol JSON object with a `state` field.

The current `#prompt-textarea` is handled as either a native `<textarea>` or a `contenteditable` editor. The extension uses the matching input path and reports an unsupported element type explicitly.
