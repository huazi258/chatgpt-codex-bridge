import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const content = readFileSync(new URL('./content.js', import.meta.url), 'utf8');
const background = readFileSync(new URL('./background.js', import.meta.url), 'utf8');

assert.match(content, /adapterStatusV3/, 'content script must expose its version over an isolated V3 message');
assert.match(content, /contentAdapterInstanceKey/, 'content script must identify its single active adapter instance');
assert.match(content, /previousAdapterInstance\?\.version === contentAdapterVersion && previousAdapterInstance\.active/, 'reinjection of the same adapter version must be idempotent');
assert.match(content, /if \(!contentAdapterInstance\.active\) return;/, 'an upgraded adapter must deactivate prior listeners');
assert.match(content, /sendMiddlewareMessageV3/, 'content script must own the V3 task message');
assert.match(background, /ensureLatestContentAdapter/, 'background must verify the content-script version before dispatch');
assert.match(background, /sendMiddlewareMessageV3/, 'background must avoid messages that older content scripts can receive');
assert.doesNotMatch(background, /message\.relay \? 'sendMiddlewareMessage'/, 'relay dispatch must also use the isolated V3 message');
assert.match(content, /adapterProbeV4/, 'content script must report whether the latest reply yielded structured JSON');
assert.match(background, /inspectProtocolAdapter/, 'popup must be able to verify the bound ChatGPT tab before protocol dispatch');
assert.match(background, /ensureLatestContentAdapter\(message\.tabId\)/, 'pairing must verify the adapter instead of merely opening a WebSocket');
assert.match(content, /waitForCompletedAssistantReply\(previousCount, baselineAssistantText, requireProtocolJson/, 'protocol dispatch must wait for a complete structured JSON reply');
assert.match(content, /runSmokeTest'[\s\S]*sendAndWait/, 'popup smoke must use the same content adapter send-and-wait path');
assert.match(content, /sendMiddlewareMessageV3'[\s\S]*sendAndWait/, 'formal relay must use the same content adapter send-and-wait path');
assert.match(content, /requireProtocolJson && !protocolReply/, 'protocol dispatch must reject an unfinished reply even when stop-button detection fails');
assert.match(content, /protocolReplySince\(previousCount\)/, 'protocol dispatch must search every assistant node added during the task');
assert.match(content, /assistantDiagnosticsSummary\(previousCount, baselineAssistantText, changedAssistantMessages\)/, 'protocol timeout must identify which assistant nodes were actually observed');
assert.match(background, /adapterDiagnostic/, 'desktop must receive non-sensitive protocol-timeout diagnostics');
assert.match(content, /baselineAssistantText/, 'protocol dispatch must compare the post-send DOM against a pre-send assistant-message baseline');
assert.match(content, /changedAssistantMessages/, 'protocol dispatch must track assistant nodes changed by the current request');
assert.match(content, /hasPendingProtocolCandidate/, 'a partial structured response must be identified before the ordinary deadline expires');
assert.match(content, /protocolReplyDeadlineMs/, 'protocol dispatch must use the bounded structured-response grace policy');
assert.match(content, /findProtocolReplyOutsideBaseline/, 'protocol dispatch must recover a new JSON reply after ChatGPT reuses an assistant DOM node');

console.log('ADAPTER_VERSION_SMOKE_OK');
