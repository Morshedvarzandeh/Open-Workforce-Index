const LIFECYCLE_STATES = new Set(["implemented", "declared", "planned"]);
const IMPACT_STATES = new Set([
  "changed",
  "affected",
  "unchanged",
  "unavailable",
]);
const FILE_KINDS = new Set(["text", "binary", "unavailable"]);
const NODE_KINDS = new Set([
  "surface",
  "api",
  "router",
  "agent",
  "runtime",
  "trigger",
  "ledger",
  "human",
  "module",
]);
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9._:@-]{0,255}$/;
const REPOSITORY_SEGMENT = /^(?!\.\.?$)[^/\\\0]+$/;
const FORBIDDEN_ACTION_FIELDS = new Set([
  "args",
  "arguments",
  "command",
  "commandline",
  "credentials",
  "cwd",
  "environment",
  "executable",
  "output",
  "path",
  "prompt",
  "provider",
  "repositorypath",
  "secret",
  "shell",
  "url",
  "workingdirectory",
]);
const DEFINITIVE_ACTION_REJECTIONS = new Set([
  "confirmation_required",
  "idempotency_conflict",
  "invalid_request",
  "unknown_project",
  "stale_snapshot",
  "stale_state",
  "unknown_action",
  "action_unavailable",
  "action_failed",
]);

export class SnapshotError extends Error {
  constructor(message) {
    super(message);
    this.name = "SnapshotError";
  }
}

export function actionFailureOutcome(code) {
  return DEFINITIVE_ACTION_REJECTIONS.has(code) ? "not-applied" : "unknown";
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function record(value, label) {
  if (!isRecord(value)) throw new SnapshotError(`${label} must be an object`);
  return value;
}

function array(value, label) {
  if (!Array.isArray(value)) throw new SnapshotError(`${label} must be an array`);
  return value;
}

function identifier(value, label) {
  if (typeof value !== "string" || !IDENTIFIER.test(value)) {
    throw new SnapshotError(`${label} must be a stable identifier`);
  }
  return value;
}

function shortText(value, label, maximum = 1000) {
  if (typeof value !== "string") throw new SnapshotError(`${label} must be text`);
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length > maximum || normalized.includes("\0")) {
    throw new SnapshotError(`${label} is not valid display text`);
  }
  return normalized;
}

function optionalText(value, label, maximum = 1000) {
  if (value === undefined || value === null || value === "") return "";
  return shortText(value, label, maximum);
}

function optionalCount(value, label) {
  if (value === undefined || value === null) return null;
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new SnapshotError(`${label} must be a non-negative integer or null`);
  }
  return value;
}

function unique(items, label, getId = (item) => item.id) {
  const seen = new Set();
  for (const item of items) {
    const id = getId(item);
    if (seen.has(id)) throw new SnapshotError(`${label} contains duplicate id ${id}`);
    seen.add(id);
  }
  return items;
}

function exactKeys(value, expected, label) {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, index) => key !== wanted[index])) {
    throw new SnapshotError(`${label} has unexpected fields`);
  }
}

function repositoryPath(value, label) {
  if (typeof value !== "string" || value.length === 0 || value.length > 512) {
    throw new SnapshotError(`${label} must be a repository-relative path`);
  }
  if (value.startsWith("/") || value.startsWith("~") || /^[A-Za-z]:/.test(value)) {
    throw new SnapshotError(`${label} must not be absolute`);
  }
  const parts = value.split("/");
  if (parts.some((part) => !REPOSITORY_SEGMENT.test(part))) {
    throw new SnapshotError(`${label} is not a normalized repository-relative path`);
  }
  return value;
}

function normalizeDependencyPaths(value, nodeId) {
  if (value === undefined || value === null) return [];
  return array(value, `impact paths for ${nodeId}`).map((path, pathIndex) =>
    array(path, `impact path ${pathIndex} for ${nodeId}`).map((part, partIndex) =>
      identifier(part, `impact path ${pathIndex} item ${partIndex}`),
    ),
  );
}

function normalizeChurn(value, direct, nodeId) {
  const churn = value === undefined ? {} : record(value, `churn for ${nodeId}`);
  const counts = {
    additions: optionalCount(churn.additions, `additions for ${nodeId}`),
    deletions: optionalCount(churn.deletions, `deletions for ${nodeId}`),
    changedFiles: optionalCount(churn.changedFiles, `changed files for ${nodeId}`),
    textFiles: optionalCount(churn.textFiles, `text files for ${nodeId}`),
    binaryFiles: optionalCount(churn.binaryFiles, `binary files for ${nodeId}`),
    unavailableFiles: optionalCount(
      churn.unavailableFiles,
      `unavailable files for ${nodeId}`,
    ),
  };
  if (!direct) {
    counts.additions = null;
    counts.deletions = null;
    counts.changedFiles = null;
    counts.textFiles = null;
    counts.binaryFiles = null;
    counts.unavailableFiles = null;
  }
  return counts;
}

function normalizeLane(value, index) {
  const lane = record(value, `lane ${index}`);
  return {
    id: identifier(lane.id, `lane ${index} id`),
    name: shortText(lane.name ?? lane.label, `lane ${index} name`, 120),
    description: optionalText(lane.description, `lane ${index} description`, 400),
    order: Number.isFinite(lane.order) ? lane.order : index,
  };
}

function normalizeNode(value, index, laneIds) {
  const node = record(value, `node ${index}`);
  const id = identifier(node.id, `node ${index} id`);
  const laneId = identifier(node.laneId, `lane id for ${id}`);
  if (!laneIds.has(laneId)) {
    throw new SnapshotError(`node ${id} references unknown lane ${laneId}`);
  }
  const lifecycle = node.lifecycle ?? "declared";
  if (!LIFECYCLE_STATES.has(lifecycle)) {
    throw new SnapshotError(`node ${id} has unsupported lifecycle ${lifecycle}`);
  }
  const impact = record(node.impact ?? {}, `impact for ${id}`);
  if (!NODE_KINDS.has(node.kind)) {
    throw new SnapshotError(`node ${id} has unsupported kind ${node.kind}`);
  }
  const impactState = impact.state ?? "unavailable";
  if (!IMPACT_STATES.has(impactState)) {
    throw new SnapshotError(`node ${id} has unsupported impact ${impactState}`);
  }
  const direct = impactState === "changed";
  const sourceRefs = array(node.sourceRefs ?? [], `source references for ${id}`).map(
    (sourceRef, sourceIndex) => {
      const item = record(sourceRef, `source reference ${sourceIndex} for ${id}`);
      if (item.kind !== "exact" && item.kind !== "prefix") {
        throw new SnapshotError(`source reference ${sourceIndex} for ${id} has an invalid kind`);
      }
      return {
        path: repositoryPath(item.path, `source reference ${sourceIndex} for ${id}`),
        kind: item.kind,
      };
    },
  );
  return {
    id,
    name: shortText(node.name ?? node.label, `name for ${id}`, 160),
    laneId,
    description: optionalText(node.description, `description for ${id}`, 800),
    kind: node.kind,
    facts: array(node.facts ?? [], `facts for ${id}`).map((fact, factIndex) =>
      shortText(fact, `fact ${factIndex} for ${id}`, 240),
    ),
    lifecycle,
    sourceRefs,
    impact: {
      state: impactState,
      direct,
      directFileCount: Array.isArray(impact.directFileIds)
        ? impact.directFileIds.length
        : Array.isArray(impact.direct)
          ? impact.direct.length
          : direct && Number.isSafeInteger(impact.directFileCount)
            ? impact.directFileCount
            : null,
      paths: normalizeDependencyPaths(impact.paths, id),
    },
    churn: normalizeChurn(node.churn, direct, id),
  };
}

function normalizeEdge(value, index, nodeIds) {
  const edge = record(value, `edge ${index}`);
  const id = identifier(edge.id, `edge ${index} id`);
  const from = identifier(edge.from, `source for ${id}`);
  const to = identifier(edge.to, `target for ${id}`);
  if (!nodeIds.has(from) || !nodeIds.has(to)) {
    throw new SnapshotError(`edge ${id} references an unknown node`);
  }
  return {
    id,
    from,
    to,
    type: shortText(edge.type ?? "depends-on", `type for ${id}`, 80),
    label: optionalText(edge.label, `label for ${id}`, 120),
  };
}

function normalizeFile(value, index, nodeIds) {
  const file = record(value, `file ${index}`);
  const kind = file.kind ?? "unavailable";
  if (!FILE_KINDS.has(kind)) {
    throw new SnapshotError(`file ${index} has unsupported kind ${kind}`);
  }
  const componentIds = array(file.componentIds ?? [], `component ids for file ${index}`).map(
    (componentId) => identifier(componentId, `component id for file ${index}`),
  );
  if (componentIds.some((componentId) => !nodeIds.has(componentId))) {
    throw new SnapshotError(`file ${index} references an unknown component`);
  }
  return {
    path: repositoryPath(file.path, `file ${index} path`),
    status: optionalText(file.status, `file ${index} status`, 40) || "unknown",
    kind,
    additions: kind === "text" ? optionalCount(file.additions, `file ${index} additions`) : null,
    deletions: kind === "text" ? optionalCount(file.deletions, `file ${index} deletions`) : null,
    componentIds,
  };
}

function normalizeEvidenceValue(value, evidenceId) {
  if (value === null || value === undefined) return null;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) throw new SnapshotError(`evidence ${evidenceId} value is invalid`);
    return { amount: value, unit: "" };
  }
  if (typeof value === "boolean") return { text: value ? "Yes" : "No" };
  if (typeof value === "string") {
    return { text: shortText(value, `value for ${evidenceId}`, 180) };
  }
  const structured = record(value, `value for ${evidenceId}`);
  const amount = structured.amount ?? structured.value;
  if (amount !== null && amount !== undefined && !Number.isFinite(amount)) {
    throw new SnapshotError(`amount for ${evidenceId} is invalid`);
  }
  const metrics = {};
  for (const [key, metric] of Object.entries(structured)) {
    if (!["amount", "value", "unit", "currency"].includes(key) &&
        /^[A-Za-z][A-Za-z0-9]{0,63}$/.test(key) &&
        (typeof metric === "number" || typeof metric === "boolean" || metric === null)) {
      if (typeof metric === "number" && !Number.isFinite(metric)) {
        throw new SnapshotError(`metric ${key} for ${evidenceId} is invalid`);
      }
      metrics[key] = metric;
    }
  }
  return {
    amount: amount ?? null,
    unit: optionalText(structured.unit, `unit for ${evidenceId}`, 40),
    currency: optionalText(structured.currency, `currency for ${evidenceId}`, 12),
    metrics,
  };
}

function normalizeEvidence(value, index) {
  const evidence = record(value, `evidence ${index}`);
  const id = identifier(evidence.id, `evidence ${index} id`);
  return {
    id,
    type: shortText(evidence.type, `type for ${id}`, 100),
    claimState: shortText(evidence.claimState, `claim state for ${id}`, 80),
    basis: shortText(
      typeof evidence.basis === "string" ? evidence.basis : evidence.basis?.type,
      `basis for ${id}`,
      100,
    ),
    subjectId: identifier(evidence.subjectId, `subject for ${id}`),
    value: normalizeEvidenceValue(evidence.value, id),
  };
}

function assertNoActionAuthorityPayload(value, trail = "action") {
  if (Array.isArray(value)) {
    value.forEach((nested, index) =>
      assertNoActionAuthorityPayload(nested, `${trail}[${index}]`),
    );
    return;
  }
  if (!isRecord(value)) return;
  for (const [key, nested] of Object.entries(value)) {
    if (FORBIDDEN_ACTION_FIELDS.has(key.toLowerCase())) {
      throw new SnapshotError(`${trail} contains forbidden authority field ${key}`);
    }
    assertNoActionAuthorityPayload(nested, `${trail}.${key}`);
  }
}

function normalizeAction(value, index) {
  const action = record(value, `action ${index}`);
  assertNoActionAuthorityPayload(action, `action ${index}`);
  const id = identifier(action.id, `action ${index} id`);
  const availability =
    action.availability === true ||
    action.availability === "available" ||
    action.available === true;
  return {
    id,
    kind: optionalText(action.kind, `kind for ${id}`, 100),
    label: shortText(action.label, `label for ${id}`, 120),
    description: optionalText(action.description, `description for ${id}`, 500),
    authority: shortText(action.authority, `authority for ${id}`, 100),
    approval: shortText(action.approval, `approval for ${id}`, 100),
    availability,
    unavailableReason: optionalText(
      action.unavailableReason,
      `unavailable reason for ${id}`,
      240,
    ),
    expectedState: identifier(action.expectedState, `expected state for ${id}`),
    targetId: optionalText(action.targetId, `target for ${id}`, 160),
  };
}

function normalizeAdapters(value) {
  if (value === undefined || value === null) return [];
  if (Array.isArray(value)) {
    return value.map((adapter, index) => {
      if (typeof adapter === "string") return shortText(adapter, `adapter ${index}`, 100);
      const item = record(adapter, `adapter ${index}`);
      return shortText(item.id ?? item.name, `adapter ${index} name`, 100);
    });
  }
  const adapters = record(value, "snapshot adapters");
  return Object.keys(adapters).map((name, index) => shortText(name, `adapter ${index}`, 100));
}

export function normalizeSnapshot(input) {
  const raw = record(input, "snapshot payload");
  if (raw.schemaVersion !== "owi.system-map.v1") {
    throw new SnapshotError("Unsupported project-map schema version");
  }
  const metadata = record(raw.snapshot, "snapshot metadata");
  const lanes = unique(array(raw.lanes, "lanes").map(normalizeLane), "lanes").sort(
    (left, right) => left.order - right.order,
  );
  const laneIds = new Set(lanes.map((lane) => lane.id));
  const nodes = unique(
    array(raw.nodes, "nodes").map((node, index) => normalizeNode(node, index, laneIds)),
    "nodes",
  );
  const nodeIds = new Set(nodes.map((node) => node.id));
  for (const node of nodes) {
    for (const path of node.impact.paths) {
      if (path.some((part) => !nodeIds.has(part))) {
        throw new SnapshotError(`impact path for ${node.id} references an unknown node`);
      }
    }
  }
  const edges = unique(
    array(raw.edges, "edges").map((edge, index) => normalizeEdge(edge, index, nodeIds)),
    "edges",
  );
  const files = array(raw.files ?? [], "files").map((file, index) =>
    normalizeFile(file, index, nodeIds),
  );
  const unmappedFiles = array(raw.unmappedFiles ?? [], "unmapped files").map(
    (path, index) => repositoryPath(path, `unmapped file ${index}`),
  );
  const evidence = unique(
    array(raw.evidence ?? [], "evidence").map(normalizeEvidence),
    "evidence",
  );
  const actions = unique(
    array(raw.actions ?? [], "actions").map(normalizeAction),
    "actions",
  );
  const generatedAt = metadata.generatedAt === null || metadata.generatedAt === undefined
    ? null
    : shortText(metadata.generatedAt, "snapshot generation time", 80);
  if (generatedAt !== null && Number.isNaN(Date.parse(generatedAt))) {
    throw new SnapshotError("snapshot generation time must be an ISO date-time");
  }
  return {
    schemaVersion: raw.schemaVersion,
    snapshot: {
      id: identifier(metadata.id, "snapshot id"),
      projectId: identifier(metadata.projectId, "project id"),
      projectName: optionalText(metadata.projectName, "project name", 160),
      projectDescription: optionalText(metadata.projectDescription, "project description", 600),
      generatedAt,
      comparisonAvailable: metadata.comparison !== null && metadata.comparison !== undefined,
      adapters: normalizeAdapters(metadata.adapters),
    },
    lanes,
    nodes,
    edges,
    files,
    unmappedFiles,
    evidence,
    actions,
  };
}

export function normalizeSnapshotEnvelope(input, expectedProjectId) {
  const envelope = record(input, "snapshot envelope");
  exactKeys(envelope, ["projectId", "snapshotId", "map"], "snapshot envelope");
  const projectId = identifier(envelope.projectId, "snapshot project id");
  const snapshotId = identifier(envelope.snapshotId, "snapshot envelope id");
  if (projectId !== expectedProjectId) {
    throw new SnapshotError("The snapshot belongs to a different project");
  }
  const map = normalizeSnapshot(envelope.map);
  if (map.snapshot.projectId !== projectId) {
    throw new SnapshotError("The map project does not match its envelope");
  }
  if (map.snapshot.id !== snapshotId) {
    throw new SnapshotError("The map snapshot does not match its envelope");
  }
  return { projectId, snapshotId, map };
}

function normalizeActionReceipt(input, expected) {
  const receipt = record(input, "action receipt");
  exactKeys(
    receipt,
    [
      "schemaVersion",
      "receiptId",
      "projectId",
      "actionId",
      "outcome",
      "createdAt",
      "beforeSnapshotId",
      "afterSnapshotId",
      "snapshotState",
    ],
    "action receipt",
  );
  if (receipt.schemaVersion !== "owi.action-receipt.v1") {
    throw new SnapshotError("Unsupported action-receipt schema version");
  }
  const normalized = {
    schemaVersion: receipt.schemaVersion,
    receiptId: identifier(receipt.receiptId, "receipt id"),
    projectId: identifier(receipt.projectId, "receipt project id"),
    actionId: identifier(receipt.actionId, "receipt action id"),
    outcome: receipt.outcome,
    createdAt: shortText(receipt.createdAt, "receipt creation time", 80),
    beforeSnapshotId: identifier(receipt.beforeSnapshotId, "receipt before snapshot id"),
    afterSnapshotId:
      receipt.afterSnapshotId === null
        ? null
        : identifier(receipt.afterSnapshotId, "receipt after snapshot id"),
    snapshotState: receipt.snapshotState,
  };
  if (Number.isNaN(Date.parse(normalized.createdAt))) {
    throw new SnapshotError("receipt creation time must be an ISO date-time");
  }
  if (normalized.outcome !== "applied") {
    throw new SnapshotError("The action receipt has an unsupported outcome");
  }
  if (!new Set(["available", "unavailable"]).has(normalized.snapshotState)) {
    throw new SnapshotError("The action receipt has an invalid snapshot state");
  }
  if (
    normalized.projectId !== expected.projectId ||
    normalized.actionId !== expected.actionId ||
    normalized.beforeSnapshotId !== expected.beforeSnapshotId
  ) {
    throw new SnapshotError("The action receipt does not match the confirmed request");
  }
  if (
    (normalized.snapshotState === "available" && normalized.afterSnapshotId === null) ||
    (normalized.snapshotState === "unavailable" && normalized.afterSnapshotId !== null)
  ) {
    throw new SnapshotError("The action receipt and snapshot state disagree");
  }
  return normalized;
}

export function normalizeActionResult(input, expected) {
  const result = record(input, "action result");
  exactKeys(result, ["receipt", "snapshot"], "action result");
  const receipt = normalizeActionReceipt(result.receipt, expected);
  if (receipt.snapshotState === "unavailable") {
    if (result.snapshot !== null) {
      throw new SnapshotError("An unavailable action snapshot must be null");
    }
    return { receipt, snapshot: null };
  }
  const snapshot = normalizeSnapshotEnvelope(result.snapshot, expected.projectId);
  if (snapshot.snapshotId !== receipt.afterSnapshotId) {
    throw new SnapshotError("The action receipt does not match the returned snapshot");
  }
  return { receipt, snapshot };
}

export function formatEvidenceValue(value) {
  if (value === null) return "Unknown";
  if (typeof value.text === "string") return value.text;
  if (value.amount === null || value.amount === undefined) {
    if (value.unit === "currency-micros") return "Unknown";
    const metrics = Object.entries(value.metrics ?? {});
    if (metrics.length === 0) return "Unknown";
    return metrics
      .slice(0, 2)
      .map(([name, metric]) => {
        const label = name.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
        return `${metric === null ? "unknown" : metric} ${label}`;
      })
      .join(" · ");
  }
  if (value.unit === "currency-micros" && value.currency) {
    const amount = (value.amount / 1_000_000).toFixed(6).replace(/\.?0+$/, "");
    const reported = value.metrics?.reportedItems;
    const observed = value.metrics?.observedItems;
    const coverage = Number.isSafeInteger(reported) && Number.isSafeInteger(observed)
      ? ` · ${reported}/${observed} reported`
      : "";
    return `${value.currency} ${amount}${coverage}`;
  }
  const suffix = [value.currency, value.unit].filter(Boolean).join(" ");
  return suffix ? `${value.amount} ${suffix}` : String(value.amount);
}

export function summarizeSnapshot(snapshot) {
  const direct = snapshot.nodes.filter((node) => node.impact.state === "changed").length;
  const affected = snapshot.nodes.filter((node) => node.impact.state === "affected").length;
  const bases = new Set(snapshot.evidence.map((record) => record.basis));
  return {
    components: snapshot.nodes.length,
    direct,
    affected,
    evidenceBases: bases.size,
    actions: snapshot.actions.length,
  };
}
