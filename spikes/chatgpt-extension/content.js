(() => {
const contentAdapterVersion = '0.6.0';
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

function assistantMessages() {
  return [...document.querySelectorAll(assistantMessageSelector)];
}

function isGenerating() {
  return Boolean(document.querySelector(stopButtonSelector));
}

function latestAssistantReply() {
  const message = assistantMessages().at(-1);
  if (!message) return { text: '', protocolJson: null };
  const codeNodes = [...message.querySelectorAll('pre code')];
  const fallbackNodes = codeNodes.length ? codeNodes : [...message.querySelectorAll('pre')];
  const codeBlocks = fallbackNodes.map((node) => node.innerText.trim());
  return {
    text: globalThis.restoreProtocolCodeBlock(message.innerText, codeBlocks),
    protocolJson: globalThis.extractProtocolJsonObject(message.innerText, codeBlocks)
  };
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

function waitForCompletedAssistantReply(previousCount, timeoutMs = 60_000) {
  return new Promise((resolve, reject) => {
    const startedAt = Date.now();
    let stableSince = 0;
    const observer = new MutationObserver(check);
    const timer = setInterval(check, 250);

    function finish(error, value) {
      observer.disconnect();
      clearInterval(timer);
      error ? reject(error) : resolve(value);
    }

    function check() {
      const messageCount = assistantMessages().length;
      if (Date.now() - startedAt > timeoutMs) {
        finish(new Error('Timed out waiting for the ChatGPT reply to finish.'));
        return;
      }

      if (messageCount <= previousCount || isGenerating()) {
        stableSince = 0;
        return;
      }

      if (!stableSince) stableSince = Date.now();
      if (Date.now() - stableSince >= 1_000) {
        finish(null, latestAssistantReply());
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

async function sendAndWait(text, includeProtocolJson = false) {
  let stage = 'locating composer';
  try {
    const composer = document.querySelector(composerSelector);
    if (!composer) throw new Error('ChatGPT composer (#prompt-textarea) was not found.');
    stage = 'checking generation state';
    if (isGenerating()) throw new Error('ChatGPT is still generating; wait before starting the smoke test.');

    stage = 'counting existing assistant messages';
    const previousCount = assistantMessages().length;
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

    stage = 'waiting for completed reply';
    const reply = await waitForCompletedAssistantReply(previousCount);
    return includeProtocolJson ? reply : reply.text;
  } catch (error) {
    error.message = `${stage}: ${error.message}`;
    throw error;
  }
}

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'adapterStatusV2') {
    sendResponse({ ok: true, adapterVersion: contentAdapterVersion });
    return;
  }

  if (message?.type === 'adapterProbeV3') {
    const reply = latestAssistantReply();
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

  if (message?.type === 'sendMiddlewareMessage' || message?.type === 'sendMiddlewareMessageV2') {
    sendAndWait(message.text, message.type === 'sendMiddlewareMessageV2')
      .then((response) => sendResponse(typeof response === 'string'
        ? { ok: true, response }
        : { ok: true, response: response.text, protocolJson: response.protocolJson }))
      .catch((error) => sendResponse({ ok: false, error: error.message }));
    return true;
  }
});
})();
