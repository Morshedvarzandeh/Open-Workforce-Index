import assert from "node:assert/strict";
import test from "node:test";

import {
  CLIENT_CHANNEL,
  MAX_OUTBOUND_BYTES,
  ProtocolError,
  assertWireSize,
  createInvokeActionRequest,
  parseClientMessage,
  parseNativeResponse,
} from "../protocol.js";

const actionInput = {
  id: "request:42",
  projectId: "project:sample",
  actionId: "action:pause-learning",
  expectedSnapshotId: "snapshot:42",
  expectedState: "state:running",
  idempotencyKey: "action_0123456789abcdef",
  confirmed: true,
};

test("builds the closed action envelope after explicit confirmation", () => {
  assert.deepEqual(createInvokeActionRequest(actionInput), {
    ...actionInput,
    type: "invoke_action",
  });
});

test("action messages cannot carry commands, paths, or arbitrary arguments", () => {
  assert.throws(
    () => createInvokeActionRequest({ ...actionInput, command: "echo unsafe" }),
    /unsupported fields: command/,
  );
  assert.throws(
    () => createInvokeActionRequest({ ...actionInput, path: "src/file.js" }),
    /unsupported fields: path/,
  );
  assert.throws(
    () => createInvokeActionRequest({ ...actionInput, confirmed: false }),
    /explicit confirmation/,
  );
  assert.throws(
    () =>
      parseClientMessage({
        channel: CLIENT_CHANNEL,
        type: "invoke_action",
        ...actionInput,
        id: undefined,
        args: ["anything"],
      }),
    /unsupported fields/,
  );
});

test("parses only versioned native responses with matching IDs", () => {
  const response = {
    schemaVersion: "owi.native-project-map.v1",
    id: "request:42",
    ok: true,
    result: { projects: [] },
  };
  assert.deepEqual(parseNativeResponse(response, "request:42"), { projects: [] });
  assert.throws(
    () => parseNativeResponse({ ...response, schemaVersion: "owi.native-project-map.v2" }, "request:42"),
    /Unsupported native response schema/,
  );
  assert.throws(
    () => parseNativeResponse(response, "request:99"),
    /does not match/,
  );
});

test("surfaces bounded native errors without accepting extra fields", () => {
  assert.throws(
    () =>
      parseNativeResponse(
        {
          schemaVersion: "owi.native-project-map.v1",
          id: "request:42",
          ok: false,
          error: { code: "stale_state", message: "State changed" },
        },
        "request:42",
      ),
    (error) => error instanceof ProtocolError && error.code === "stale_state",
  );
  assert.throws(
    () => assertWireSize({ text: "x".repeat(MAX_OUTBOUND_BYTES) }, MAX_OUTBOUND_BYTES),
    /exceeds/,
  );
});
