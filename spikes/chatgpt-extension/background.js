const bridgeUrl = 'ws://127.0.0.1:8765';
const contentAdapterVersion = '1.3.0';
let socket = null;
let sessionId = null;
let pairedTabId = null;
let pairingSecret = null;
let reconnectTimer = null;
let heartbeatTimer = null;

function relayTrace(requestId, stage, detail = undefined) {
  console.info('[relay-trace]', { requestId, stage, ...(detail ? { detail } : {}) });
}

function hasNoReceiver(error) {
  return /Receiving end does not exist/i.test(error?.message ?? '');
}

async function sendToChatGPTTab(tabId, message) {
  try {
    return await chrome.tabs.sendMessage(tabId, message);
  } catch (error) {
    if (!hasNoReceiver(error)) throw error;
    await chrome.scripting.executeScript({ target: { tabId }, files: ['protocol-text.js', 'content.js'] });
    return chrome.tabs.sendMessage(tabId, message);
  }
}

async function ensureLatestContentAdapter(tabId) {
  let status;
  try {
    status = await chrome.tabs.sendMessage(tabId, { type: 'adapterStatusV3' });
  } catch (error) {
    if (!hasNoReceiver(error)) throw error;
  }

  if (status?.ok && status.adapterVersion === contentAdapterVersion) return;

  await chrome.scripting.executeScript({
    target: { tabId },
    files: ['protocol-text.js', 'content.js']
  });
  status = await chrome.tabs.sendMessage(tabId, { type: 'adapterStatusV3' });
  if (!status?.ok || status.adapterVersion !== contentAdapterVersion) {
    throw new Error('The current ChatGPT tab did not load the required protocol adapter version.');
  }
}

async function inspectProtocolAdapter(tabId) {
  await ensureLatestContentAdapter(tabId);
  const probe = await chrome.tabs.sendMessage(tabId, { type: 'adapterProbeV4' });
  if (!probe?.ok || probe.adapterVersion !== contentAdapterVersion) {
    throw new Error('The ChatGPT tab did not return a valid adapter inspection result.');
  }
  return { extensionVersion: contentAdapterVersion, ...probe };
}

async function savePairing(pairingSecret, tabId) {
  await chrome.storage.local.set({ pairingSecret, pairedTabId: tabId });
}

function connectionStatus() {
  return {
    connected: socket?.readyState === WebSocket.OPEN,
    paired: Boolean(sessionId),
    tabId: pairedTabId
  };
}

function stopHeartbeat() {
  if (heartbeatTimer) clearInterval(heartbeatTimer);
  heartbeatTimer = null;
}

function startHeartbeat() {
  stopHeartbeat();
  heartbeatTimer = setInterval(() => {
    if (socket?.readyState === WebSocket.OPEN && sessionId) {
      socket.send(JSON.stringify({ type: 'keepAlive', sessionId }));
    }
  }, 15_000);
}

function scheduleReconnect() {
  if (!pairingSecret || !pairedTabId || reconnectTimer) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect(pairingSecret, pairedTabId).catch(() => scheduleReconnect());
  }, 2_000);
}

async function connect(secret, tabId) {
  if (socket) socket.close();
  pairingSecret = secret;
  sessionId = null;
  pairedTabId = tabId;
  stopHeartbeat();
  await savePairing(secret, tabId);

  const nextSocket = new WebSocket(bridgeUrl);
  socket = nextSocket;
  nextSocket.addEventListener('open', () => {
    nextSocket.send(JSON.stringify({ type: 'pair', pairingSecret: secret, tabId }));
  });
  nextSocket.addEventListener('message', async (event) => {
    const message = JSON.parse(event.data);
    if (message.type === 'paired') {
      sessionId = message.sessionId;
      startHeartbeat();
      return;
    }
    if (message.type === 'sendChatGptMessage' && message.sessionId === sessionId) {
      const requestId = typeof message.requestId === 'string' ? message.requestId : 'unknown';
      relayTrace(requestId, 'BACKGROUND_RECEIVED');
      try {
        await ensureLatestContentAdapter(pairedTabId);
        relayTrace(requestId, 'TAB_DISPATCH', { pairedTabId });
        const response = await chrome.tabs.sendMessage(pairedTabId, {
          type: 'sendMiddlewareMessageV3',
          text: message.text,
          includeProtocolJson: !message.relay,
          requestId
        });
        if (!response?.ok) {
          const error = new Error(response?.error || 'ChatGPT tab did not return a reply.');
          error.adapterDiagnostic = response?.adapterDiagnostic ?? null;
          throw error;
        }
        const reply = {
          type: 'chatgptReply',
          sessionId,
          requestId,
          text: response.response,
          protocolJson: typeof response.protocolJson === 'string' ? response.protocolJson : undefined,
          protocolJsonPresent: Boolean(response.protocolJsonPresent),
          adapterVersion: response.adapterVersion ?? contentAdapterVersion,
          relay: Boolean(message.relay)
        };
        relayTrace(requestId, 'CHATGPT_REPLY_SENT');
        socket?.send(JSON.stringify(reply));
      } catch (error) {
        const reply = {
          type: 'chatgptReply',
          sessionId,
          requestId,
          text: `Protocol adapter failed before a reply was received: ${error.message}`,
          protocolJsonPresent: false,
          adapterVersion: contentAdapterVersion,
          adapterError: error.message,
          adapterDiagnostic: error.adapterDiagnostic ?? null,
          relay: Boolean(message.relay)
        };
        relayTrace(requestId, 'CHATGPT_REPLY_SENT', { adapterError: true });
        socket?.send(JSON.stringify(reply));
      }
    } else if (message.type === 'sendChatGptMessage') {
      relayTrace(typeof message.requestId === 'string' ? message.requestId : 'unknown', 'SESSION_MISMATCH');
    }
  });
  nextSocket.addEventListener('close', () => {
    if (socket !== nextSocket) return;
    sessionId = null;
    stopHeartbeat();
    scheduleReconnect();
  });
  nextSocket.addEventListener('error', () => undefined);
}

async function restorePairing() {
  const { pairingSecret, pairedTabId: tabId } = await chrome.storage.local.get(['pairingSecret', 'pairedTabId']);
  if (pairingSecret && Number.isInteger(tabId) && tabId > 0) {
    connect(pairingSecret, tabId).catch(() => undefined);
  }
}

chrome.runtime.onStartup.addListener(restorePairing);
chrome.runtime.onInstalled.addListener(restorePairing);

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message?.type === 'pairDesktop') {
    ensureLatestContentAdapter(message.tabId)
      .then(() => connect(message.pairingSecret, message.tabId))
      .then(() => sendResponse({ ok: true }))
      .catch((error) => sendResponse({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === 'inspectProtocolAdapter') {
    inspectProtocolAdapter(message.tabId)
      .then((inspection) => sendResponse({ ok: true, inspection }))
      .catch((error) => sendResponse({ ok: false, error: error.message }));
    return true;
  }
  if (message?.type === 'bridgeStatus') {
    sendResponse({ ok: true, ...connectionStatus() });
  }
});
