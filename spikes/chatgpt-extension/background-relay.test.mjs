import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('./background.js', import.meta.url), 'utf8');
const socketHandlers = new Map();
const runtimeListeners = [];
const tabMessages = [];
const socketSent = [];

class FakeWebSocket {
  static OPEN = 1;

  constructor() {
    this.readyState = FakeWebSocket.OPEN;
  }

  addEventListener(type, listener) {
    socketHandlers.set(type, listener);
  }

  send(message) {
    socketSent.push(JSON.parse(message));
  }

  close() {}
}

const chrome = {
  runtime: {
    onStartup: { addListener() {} },
    onInstalled: { addListener() {} },
    onMessage: { addListener(listener) { runtimeListeners.push(listener); } },
  },
  storage: { local: { async get() { return {}; }, async set() {} } },
  scripting: { async executeScript() { throw new Error('adapter injection was not expected'); } },
  tabs: {
    async sendMessage(tabId, message) {
      tabMessages.push({ tabId, message });
      if (message.type === 'adapterStatusV3') return { ok: true, adapterVersion: '1.3.1' };
      if (message.type === 'sendMiddlewareMessageV3') return { ok: true, response: 'relay reply', adapterVersion: '1.3.1' };
      throw new Error(`unexpected tab message: ${message.type}`);
    },
  },
};

const context = vm.createContext({ chrome, WebSocket: FakeWebSocket, console, setTimeout, clearTimeout, setInterval, clearInterval, JSON });
vm.runInContext(source, context);
assert.equal(runtimeListeners.length, 1, 'background must register its popup listener');

await context.connect('test-secret', 73);
await socketHandlers.get('open')();
await socketHandlers.get('message')({ data: JSON.stringify({ type: 'paired', sessionId: 'session-1' }) });
await socketHandlers.get('message')({ data: JSON.stringify({
  type: 'sendChatGptMessage', sessionId: 'session-1', requestId: 'request-1', text: 'hello', relay: true,
}) });

const relayDispatches = tabMessages.filter(({ message }) => message.type === 'sendMiddlewareMessageV3');
assert.deepEqual(JSON.parse(JSON.stringify(relayDispatches)), [{
  tabId: 73,
  message: { type: 'sendMiddlewareMessageV3', text: 'hello', includeProtocolJson: false, requestId: 'request-1' },
}], 'relay dispatch must use the paired tab and isolated message type');
assert.deepEqual(JSON.parse(JSON.stringify(socketSent.at(-1))), {
  type: 'chatgptReply', sessionId: 'session-1', requestId: 'request-1', text: 'relay reply',
  protocolJsonPresent: false, adapterVersion: '1.3.1', relay: true,
}, 'a successful relay adapter response must emit exactly one correlated reply');

const replyCount = socketSent.length;
await socketHandlers.get('message')({ data: JSON.stringify({
  type: 'sendChatGptMessage', sessionId: 'stale-session', requestId: 'request-2', text: 'ignored', relay: true,
}) });
assert.equal(socketSent.length, replyCount, 'a stale session must not dispatch a tab message or emit a reply');

context.stopHeartbeat();
console.log('BACKGROUND_RELAY_SMOKE_OK');
