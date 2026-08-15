const runButton = document.querySelector('#run');
const pairButton = document.querySelector('#pair');
const inspectButton = document.querySelector('#inspect');
const pairingSecret = document.querySelector('#pairing-secret');
const result = document.querySelector('#result');

async function activeChatGPTTab() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (!tab?.url?.startsWith('https://chatgpt.com/')) {
    throw new Error('Open a signed-in https://chatgpt.com conversation in the active tab first.');
  }
  return tab;
}

function hasNoReceiver(error) {
  return /Receiving end does not exist/i.test(error?.message ?? '');
}

async function sendToChatGPTTab(tabId, message) {
  try {
    return await chrome.tabs.sendMessage(tabId, message);
  } catch (error) {
    if (!hasNoReceiver(error)) throw error;

    await chrome.scripting.executeScript({
      target: { tabId },
      files: ['protocol-text.js', 'content.js']
    });
    return chrome.tabs.sendMessage(tabId, message);
  }
}

pairButton.addEventListener('click', async () => {
  pairButton.disabled = true;
  try {
    const secret = pairingSecret.value.trim();
    if (!secret) throw new Error('Copy the desktop pairing secret into this field first.');
    const tab = await activeChatGPTTab();
    const response = await chrome.runtime.sendMessage({ type: 'pairDesktop', pairingSecret: secret, tabId: tab.id });
    if (!response?.ok) throw new Error(response?.error || 'Could not start pairing.');
    result.textContent = 'Pairing request sent. Keep this ChatGPT tab open while the middleware runs.';
  } catch (error) {
    result.textContent = `Pairing failed: ${error.message}`;
  } finally {
    pairButton.disabled = false;
  }
});

inspectButton.addEventListener('click', async () => {
  inspectButton.disabled = true;
  result.textContent = 'Checking the extension and the active ChatGPT tab…';
  try {
    const tab = await activeChatGPTTab();
    const response = await chrome.runtime.sendMessage({ type: 'inspectProtocolAdapter', tabId: tab.id });
    if (!response?.ok) throw new Error(response?.error || 'No inspection response from the ChatGPT tab.');
    const inspection = response.inspection;
    result.textContent = [
      `Extension adapter: ${inspection.extensionVersion}`,
      `Tab adapter: ${inspection.adapterVersion}`,
      `Assistant messages: ${inspection.assistantCount}`,
      `Structured JSON in latest reply: ${inspection.protocolJsonPresent ? 'YES' : 'NO'}`
    ].join('\n');
  } catch (error) {
    result.textContent = `Adapter check failed: ${error.message}`;
  } finally {
    inspectButton.disabled = false;
  }
});

runButton.addEventListener('click', async () => {
  runButton.disabled = true;
  result.textContent = 'Sending test message and waiting for ChatGPT to finish…';
  try {
    const tab = await activeChatGPTTab();
    const response = await sendToChatGPTTab(tab.id, { type: 'runSmokeTest' });
    if (!response?.ok) throw new Error(response?.error || 'No response from the ChatGPT tab.');
    result.textContent = `Success:\n${response.response}`;
  } catch (error) {
    result.textContent = `Failed: ${error.message}`;
  } finally {
    runButton.disabled = false;
  }
});
