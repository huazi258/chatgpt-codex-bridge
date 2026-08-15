import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const content = readFileSync(new URL('./content.js', import.meta.url), 'utf8');
const background = readFileSync(new URL('./background.js', import.meta.url), 'utf8');

assert.match(content, /adapterStatusV2/, 'content script must expose its version over an isolated V2 message');
assert.match(content, /sendMiddlewareMessageV2/, 'content script must own the V2 task message');
assert.match(background, /ensureLatestContentAdapter/, 'background must verify the content-script version before dispatch');
assert.match(background, /sendMiddlewareMessageV2/, 'background must avoid the legacy message that an old content script can receive');
assert.match(content, /adapterProbeV3/, 'content script must report whether the latest reply yielded structured JSON');
assert.match(background, /inspectProtocolAdapter/, 'popup must be able to verify the bound ChatGPT tab before protocol dispatch');
assert.match(background, /ensureLatestContentAdapter\(message\.tabId\)/, 'pairing must verify the adapter instead of merely opening a WebSocket');
assert.match(content, /waitForCompletedAssistantReply\(previousCount, baselineAssistantText, requireProtocolJson/, 'protocol dispatch must wait for a complete structured JSON reply');
assert.match(content, /requireProtocolJson && !protocolReply/, 'protocol dispatch must reject an unfinished reply even when stop-button detection fails');
assert.match(content, /protocolReplySince\(previousCount\)/, 'protocol dispatch must search every assistant node added during the task');
assert.match(content, /assistantDiagnosticsSummary\(previousCount, baselineAssistantText, changedAssistantMessages\)/, 'protocol timeout must identify which assistant nodes were actually observed');
assert.match(background, /adapterDiagnostic/, 'desktop must receive non-sensitive protocol-timeout diagnostics');
assert.match(content, /baselineAssistantText/, 'protocol dispatch must compare the post-send DOM against a pre-send assistant-message baseline');
assert.match(content, /changedAssistantMessages/, 'protocol dispatch must track assistant nodes changed by the current request');

console.log('ADAPTER_VERSION_SMOKE_OK');
