# 003 — ChatGPT local pairing and protocol boundary

- Status: Accepted; live-browser validation completed
- Date: 2026-08-15
- Decision: Pair the dedicated ChatGPT extension tab to the desktop application through a loopback-only WebSocket and an in-memory, one-time pairing secret.

## Contract

The desktop app listens only on `ws://127.0.0.1:8765`. At launch it generates a fresh pairing secret and exposes it in the local desktop UI; it is not written to SQLite or logs. The extension sends the secret and selected ChatGPT tab ID only to pair. A successful pair receives a short-lived session ID, which is required for all later messages.

The desktop app sends middleware-owned ChatGPT text through the paired extension. The extension injects it into the bound `chatgpt.com` tab, waits for the answer to finish, and returns only the completed reply text. The desktop app rejects malformed bridge traffic, unpaired replies, multiple/non-final JSON code blocks, unknown protocol states, and envelopes that violate the documented state requirements. Every validation failure is surfaced as `BLOCKED`; no Codex work is started by this adapter.

## Evidence

- `BRIDGE_SMOKE_OK`: the running desktop bridge rejected an unauthorized pairing attempt at `127.0.0.1:8765`.
- Rust tests verify valid `PAUSE` parsing and rejection of malformed `NEXT_TASK` and multiple code blocks.
- On 2026-08-16, the signed-in Chrome extension paired with the desktop application and a protocol bootstrap completed successfully. The desktop UI displayed `已验证 ChatGPT 协议状态：PAUSE` and `协议已验证：PAUSE`.

## Keepalive repair

The first live pairing disconnected after a short idle period. The desktop endpoint previously treated a WebSocket `Ping` control frame as a disconnect, and the Manifest V3 service worker sent no traffic while idle. The bridge now replies to `Ping` frames; the extension sends a session-bound keepalive every 15 seconds and reconnects after an unexpected close. The regression test `bridge_keeps_a_paired_connection_alive_when_it_receives_a_ping` failed before this repair and passes after it.

The first live protocol bootstrap was also rejected because ChatGPT's rendered message DOM exposes a code block's JSON text but omits the literal ```json fence from `innerText`. The extension now extracts one JSON `pre/code` block from the completed assistant message and restores it as the required final fenced JSON block before returning the reply. `protocol-text.test.mjs` reproduces this exact rendered-text path and now passes.

The second live screenshot showed a code-block DOM shape that did not match the `pre/code` selector, leaving the extension with an empty code-block list despite the JSON being visibly present. The text normalizer now has a constrained fallback: if no code node is found, it recovers one trailing JSON object containing a `state` field from the completed assistant text and restores its fence. The same smoke test covers both selector and no-selector paths.

## Content-script version repair

Reloading a Manifest V3 extension does not replace a content script that was already injected into an open ChatGPT tab. That stale script can still answer the legacy task message and bypass later extraction fixes. The bridge now verifies an isolated `adapterStatusV2` handshake before each protocol dispatch; a mismatched or absent adapter is reinjected, and the actual task uses `sendMiddlewareMessageV2`, which legacy scripts ignore. The scripts use IIFEs so reinjection is safe. `adapter-version.test.mjs` failed before this contract existed and passes with it. The PAUSE bootstrap prompt now also explicitly forbids the `codex_prompt` field, which is invalid for PAUSE.

The next captured DOM shape included presentation chrome around the JSON (`JSON` and `Copy code`). The earlier fallback incorrectly parsed from the opening `{` through the end of the assistant text, so that UI text made a valid object appear invalid. The normalizer now scans balanced JSON objects while respecting quoted strings and escape sequences, then rebuilds the final fenced block without trailing presentation chrome. `protocol-text.test.mjs` contains this exact minimal failure case and failed before the repair.

The repeated live failure showed that even normalized text is the wrong interface between the extension and desktop validator. Adapter version `0.5.0` now returns the extracted JSON object as a separate `protocolJson` bridge field. The desktop validates that structured field when present and uses the fenced-text parser only for backwards compatibility. The Rust test `protocol_validator_accepts_structured_json_from_extension` failed before this path existed and passes after the repair.

The live adapter probe later confirmed version `0.6.0` on both extension and ChatGPT tab, and confirmed that it could extract structured JSON, while the desktop still received an unfenced response. The desktop validator therefore also accepts one unique JSON object embedded in otherwise rendered ChatGPT text, after its preferred structured-field and fenced-block paths. It uses `serde_json` streaming parsing rather than brace counting, so quoted braces do not corrupt extraction. `protocol_validator_accepts_unfenced_json_with_chatgpt_presentation_chrome` reproduces the observed `JSON`/`Copy code` text and passes.

The remaining timing race was in the content script: it could return one second after an assistant node appeared even while text was still streaming, because completion depended on a fragile stop-button selector. Adapter `0.7.0` requires a parsed `protocolJson` for V2 protocol dispatch and then waits for two seconds of unchanged reply text. If no structured object is returned before timeout, the desktop reports that precise adapter failure rather than a misleading missing-code-block error. Manifest and adapter versions are now both `0.7.0`.

The first precise timeout showed that the last assistant DOM node can be an empty auxiliary node even when a preceding node contains the completed JSON. Adapter `0.8.0` scans all assistant nodes created after the task started and selects the most recent one with a parsed protocol object. Manifest and adapter versions are both `0.8.0`.

The next timeout established that ChatGPT can also reuse an existing assistant DOM node, so node counts and post-count slicing are not reliable task boundaries. Adapter `1.0.0` records the text of every assistant node immediately before dispatch, tracks mutation-observer changes, and accepts only protocol JSON found in a node that is new or changed by that dispatch. Timeout reports include only counts, lengths, and boolean extraction results; no conversation text is exposed.

## Delayed protocol rendering repair

The Task 7 pilot produced a valid `PAUSE` reply visible in ChatGPT after the adapter had already timed out. Its timeout diagnostic showed a new assistant node with one code block but only 30 characters and no parseable JSON. Adapter `1.1.0` retains the normal 90-second deadline when no structured candidate exists. When the current dispatch has created or changed an assistant node containing a JSON/code-block candidate, it grants exactly one additional 90-second grace period and still requires a parsed JSON object plus two seconds of stable text before returning. The pure deadline policy is covered by `protocol-text.test.mjs`; diagnostics expose candidate and deadline state without returning conversation text.
