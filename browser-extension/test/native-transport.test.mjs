import assert from "node:assert/strict";
import test from "node:test";

import { NativeMapTransport } from "../native-transport.js";

class EventHook {
  listeners = [];
  addListener(listener) { this.listeners.push(listener); }
  emit(value) { for (const listener of this.listeners) listener(value); }
}

function fakeRuntime() {
  const fake = {
    ports: [],
    get port() { return this.ports.at(-1); },
    runtime: {
      lastError: null,
      connectNative(hostName) {
        assert.equal(hostName, "org.openworkforceindex.project_map");
        const port = {
          sent: [],
          disconnected: false,
          onMessage: new EventHook(),
          onDisconnect: new EventHook(),
          postMessage(message) { this.sent.push(message); },
          disconnect() {
            this.disconnected = true;
            this.onDisconnect.emit();
          },
        };
        fake.ports.push(port);
        return port;
      },
    },
  };
  return fake;
}

test("correlates one bounded native request and response", async () => {
  const fake = fakeRuntime();
  const transport = new NativeMapTransport(fake.runtime, { timeoutMs: 100 });
  const pending = transport.request({ id: "request:1", type: "list_projects" });
  assert.deepEqual(fake.port.sent, [{ id: "request:1", type: "list_projects" }]);
  fake.port.onMessage.emit({
    schemaVersion: "owi.native-project-map.v1",
    id: "request:1",
    ok: true,
    result: { projects: [{ projectId: "sample" }] },
  });
  assert.deepEqual(await pending, { projects: [{ projectId: "sample" }] });
  assert.equal(fake.port.disconnected, true);
});

test("rejects pending work when the native host disconnects", async () => {
  const fake = fakeRuntime();
  const transport = new NativeMapTransport(fake.runtime, { timeoutMs: 100 });
  const pending = transport.request({ id: "request:2", type: "list_projects" });
  fake.runtime.lastError = { message: "Host is not registered" };
  fake.port.onDisconnect.emit();
  await assert.rejects(pending, /Host is not registered/);
});

test("a timeout closes the port and the next request starts a fresh host", async () => {
  const fake = fakeRuntime();
  const transport = new NativeMapTransport(fake.runtime, { timeoutMs: 5 });
  await assert.rejects(
    transport.request({ id: "request:timeout", type: "list_projects" }),
    /did not respond/,
  );
  assert.equal(fake.ports.length, 1);
  assert.equal(fake.ports[0].disconnected, true);

  const pending = transport.request({ id: "request:fresh", type: "list_projects" });
  assert.equal(fake.ports.length, 2);
  fake.port.onMessage.emit({
    schemaVersion: "owi.native-project-map.v1",
    id: "request:fresh",
    ok: true,
    result: { projects: [] },
  });
  assert.deepEqual(await pending, { projects: [] });
});
