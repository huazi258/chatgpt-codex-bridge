import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(new URL('./protocol-text.js', import.meta.url), 'utf8');
const context = { globalThis: {} };
vm.runInNewContext(source, context);

const visibleText = '协议已加载。\n{\n  "state": "PAUSE"\n}';
const normalized = context.globalThis.restoreProtocolCodeBlock(visibleText, ['{\n  "state": "PAUSE"\n}']);
assert.match(normalized, /```json\n\{\n  "state": "PAUSE"\n\}\n```$/);

const fallbackNormalized = context.globalThis.restoreProtocolCodeBlock(visibleText, []);
assert.match(fallbackNormalized, /```json\n\{\n  "state": "PAUSE"\n\}\n```$/);

const chromeRenderedText = '协议已加载。\nJSON\n{\n  "state": "PAUSE"\n}\nCopy code';
const chromeRenderedNormalized = context.globalThis.restoreProtocolCodeBlock(chromeRenderedText, []);
assert.match(chromeRenderedNormalized, /```json\n\{\n  "state": "PAUSE"\n\}\n```$/, 'must remove ChatGPT code-block chrome surrounding a valid protocol object');
assert.equal(
  context.globalThis.extractProtocolJsonObject(chromeRenderedText, []),
  '{\n  "state": "PAUSE"\n}',
  'must expose the raw protocol JSON for the structured bridge channel'
);

assert.equal(
  context.globalThis.protocolReplyDeadlineMs(true, false),
  90_000,
  'an absent structured-reply candidate must retain the normal protocol deadline'
);
assert.equal(
  context.globalThis.protocolReplyDeadlineMs(true, true),
  180_000,
  'a partially rendered structured-reply candidate must receive one bounded grace period'
);

const pauseJson = '{"state":"PAUSE","module":"Task 7 bounded pilot","reason":"ready"}';
const nextTaskJson = '{"state":"NEXT_TASK","module":"Task 7 bounded pilot","reason":"continue","codex_prompt":"write only test/task7-pilot.md","acceptance_criteria":["file exists"]}';
assert.equal(
  context.globalThis.findProtocolReplyOutsideBaseline(
    [
      { text: 'old visible reply', protocolJson: pauseJson },
      { text: 'new visible reply', protocolJson: nextTaskJson }
    ],
    new Set([pauseJson])
  )?.protocolJson,
  nextTaskJson,
  'a valid protocol reply must be accepted when DOM reuse hides its mutation but its JSON differs from the pre-send baseline'
);
console.log('PROTOCOL_TEXT_SMOKE_OK');
