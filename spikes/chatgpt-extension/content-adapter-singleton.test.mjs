import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('./content.js', import.meta.url), 'utf8');
const listeners = [];
const context = vm.createContext({
  chrome: {
    runtime: {
      onMessage: {
        addListener(listener) {
          listeners.push(listener);
        }
      }
    }
  }
});

vm.runInContext(source, context);
vm.runInContext(source, context);

assert.equal(
  listeners.length,
  1,
  'reinjecting content.js into the same tab must not register a second message listener',
);
