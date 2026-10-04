import assert from "node:assert/strict";
import test from "node:test";

class EventHook {
  listeners = [];
  addListener(listener) { this.listeners.push(listener); }
  emit(...values) { for (const listener of this.listeners) listener(...values); }
}

function settle() {
  return new Promise((resolve) => setTimeout(resolve, 0));
}

test("toolbar clicks reuse and focus one dashboard tab", async () => {
  const action = new EventHook();
  const removed = new EventHook();
  const onMessage = new EventHook();
  const storage = {};
  const tabs = new Map();
  const calls = { create: 0, update: 0, focus: 0 };
  globalThis.chrome = {
    runtime: {
      id: "extension-id",
      lastError: null,
      connectNative() { throw new Error("not used by this test"); },
      getURL(path) { return `chrome-extension://extension-id/${path}`; },
      onMessage,
    },
    storage: {
      local: {
        get(key, callback) { callback({ [key]: storage[key] }); },
        set(value, callback) { Object.assign(storage, value); callback(); },
      },
    },
    tabs: {
      onRemoved: removed,
      get(tabId, callback) { callback(tabs.get(tabId)); },
      update(tabId, properties, callback) {
        calls.update += 1;
        const value = { ...tabs.get(tabId), ...properties };
        tabs.set(tabId, value);
        callback(value);
      },
      create(properties, callback) {
        calls.create += 1;
        const value = { id: 42, windowId: 7, ...properties };
        tabs.set(value.id, value);
        callback(value);
      },
    },
    windows: {
      update(_windowId, _properties, callback) { calls.focus += 1; callback(); },
    },
    action: { onClicked: action },
  };

  await import(`../background.js?singleton=${Date.now()}`);
  action.emit();
  action.emit();
  await settle();
  await settle();
  assert.equal(calls.create, 1);

  action.emit();
  await settle();
  await settle();
  assert.equal(calls.create, 1);
  assert.equal(calls.update, 1);
  assert.equal(calls.focus, 1);
  delete globalThis.chrome;
});
