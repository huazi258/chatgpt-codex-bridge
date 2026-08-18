(() => {
const contentAdapterVersion = '1.3.2';
const contentAdapterInstanceKey = '__chatgptCodexContentAdapterInstanceV3__';
const previousAdapterInstance = globalThis[contentAdapterInstanceKey];

if (previousAdapterInstance?.version === contentAdapterVersion && previousAdapterInstance.active) return;
if (previousAdapterInstance) previousAdapterInstance.active = false;
const contentAdapterInstance = { version: contentAdapterVersion, active: true };
globalThis[contentAdapterInstanceKey] = contentAdapterInstance;

const assistantMessageSelector = '[data-message-author-role="assistant"]';
const composerSelector = '#prompt-textarea';
const sendButtonSelectors = [
  'button[data-testid="send-button"]',
  'button[aria-label*="send" i]',
  'button[title*="send" i]',
  'button[aria-label*="发送"]',
  'button[title*="发送"]'
];
const stopButtonSelector = 'button[data-testid="stop-button"]';
const relayWrappedControls = [
  ['@@@CODEX_PROMPT@@@', '@@@END_CODEX_PROMPT@@@'],
  ['@@@BLOCKED@@@', '@@@END_BLOCKED@@@'],
  ['@@@CODEX_INPUT@@@', '@@@END_CODEX_INPUT@@@'],
];
const relayControlMarkers = [
  ...relayWrappedControls.flatMap(([start, end]) => [start, end]),
  '@@@MODULE_DONE@@@',
];

function assistantMessages() {
  return [...document.querySelectorAll(assistantMessageSelector)];
}

function isGenerating() {
  return Boolean(document.querySelector(stopButtonSelector));
}

function replyFromAssistantMessage(message) {
  if (!message) return { text: '', protocolJson: null };
  const codeNodes = [...message.querySelectorAll('pre code')];
  const fallbackNodes = codeNodes.length ? codeNodes : [...message.querySelectorAll('pre')];
  const codeBlocks = fallbackNodes.map((node) => node.innerText.trim());
  return {
    text: globalThis.restoreProtocolCodeBlock(message.innerText, codeBlocks),
    protocolJson: globalThis.extractProtocolJsonObject(message.innerText, codeBlocks)
  };
}

function latestAssistantReply() {
  return replyFromAssistantMessage(assistantMessages().at(-1));
}

function protocolReplySince(previousCount) {
  return assistantMessages()
    .slice(previousCount)
    .reverse()
    .map(replyFromAssistantMessage)
    .find((reply) => Boolean(reply.protocolJson)) ?? null;
}

function assistantDiagnosticsSummary(previousCount, baselineAssistantText, changedAssistantMessages) {
  return assistantMessages().map((message, index) => {
    const reply = replyFromAssistantMessage(message);
    const codeBlocks = message.querySelectorAll('pre, pre code').length;
    const isNew = !baselineAssistantText.has(message);
    const changed = changedAssistantMessages.has(message) || baselineAssistantText.get(message) !== message.innerText;
    return `#${index}${index >= previousCount ? '*' : ''}(chars=${message.innerText.length},new=${isNew},changed=${changed},code=${codeBlocks},json=${Boolean(reply.protocolJson)})`;
  }).join(', ');
}

function hasPendingProtocolCandidate(message) {
  return Boolean(message.querySelector('pre, pre code'))
    || /"(?:state|module|reason)"\s*:/.test(message.innerText);
}

function hasPendingRelayControlCandidate(text) {
  const current = text.trimEnd();
  for (const [start, end] of relayWrappedControls) {
    const startOffset = current.lastIndexOf(start);
    if (startOffset >= 0 && !current.slice(startOffset + start.length).includes(end)) return true;
  }
  if (relayControlMarkers.some((marker) => current.endsWith(marker))) return false;
  const markerOffset = current.lastIndexOf('@@@');
  if (markerOffset < 0) return false;
  const trailingMarker = current.slice(markerOffset);
  return trailingMarker.length >= 3
    && relayControlMarkers.some((marker) => marker !== trailingMarker && marker.startsWith(trailingMarker));
}

function textOfLatestAssistantMessage() {
  return latestAssistantReply().text;
}

function findSendButton() {
  for (const selector of sendButtonSelectors) {
    const button = [...document.querySelectorAll(selector)].find((candidate) => !candidate.disabled);
    if (button) return button;
  }
  return null;
}

function describeButtons(composer) {
  const scope = composer.closest('form') ?? composer.parentElement?.parentElement ?? document;
  return [...scope.querySelectorAll('button')].slice(-12).map((button) => ({
    testId: button.dataset.testid ?? null,
    ariaLabel: button.getAttribute('aria-label'),
    title: button.getAttribute('title'),
    disabled: button.disabled,
    text: button.innerText.trim().slice(0, 40)
  }));
}

function waitForSendButton(composer, timeoutMs = 2_000) {
  return new Promise((resolve) => {
    const startedAt = Date.now();
    const timer = setInterval(() => {
      const button = findSendButton();
      if (button || Date.now() - startedAt >= timeoutMs) {
        clearInterval(timer);
        resolve(button);
      }
    }, 100);
  });
}

function waitForCompletedAssistantReply(previousCount, baselineAssistantText, requireProtocolJson = false, timeoutMs = 90_000, baselineProtocolJson = new Set()) {
  return new Promise((resolve, reject) => {
    const startedAt = Date.now();
    let stableSince = 0;
    let latestSignature = '';
    const changedAssistantMessages = new Set();
    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        const element = mutation.target.nodeType === Node.ELEMENT_NODE
          ? mutation.target
          : mutation.target.parentElement;
        const assistant = element?.closest?.(assistantMessageSelector);
        if (assistant) changedAssistantMessages.add(assistant);
      }
      check();
    });
    const timer = setInterval(check, 250);

    function finish(error, value) {
      observer.disconnect();
      clearInterval(timer);
      error ? reject(error) : resolve(value);
    }

    function check() {
      const messageCount = assistantMessages().length;
      const changedMessages = assistantMessages()
        .filter((message) => changedAssistantMessages.has(message)
          || !baselineAssistantText.has(message)
          || baselineAssistantText.get(message) !== message.innerText);
      const changedReplies = changedMessages.map(replyFromAssistantMessage);
      const currentReplies = assistantMessages().map(replyFromAssistantMessage);
      const replyOutsideBaseline = requireProtocolJson
        ? globalThis.findProtocolReplyOutsideBaseline(currentReplies, baselineProtocolJson)
        : null;
      const protocolReply = requireProtocolJson
        ? changedReplies.find((reply) => Boolean(reply.protocolJson)) ?? replyOutsideBaseline
        : null;
      const freshRelayMessage = !requireProtocolJson ? changedMessages.at(-1) ?? null : null;
      const freshRelayReply = freshRelayMessage
        ? replyFromAssistantMessage(freshRelayMessage)
        : null;
      const latestReply = protocolReply ?? freshRelayReply ?? latestAssistantReply();
      const signature = `${messageCount}\n${latestReply.text}`;
      const pendingProtocolCandidate = requireProtocolJson
        && changedMessages.some(hasPendingProtocolCandidate);
      const pendingRelayControlCandidate = !requireProtocolJson
        && Boolean(freshRelayReply)
        && hasPendingRelayControlCandidate(freshRelayReply.text);
      const deadlineMs = globalThis.protocolReplyDeadlineMs(
        requireProtocolJson,
        pendingProtocolCandidate,
        timeoutMs
      );
      if (Date.now() - startedAt > deadlineMs) {
        const error = new Error(requireProtocolJson
          ? 'Timed out waiting for a complete structured protocol JSON reply.'
          : 'Timed out waiting for the ChatGPT reply to finish.');
        error.adapterDiagnostic = `baseline=${baselineAssistantText.size},baselineProtocolJson=${baselineProtocolJson.size},current=${messageCount},changed=${changedAssistantMessages.size},freshRelayCandidates=${changedMessages.length},freshRelayReply=${Boolean(freshRelayReply)},replyOutsideBaseline=${Boolean(replyOutsideBaseline)},pendingProtocolCandidate=${pendingProtocolCandidate},pendingRelayControlCandidate=${pendingRelayControlCandidate},deadlineMs=${deadlineMs}; ${assistantDiagnosticsSummary(previousCount, baselineAssistantText, changedAssistantMessages)}`;
        finish(error);
        return;
      }

      if (signature !== latestSignature) {
        latestSignature = signature;
        stableSince = 0;
      }

      if ((requireProtocolJson && !protocolReply)
        || (!requireProtocolJson && !freshRelayReply)
        || (!requireProtocolJson && isGenerating())
        || pendingRelayControlCandidate) {
        stableSince = 0;
        return;
      }

      if (!stableSince) stableSince = Date.now();
      if (Date.now() - stableSince >= 2_000) {
        finish(null, latestReply);
      }
    }

    observer.observe(document.documentElement, { childList: true, characterData: true, subtree: true });
    check();
  });
}

function setComposerText(composer, text) {
  if (composer instanceof HTMLTextAreaElement) {
    composer.value = text;
    composer.dispatchEvent(new InputEvent('input', {
      bubbles: true,
      inputType: 'insertText',
      data: text
    }));
    return;
  }

  if (composer.isContentEditable) {
    composer.focus();
    document.execCommand('selectAll', false);
    const inserted = document.execCommand('insertText', false, text);
    if (!inserted) {
      composer.textContent = text;
      composer.dispatchEvent(new InputEvent('input', {
        bubbles: true,
        inputType: 'insertText',
        data: text
      }));
    }
    return;
  }

  throw new Error(`Unsupported composer element: <${composer.tagName.toLowerCase()}>`);
}

async function sendAndWait(text, includeProtocolJson = false, onTrace = () => undefined) {
  let stage = 'locating composer';
  try {
    const composer = document.querySelector(composerSelector);
    if (!composer) throw new Error('ChatGPT composer (#prompt-textarea) was not found.');
    stage = 'checking generation state';
    if (isGenerating()) throw new Error('ChatGPT is still generating; wait before starting the smoke test.');

    stage = 'counting existing assistant messages';
    const previousMessages = assistantMessages();
    const previousCount = previousMessages.length;
    const baselineAssistantText = new Map(previousMessages.map((message) => [message, message.innerText]));
    const baselineProtocolJson = new Set(
      previousMessages
        .map(replyFromAssistantMessage)
        .map((reply) => reply.protocolJson)
        .filter(Boolean)
    );
    stage = 'focusing composer';
    composer.focus();
    stage = 'setting composer text';
    setComposerText(composer, text);

    stage = 'locating send button';
    const sendButton = await waitForSendButton(composer);
    if (!sendButton) {
      throw new Error(`ChatGPT send button was not available. Candidate buttons: ${JSON.stringify(describeButtons(composer))}`);
    }
    stage = 'clicking send button';
    sendButton.click();
    onTrace('CHATGPT_SEND_CLICKED');

    stage = 'waiting for completed reply';
    const reply = await waitForCompletedAssistantReply(
      previousCount,
      baselineAssistantText,
      includeProtocolJson,
      90_000,
      baselineProtocolJson
    );
    onTrace('CHATGPT_REPLY_COMPLETED');
    return includeProtocolJson ? reply : reply.text;
  } catch (error) {
    error.message = `${stage}: ${error.message}`;
    throw error;
  }
}

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (!contentAdapterInstance.active) return;

  if (message?.type === 'adapterStatusV3') {
    sendResponse({ ok: true, adapterVersion: contentAdapterVersion });
    return;
  }

  if (message?.type === 'adapterProbeV4') {
    const reply = protocolReplySince(0) ?? latestAssistantReply();
    sendResponse({
      ok: true,
      adapterVersion: contentAdapterVersion,
      assistantCount: assistantMessages().length,
      protocolJsonPresent: Boolean(reply.protocolJson),
      normalizedReplyLength: reply.text.length
    });
    return;
  }

  if (message?.type === 'status') {
    sendResponse({
      ok: true,
      generating: isGenerating(),
      assistantCount: assistantMessages().length,
      latestAssistantText: textOfLatestAssistantMessage()
    });
    return;
  }

  if (message?.type === 'runSmokeTest') {
    sendAndWait('Reply with exactly CHATGPT_EXTENSION_SMOKE_OK and nothing else.')
      .then((response) => sendResponse({ ok: true, response }))
      .catch((error) => sendResponse({
        ok: false,
        error: error.message,
        stack: error.stack
      }));
    return true;
  }

  if (message?.type === 'sendMiddlewareMessageV3') {
    const requestId = typeof message.requestId === 'string' ? message.requestId : 'unknown';
    console.info('[relay-trace]', { requestId, stage: 'CONTENT_RECEIVED' });
    sendAndWait(message.text, message.includeProtocolJson === true, (stage) => {
      console.info('[relay-trace]', { requestId, stage });
    })
      .then((response) => sendResponse(typeof response === 'string'
        ? { ok: true, response }
        : {
            ok: true,
            response: response.text,
            protocolJson: response.protocolJson,
            protocolJsonPresent: Boolean(response.protocolJson),
            adapterVersion: contentAdapterVersion
          }))
      .catch((error) => sendResponse({
        ok: false,
        error: error.message,
        protocolJsonPresent: false,
        adapterVersion: contentAdapterVersion,
        adapterDiagnostic: error.adapterDiagnostic ?? null
      }));
    return true;
  }
});
})();
