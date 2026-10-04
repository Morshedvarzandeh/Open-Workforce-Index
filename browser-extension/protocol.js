export const CLIENT_CHANNEL = "owi.project-map.client.v1";
export const NATIVE_SCHEMA_VERSION = "owi.native-project-map.v1";
export const NATIVE_HOST_NAME = "org.openworkforceindex.project_map";
export const MAX_OUTBOUND_BYTES = 64 * 1024;
export const MAX_INBOUND_BYTES = 1024 * 1024;

const OPAQUE_ID = /^[A-Za-z0-9][A-Za-z0-9._:@-]{0,255}$/;
const STATE = /^[a-z][a-z0-9._:-]{0,127}$/;

export class ProtocolError extends Error {
  constructor(message, code = "invalid_client_message") {
    super(message);
    this.name = "ProtocolError";
    this.code = code;
  }
}

function isPlainRecord(value) {
  return (
    value !== null &&
    typeof value === "object" &&
    !Array.isArray(value) &&
    (Object.getPrototypeOf(value) === Object.prototype ||
      Object.getPrototypeOf(value) === null)
  );
}

function assertRecord(value, label) {
  if (!isPlainRecord(value)) {
    throw new ProtocolError(`${label} must be an object`);
  }
}

function assertExactKeys(value, allowed, label) {
  assertRecord(value, label);
  const extras = Object.keys(value).filter((key) => !allowed.includes(key));
  if (extras.length > 0) {
    throw new ProtocolError(`${label} has unsupported fields: ${extras.join(", ")}`);
  }
}

function assertOpaqueId(value, label) {
  if (typeof value !== "string" || !OPAQUE_ID.test(value)) {
    throw new ProtocolError(`${label} must be an opaque identifier`);
  }
  return value;
}

function assertState(value, label) {
  if (typeof value !== "string" || !STATE.test(value)) {
    throw new ProtocolError(`${label} must be a state token`);
  }
  return value;
}

export function assertWireSize(value, maximumBytes, label = "message") {
  let encoded;
  try {
    encoded = JSON.stringify(value);
  } catch {
    throw new ProtocolError(`${label} is not serializable`);
  }
  if (typeof encoded !== "string") {
    throw new ProtocolError(`${label} is not serializable`);
  }
  const bytes = new TextEncoder().encode(encoded).byteLength;
  if (bytes > maximumBytes) {
    throw new ProtocolError(`${label} exceeds the ${maximumBytes}-byte limit`, "message_too_large");
  }
  return bytes;
}

export function createListProjectsRequest(id) {
  return {
    id: assertOpaqueId(id, "request id"),
    type: "list_projects",
  };
}

export function createGetSnapshotRequest({ id, projectId }) {
  assertExactKeys(arguments[0], ["id", "projectId"], "snapshot request");
  return {
    id: assertOpaqueId(id, "request id"),
    type: "get_snapshot",
    projectId: assertOpaqueId(projectId, "project id"),
  };
}

export function createInvokeActionRequest(input) {
  assertExactKeys(
    input,
    [
      "id",
      "projectId",
      "actionId",
      "expectedSnapshotId",
      "expectedState",
      "idempotencyKey",
      "confirmed",
    ],
    "action request",
  );
  if (input.confirmed !== true) {
    throw new ProtocolError("Action invocation requires explicit confirmation");
  }
  return {
    id: assertOpaqueId(input.id, "request id"),
    type: "invoke_action",
    projectId: assertOpaqueId(input.projectId, "project id"),
    actionId: assertOpaqueId(input.actionId, "action id"),
    expectedSnapshotId: assertOpaqueId(
      input.expectedSnapshotId,
      "expected snapshot id",
    ),
    expectedState: assertState(input.expectedState, "expected state"),
    idempotencyKey: assertOpaqueId(input.idempotencyKey, "idempotency key"),
    confirmed: true,
  };
}

export function parseClientMessage(message) {
  assertRecord(message, "client message");
  if (message.channel !== CLIENT_CHANNEL) {
    throw new ProtocolError("Unsupported client channel");
  }
  if (message.type === "list_projects") {
    assertExactKeys(message, ["channel", "type"], "list-projects message");
    return { type: "list_projects" };
  }
  if (message.type === "get_snapshot") {
    assertExactKeys(
      message,
      ["channel", "type", "projectId"],
      "get-snapshot message",
    );
    return {
      type: "get_snapshot",
      projectId: assertOpaqueId(message.projectId, "project id"),
    };
  }
  if (message.type === "invoke_action") {
    assertExactKeys(
      message,
      [
        "channel",
        "type",
        "projectId",
        "actionId",
        "expectedSnapshotId",
        "expectedState",
        "idempotencyKey",
        "confirmed",
      ],
      "invoke-action message",
    );
    if (message.confirmed !== true) {
      throw new ProtocolError("Action invocation requires explicit confirmation");
    }
    return {
      type: "invoke_action",
      projectId: assertOpaqueId(message.projectId, "project id"),
      actionId: assertOpaqueId(message.actionId, "action id"),
      expectedSnapshotId: assertOpaqueId(
        message.expectedSnapshotId,
        "expected snapshot id",
      ),
      expectedState: assertState(message.expectedState, "expected state"),
      idempotencyKey: assertOpaqueId(message.idempotencyKey, "idempotency key"),
      confirmed: true,
    };
  }
  throw new ProtocolError("Unsupported client message type");
}

export function parseNativeResponse(message, expectedId) {
  assertRecord(message, "native response");
  assertWireSize(message, MAX_INBOUND_BYTES, "native response");
  if (message.schemaVersion !== NATIVE_SCHEMA_VERSION) {
    throw new ProtocolError("Unsupported native response schema version");
  }
  if (message.id !== expectedId) {
    throw new ProtocolError("Native response id does not match the request", "response_mismatch");
  }
  if (message.ok === true) {
    assertExactKeys(
      message,
      ["schemaVersion", "id", "ok", "result"],
      "successful native response",
    );
    return message.result;
  }
  if (message.ok === false) {
    assertExactKeys(
      message,
      ["schemaVersion", "id", "ok", "error"],
      "failed native response",
    );
    assertRecord(message.error, "native error");
    const code =
      typeof message.error.code === "string" ? message.error.code : "native_error";
    const detail =
      typeof message.error.message === "string"
        ? message.error.message.slice(0, 500)
        : "The local OWI host rejected the request";
    throw new ProtocolError(detail, code);
  }
  throw new ProtocolError("Native response is missing an outcome");
}
