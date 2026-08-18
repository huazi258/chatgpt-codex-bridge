import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { JSDOM } from 'jsdom';

const [protocolTextSource, contentSource] = await Promise.all([
  readFile(new URL('./protocol-text.js', import.meta.url), 'utf8'),
  readFile(new URL('./content.js', import.meta.url), 'utf8'),
]);

function wait(window, milliseconds) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

async function runRelayCompletionScenario({
  initialText,
  finalText = initialText,
  reconcileAfterMs,
  baselineAssistantTexts = [],
  replaceLastBaselineAssistant = false,
  updateLastBaselineAssistant = false,
  createAssistantOnSend = true,
  adapterTimeoutMs = 90_000,
}) {
  const startedAt = Date.now();
  const dom = new JSDOM(
    '<!doctype html><body><textarea id="prompt-textarea"></textarea><button data-testid="send-button">发送</button></body>',
    { runScripts: 'outside-only', url: 'https://chatgpt.com/' },
  );
  const { window } = dom;
  Object.defineProperty(window.HTMLElement.prototype, 'innerText', {
    configurable: true,
    get() { return this.textContent ?? ''; },
  });

  let messageListener;
  window.chrome = {
    runtime: {
      onMessage: {
        addListener(listener) { messageListener = listener; },
      },
    },
  };
  for (const text of baselineAssistantTexts) {
    const assistant = window.document.createElement('article');
    assistant.dataset.messageAuthorRole = 'assistant';
    assistant.textContent = text;
    window.document.body.append(assistant);
  }

  window.eval(protocolTextSource);
  window.eval(adapterTimeoutMs === 90_000
    ? contentSource
    : contentSource.replace(
      '      90_000,\n      baselineProtocolJson',
      `      ${adapterTimeoutMs},\n      baselineProtocolJson`,
    ));

  const sendButton = window.document.querySelector('[data-testid="send-button"]');
  sendButton.addEventListener('click', () => {
    let assistant;
    if (createAssistantOnSend) {
      if (updateLastBaselineAssistant) {
        const baselineAssistants = [...window.document.querySelectorAll('[data-message-author-role="assistant"]')];
        assistant = baselineAssistants.at(-1);
        assistant.textContent = initialText;
      } else {
        assistant = window.document.createElement('article');
        assistant.dataset.messageAuthorRole = 'assistant';
        assistant.textContent = initialText;
        if (replaceLastBaselineAssistant) {
          const baselineAssistants = [...window.document.querySelectorAll('[data-message-author-role="assistant"]')];
          baselineAssistants.at(-1).replaceWith(assistant);
        } else {
          window.document.body.append(assistant);
        }
      }
    }

    const stopButton = window.document.createElement('button');
    stopButton.dataset.testid = 'stop-button';
    window.document.body.append(stopButton);
    window.setTimeout(() => stopButton.remove(), 100);
    if (assistant && reconcileAfterMs !== undefined) {
      window.setTimeout(() => { assistant.textContent = finalText; }, reconcileAfterMs);
    }
  });

  const response = await Promise.race([
    new Promise((resolve, reject) => {
      const isAsync = messageListener(
        { type: 'sendMiddlewareMessageV3', requestId: 'completion-regression', text: '测试', includeProtocolJson: false },
        {},
        resolve,
      );
      if (isAsync !== true) reject(new Error('content adapter did not retain the async response channel'));
    }),
    wait(window, 10_000).then(() => { throw new Error('content adapter did not complete within the bounded test window'); }),
  ]);
  dom.window.close();
  return { response, elapsedMs: Date.now() - startedAt };
}

const promptPartial = '@@@CODEX_PROMPT@@@\n只回复 RELAY_E2E_OK\n@@@END_CODEX_PRO';
const promptComplete = '@@@CODEX_PROMPT@@@\n只回复 RELAY_E2E_OK\n@@@END_CODEX_PROMPT@@@';
const promptScenario = await runRelayCompletionScenario({
  initialText: promptPartial,
  finalText: promptComplete,
  reconcileAfterMs: 3_250,
});
const { response: promptResponse } = promptScenario;
assert.equal(promptResponse.ok, true);
assert.equal(promptResponse.response, promptComplete, 'must not capture a partial CODEX_PROMPT terminal marker after stop-button disappears');
assert.notEqual(promptResponse.response, promptPartial, 'must wait for the delayed CODEX_PROMPT DOM reconciliation');

const inputPartial = '@@@CODEX_INPUT@@@\n请';
const inputComplete = '@@@CODEX_INPUT@@@\n1. 请返回 Codex 的输出\n@@@END_CODEX_INPUT@@@';
const inputScenario = await runRelayCompletionScenario({
  initialText: inputPartial,
  finalText: inputComplete,
  reconcileAfterMs: 3_250,
});
const { response: inputResponse } = inputScenario;
assert.equal(inputResponse.ok, true);
assert.equal(inputResponse.response, inputComplete, 'must not capture an incomplete CODEX_INPUT body or end marker');
assert.notEqual(inputResponse.response, inputPartial, 'must wait for the delayed CODEX_INPUT DOM reconciliation');

const ordinaryText = '普通聊天回复完成。';
const ordinaryScenario = await runRelayCompletionScenario({ initialText: ordinaryText });
const { response: ordinaryResponse } = ordinaryScenario;
assert.equal(ordinaryResponse.ok, true);
assert.equal(ordinaryResponse.response, ordinaryText, 'ordinary plain-text replies must still complete without a relay control block');
assert.ok(ordinaryScenario.elapsedMs < 5_500, 'ordinary plain-text replies must retain the normal bounded completion path');

const donePartial = '@@@MODULE_DON';
const doneComplete = '@@@MODULE_DONE@@@';
const doneScenario = await runRelayCompletionScenario({
  initialText: donePartial,
  finalText: doneComplete,
  reconcileAfterMs: 3_250,
});
const { response: doneResponse } = doneScenario;
assert.equal(doneResponse.ok, true);
assert.equal(doneResponse.response, doneComplete, 'must not capture a partial MODULE_DONE terminal marker');
assert.notEqual(doneResponse.response, donePartial, 'must wait for the delayed MODULE_DONE DOM reconciliation');

const acceptancePrompt = '@@@CODEX_PROMPT@@@\n只回复完全一致的一行：ACCEPTANCE_CYCLE_1_OK\n不要运行命令，不要读取或修改文件。\n@@@END_CODEX_PROMPT@@@';
const replacementScenario = await runRelayCompletionScenario({
  baselineAssistantTexts: ['历史回复 #0', '历史回复 #1', '历史回复 #2'],
  initialText: acceptancePrompt,
  replaceLastBaselineAssistant: true,
  adapterTimeoutMs: 3_500,
});
const { response: replacementResponse } = replacementScenario;
assert.equal(replacementResponse.ok, true, 'a same-count replacement must be accepted as the fresh relay reply');
assert.equal(replacementResponse.response, acceptancePrompt, 'must return the complete fresh replacement reply');
assert.ok(replacementScenario.elapsedMs < 3_500, 'same-count replacement must complete before the test adapter timeout');

const updateScenario = await runRelayCompletionScenario({
  baselineAssistantTexts: ['历史回复 #0', '历史回复 #1', '历史回复 #2'],
  initialText: acceptancePrompt,
  updateLastBaselineAssistant: true,
  adapterTimeoutMs: 3_500,
});
const { response: updateResponse } = updateScenario;
assert.equal(updateResponse.ok, true, 'an updated baseline assistant node must be accepted as the fresh relay reply');
assert.equal(updateResponse.response, acceptancePrompt, 'must return the complete updated assistant reply');

const noFreshScenario = await runRelayCompletionScenario({
  baselineAssistantTexts: ['历史回复 #0', '历史回复 #1', '历史回复 #2'],
  createAssistantOnSend: false,
  adapterTimeoutMs: 700,
});
const { response: noFreshResponse } = noFreshScenario;
assert.equal(noFreshResponse.ok, false, 'without a fresh assistant node, the adapter must not accept a baseline reply');
assert.match(noFreshResponse.error, /Timed out waiting for the ChatGPT reply to finish/, 'no fresh reply must fail by completion timeout');

console.log('CONTENT_ADAPTER_COMPLETION_OK');
