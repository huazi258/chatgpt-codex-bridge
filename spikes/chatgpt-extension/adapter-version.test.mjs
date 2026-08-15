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

console.log('ADAPTER_VERSION_SMOKE_OK');
