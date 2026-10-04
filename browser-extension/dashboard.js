import {
  SnapshotError,
  actionFailureOutcome,
  formatEvidenceValue,
  normalizeActionResult,
  normalizeSnapshotEnvelope,
  summarizeSnapshot,
} from "./model.js";
import { CLIENT_CHANNEL } from "./protocol.js";

const extensionApi = globalThis.browser ?? globalThis.chrome;
const OPAQUE_ID = /^[A-Za-z0-9][A-Za-z0-9._:@-]{0,255}$/;
const state = {
  projects: [],
  projectId: null,
  snapshot: null,
  hostSnapshotId: null,
  selectedNodeId: null,
  pendingAction: null,
  uncertainAction: null,
  nodeElements: new Map(),
  drawing: false,
  busy: false,
  requestEpoch: 0,
  requiresOutcomeReview: false,
};

const dom = Object.fromEntries(
  [
    "project-select",
    "refresh-button",
    "connection-status",
    "project-title",
    "snapshot-caption",
    "snapshot-meta",
    "snapshot-id",
    "snapshot-time",
    "adapter-count",
    "summary-grid",
    "map-viewport",
    "map-content",
    "edge-layer",
    "lane-grid",
    "node-inspector",
    "basis-badges",
    "evidence-list",
    "agent-list",
    "action-list",
    "receipt-panel",
    "fatal-state",
    "fatal-title",
    "fatal-message",
    "retry-button",
    "action-dialog",
    "dialog-title",
    "dialog-description",
    "dialog-authority",
    "dialog-state",
    "dialog-snapshot",
    "dialog-confirm",
    "live-status",
  ].map((id) => [id.replaceAll("-", "_"), document.getElementById(id)]),
);

function element(tagName, options = {}, children = []) {
  const node = document.createElement(tagName);
  if (options.className) node.className = options.className;
  if (options.text !== undefined) node.textContent = String(options.text);
  if (options.type) node.type = options.type;
  if (options.dataset) {
    for (const [key, value] of Object.entries(options.dataset)) {
      node.dataset[key] = value;
    }
  }
  if (options.attributes) {
    for (const [name, value] of Object.entries(options.attributes)) {
      node.setAttribute(name, String(value));
    }
  }
  for (const child of children.flat()) {
    if (child !== null && child !== undefined) node.append(child);
  }
  return node;
}

function prettyToken(value) {
  return value
    .replace(/[._:-]+/g, " ")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .replace(/^./, (character) => character.toUpperCase());
}

function formatDate(value) {
  if (!value) return "Unknown";
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return "Unknown";
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(date);
}

function shortId(value) {
  return value.length > 20 ? `${value.slice(0, 10)}…${value.slice(-7)}` : value;
}

function announce(message) {
  dom.live_status.textContent = "";
  requestAnimationFrame(() => {
    dom.live_status.textContent = message;
  });
}

function setConnection(mode, label) {
  dom.connection_status.dataset.state = mode;
  dom.connection_status.lastChild.textContent = label;
}

function runtimeMessage(message) {
  if (globalThis.browser?.runtime) return globalThis.browser.runtime.sendMessage(message);
  return new Promise((resolve, reject) => {
    extensionApi.runtime.sendMessage(message, (response) => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve(response);
    });
  });
}

function storageGet(key) {
  if (globalThis.browser?.storage) return globalThis.browser.storage.local.get(key);
  return new Promise((resolve, reject) => {
    extensionApi.storage.local.get(key, (result) => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve(result);
    });
  });
}

function storageSet(value) {
  if (globalThis.browser?.storage) return globalThis.browser.storage.local.set(value);
  return new Promise((resolve, reject) => {
    extensionApi.storage.local.set(value, () => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve();
    });
  });
}

async function request(type, fields = {}) {
  const response = await runtimeMessage({ channel: CLIENT_CHANNEL, type, ...fields });
  if (!response || response.ok !== true) {
    const error = new Error(
      response?.error?.message || "The local OWI host could not complete the request",
    );
    error.code = response?.error?.code || "request_failed";
    throw error;
  }
  return response.result;
}

function normalizeProjects(result) {
  const raw = Array.isArray(result) ? result : result?.projects;
  if (!Array.isArray(raw)) throw new Error("The local host returned an invalid project list");
  const seen = new Set();
  return raw.map((item, index) => {
    const value = typeof item === "string" ? { projectId: item } : item;
    const id = value?.projectId ?? value?.id;
    if (typeof id !== "string" || !OPAQUE_ID.test(id) || seen.has(id)) {
      throw new Error(`The local host returned an invalid project at position ${index + 1}`);
    }
    seen.add(id);
    const name = typeof value.name === "string" && value.name.trim()
      ? value.name.trim().slice(0, 160)
      : id;
    return { id, name };
  });
}

function extractSnapshotEnvelope(result, expectedProjectId) {
  const wrapper = result?.map ? result : result?.snapshot?.map ? result.snapshot : null;
  if (!wrapper) {
    throw new SnapshotError("The local host returned an invalid snapshot envelope");
  }
  return normalizeSnapshotEnvelope(wrapper, expectedProjectId);
}

function updateActionControls() {
  for (const button of dom.action_list.querySelectorAll("button")) {
    button.disabled =
      state.busy ||
      state.requiresOutcomeReview ||
      !state.snapshot ||
      button.dataset.available !== "true";
  }
}

function setBusy(busy) {
  state.busy = busy;
  dom.project_select.disabled = busy || state.projects.length === 0;
  dom.refresh_button.disabled = busy || !state.projectId;
  updateActionControls();
}

function invalidateSnapshotState() {
  state.snapshot = null;
  state.hostSnapshotId = null;
  state.pendingAction = null;
  state.selectedNodeId = null;
  if (dom.action_dialog.open) dom.action_dialog.close("cancel");
  updateActionControls();
}

function showFatal(title, message) {
  invalidateSnapshotState();
  dom.fatal_title.textContent = title;
  dom.fatal_message.textContent = message;
  dom.fatal_state.hidden = false;
  setConnection("error", "Unavailable");
}

function clearFatal() {
  dom.fatal_state.hidden = true;
}

async function loadProjects() {
  const epoch = ++state.requestEpoch;
  clearFatal();
  setConnection("loading", "Connecting");
  setBusy(true);
  try {
    const [result, preferences] = await Promise.all([
      request("list_projects"),
      storageGet("selectedProjectId").catch(() => ({})),
    ]);
    if (epoch !== state.requestEpoch) return;
    state.projects = normalizeProjects(result);
    if (state.projects.length === 0) {
      throw new Error("No project is configured in the local OWI host");
    }
    dom.project_select.replaceChildren(
      ...state.projects.map((project) =>
        element("option", {
          text: project.name,
          attributes: { value: project.id },
        }),
      ),
    );
    const storedId = preferences?.selectedProjectId;
    state.projectId = state.projects.some((project) => project.id === storedId)
      ? storedId
      : state.projects[0].id;
    dom.project_select.value = state.projectId;
    await loadSnapshot(state.projectId);
  } catch (error) {
    if (epoch === state.requestEpoch) {
      showFatal("Could not open the project map", safeErrorMessage(error));
    }
  } finally {
    if (epoch === state.requestEpoch) setBusy(false);
  }
}

function safeErrorMessage(error) {
  if (error instanceof SnapshotError) return error.message;
  if (error instanceof Error && error.message) return error.message.slice(0, 500);
  return "The local OWI host is unavailable";
}

async function loadSnapshot(requestedProjectId = state.projectId) {
  if (!requestedProjectId) return false;
  const epoch = ++state.requestEpoch;
  clearFatal();
  setConnection("loading", "Refreshing");
  setBusy(true);
  try {
    const result = await request("get_snapshot", { projectId: requestedProjectId });
    if (epoch !== state.requestEpoch || state.projectId !== requestedProjectId) return false;
    const envelope = extractSnapshotEnvelope(result, requestedProjectId);
    state.snapshot = envelope.map;
    state.hostSnapshotId = envelope.snapshotId;
    state.selectedNodeId = null;
    state.uncertainAction = null;
    state.requiresOutcomeReview = false;
    renderSnapshot();
    setConnection("ready", "Local host connected");
    announce("Project map refreshed");
    return true;
  } catch (error) {
    if (epoch === state.requestEpoch && state.projectId === requestedProjectId) {
      showFatal("Project map unavailable", safeErrorMessage(error));
    }
    return false;
  } finally {
    if (epoch === state.requestEpoch) setBusy(false);
  }
}

function renderSnapshot() {
  const snapshot = state.snapshot;
  const selectedProject = state.projects.find((project) => project.id === state.projectId);
  const title = snapshot.snapshot.projectName || selectedProject?.name || "Selected project";
  dom.project_title.textContent = title;
  dom.snapshot_caption.textContent =
    snapshot.snapshot.projectDescription ||
    "A local view of declared components, direct changes, dependency reach, and OWI evidence.";
  dom.snapshot_meta.hidden = false;
  dom.snapshot_id.textContent = shortId(state.hostSnapshotId);
  dom.snapshot_id.title = state.hostSnapshotId;
  dom.snapshot_time.textContent = formatDate(snapshot.snapshot.generatedAt);
  dom.adapter_count.textContent = String(snapshot.snapshot.adapters.length);
  renderSummary();
  renderMap();
  renderInspector(null);
  renderActivity();
  dom.receipt_panel.hidden = true;
}

function renderSummary() {
  const summary = summarizeSnapshot(state.snapshot);
  const items = [
    ["Components", summary.components, ""],
    ["Direct changes", summary.direct, "changed"],
    ["Affected", summary.affected, "affected"],
    ["Evidence bases", summary.evidenceBases, ""],
    ["Available actions", state.snapshot.actions.filter((action) => action.availability).length, ""],
  ];
  dom.summary_grid.replaceChildren(
    ...items.map(([label, value, className]) =>
      element("div", { className: `summary-item ${className}`.trim() }, [
        element("span", { text: label }),
        element("strong", { text: value }),
      ]),
    ),
  );
}

function nodeChurn(node) {
  if (!node.impact.direct) return null;
  if (node.churn.additions === null || node.churn.deletions === null) {
    return element("span", { className: "muted", text: "Churn unavailable" });
  }
  return element("span", { className: "churn", attributes: { "aria-label": `${node.churn.additions} additions and ${node.churn.deletions} deletions` } }, [
    element("span", { className: "plus", text: `+${node.churn.additions}` }),
    element("span", { className: "minus", text: `−${node.churn.deletions}` }),
  ]);
}

function renderMap() {
  state.nodeElements.clear();
  const nodesByLane = new Map(state.snapshot.lanes.map((lane) => [lane.id, []]));
  for (const node of state.snapshot.nodes) nodesByLane.get(node.laneId).push(node);
  const laneElements = state.snapshot.lanes.map((lane) => {
    const nodeCards = nodesByLane.get(lane.id).map((node) => {
      const card = element(
        "button",
        {
          className: "node-card",
          type: "button",
          dataset: { nodeId: node.id, impact: node.impact.state },
          attributes: { "aria-pressed": "false" },
        },
        [
          element("div", { className: "node-topline" }, [
            element("span", { className: "node-kind", text: node.kind || "Component" }),
            element("span", { className: "lifecycle-badge", text: prettyToken(node.lifecycle) }),
          ]),
          element("h4", { text: node.name }),
          element("div", { className: "node-footer" }, [
            element("span", {
              className: `impact-badge ${node.impact.state}`,
              text: prettyToken(node.impact.state),
            }),
            node.impact.state === "affected"
              ? element("span", { className: "indirect-label", text: "Indirect reach" })
              : nodeChurn(node),
          ]),
        ],
      );
      card.addEventListener("click", () => selectNode(node.id));
      state.nodeElements.set(node.id, card);
      return card;
    });
    if (nodeCards.length === 0) {
      nodeCards.push(element("p", { className: "muted", text: "No components in this lane." }));
    }
    return element("section", { className: "lane", dataset: { laneId: lane.id } }, [
      element("header", { className: "lane-header" }, [
        element("h3", { text: lane.name }),
        lane.description ? element("p", { text: lane.description }) : null,
      ]),
      element("div", { className: "lane-nodes" }, nodeCards),
    ]);
  });
  dom.lane_grid.replaceChildren(...laneElements);
  dom.lane_grid.style.gridTemplateColumns = `repeat(${Math.max(1, laneElements.length)}, minmax(230px, 1fr))`;
  scheduleEdges();
}

function scheduleEdges() {
  if (state.drawing) return;
  state.drawing = true;
  requestAnimationFrame(() => {
    state.drawing = false;
    drawEdges();
  });
}

function drawEdges() {
  if (!state.snapshot) return;
  const definitions = dom.edge_layer.querySelector("defs");
  dom.edge_layer.replaceChildren(definitions);
  const contentRect = dom.map_content.getBoundingClientRect();
  const width = Math.max(dom.map_content.scrollWidth, dom.map_viewport.clientWidth);
  const height = Math.max(dom.map_content.scrollHeight, dom.map_viewport.clientHeight);
  dom.edge_layer.setAttribute("viewBox", `0 0 ${width} ${height}`);
  dom.edge_layer.setAttribute("width", String(width));
  dom.edge_layer.setAttribute("height", String(height));
  for (const edge of state.snapshot.edges) {
    const source = state.nodeElements.get(edge.from);
    const target = state.nodeElements.get(edge.to);
    if (!source || !target) continue;
    const sourceRect = source.getBoundingClientRect();
    const targetRect = target.getBoundingClientRect();
    const goesRight = targetRect.left >= sourceRect.left;
    const startX = (goesRight ? sourceRect.right : sourceRect.left) - contentRect.left;
    const endX = (goesRight ? targetRect.left : targetRect.right) - contentRect.left;
    const startY = sourceRect.top + sourceRect.height / 2 - contentRect.top;
    const endY = targetRect.top + targetRect.height / 2 - contentRect.top;
    const bend = Math.max(32, Math.abs(endX - startX) * 0.48);
    const direction = goesRight ? 1 : -1;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute(
      "d",
      `M ${startX} ${startY} C ${startX + bend * direction} ${startY}, ${endX - bend * direction} ${endY}, ${endX} ${endY}`,
    );
    const targetNode = state.snapshot.nodes.find((node) => node.id === edge.to);
    path.setAttribute(
      "class",
      `edge-path${targetNode?.impact.state === "affected" ? " is-impact" : ""}`,
    );
    dom.edge_layer.append(path);
  }
}

function selectNode(nodeId) {
  state.selectedNodeId = nodeId;
  for (const [id, card] of state.nodeElements) {
    card.setAttribute("aria-pressed", id === nodeId ? "true" : "false");
  }
  const node = state.snapshot.nodes.find((candidate) => candidate.id === nodeId);
  renderInspector(node);
}

function detailRow(label, value) {
  return element("div", {}, [
    element("dt", { text: label }),
    element("dd", { text: value }),
  ]);
}

function renderInspector(node) {
  if (!node) {
    dom.node_inspector.replaceChildren(
      element("p", { className: "eyebrow", text: "Selection" }),
      element("h2", { attributes: { id: "inspector-heading" }, text: "Choose a component" }),
      element("p", {
        className: "muted",
        text: "Select a map card to inspect its lifecycle, direct churn, dependency paths, and owned changed files.",
      }),
    );
    return;
  }
  const files = state.snapshot.files.filter((file) => file.componentIds.includes(node.id));
  const paths = node.impact.paths.map((path) =>
    path
      .map((id) => state.snapshot.nodes.find((candidate) => candidate.id === id)?.name || id)
      .join(" → "),
  );
  const details = [
    detailRow("Lifecycle", prettyToken(node.lifecycle)),
    detailRow("Impact", prettyToken(node.impact.state)),
  ];
  if (node.impact.direct) {
    details.push(
      detailRow(
        "Line churn",
        node.churn.additions === null || node.churn.deletions === null
          ? "Unavailable"
          : `+${node.churn.additions} / −${node.churn.deletions}`,
      ),
      detailRow(
        "Changed files",
        node.churn.changedFiles === null ? "Unknown" : String(node.churn.changedFiles),
      ),
    );
  }
  const groups = [];
  if (paths.length > 0) {
    groups.push(
      element("div", { className: "detail-group" }, [
        element("h3", { text: "Dependency path" }),
        element("ul", {}, paths.map((path) => element("li", { text: path }))),
      ]),
    );
  }
  if (node.sourceRefs.length > 0) {
    groups.push(
      element("div", { className: "detail-group" }, [
        element("h3", { text: "Declared ownership" }),
        element("ul", {}, node.sourceRefs.map((source) =>
          element("li", { text: source.kind === "prefix" ? `${source.path}/` : source.path }),
        )),
      ]),
    );
  }
  if (node.facts.length > 0) {
    groups.push(
      element("div", { className: "detail-group" }, [
        element("h3", { text: "Facts" }),
        element("ul", {}, node.facts.map((fact) => element("li", { text: fact }))),
      ]),
    );
  }
  if (files.length > 0) {
    groups.push(
      element("div", { className: "detail-group" }, [
        element("h3", { text: "Directly changed files" }),
        element("ul", {}, files.map((file) => {
          const churn = file.additions === null || file.deletions === null
            ? prettyToken(file.kind)
            : `+${file.additions} / −${file.deletions}`;
          return element("li", { text: `${file.path} · ${churn}` });
        })),
      ]),
    );
  }
  dom.node_inspector.replaceChildren(
    element("p", { className: "eyebrow", text: node.kind || "Component" }),
    element("div", { className: "inspector-head" }, [
      element("h2", { attributes: { id: "inspector-heading" }, text: node.name }),
      element("span", {
        className: `impact-badge ${node.impact.state}`,
        text: prettyToken(node.impact.state),
      }),
    ]),
    node.description ? element("p", { className: "inspector-description", text: node.description }) : null,
    element("dl", { className: "detail-list" }, details),
    ...groups,
  );
}

function renderActivity() {
  const bases = [...new Set(state.snapshot.evidence.map((record) => record.basis))].sort();
  dom.basis_badges.replaceChildren(
    ...bases.map((basis) => element("span", { className: "basis-badge", text: prettyToken(basis) })),
  );
  if (state.snapshot.evidence.length === 0) {
    dom.evidence_list.replaceChildren(emptyState("No evidence was reported for this snapshot."));
  } else {
    dom.evidence_list.replaceChildren(
      ...state.snapshot.evidence.map((record) =>
        element("article", { className: "evidence-row" }, [
          element("div", { className: "evidence-main" }, [
            element("strong", { text: prettyToken(record.type) }),
            element("span", { text: shortId(record.id) }),
          ]),
          element("span", { className: "evidence-subject", text: shortId(record.subjectId) }),
          element("span", { className: "evidence-basis", text: prettyToken(record.basis) }),
          element("div", {}, [
            element("span", { className: "claim-badge", text: prettyToken(record.claimState) }),
            element("div", { className: "evidence-value", text: formatEvidenceValue(record.value) }),
          ]),
        ]),
      ),
    );
  }

  const activity = state.snapshot.evidence.filter((record) =>
    /agent|instruction|revision|runner|runtime/i.test(record.type),
  );
  if (activity.length === 0) {
    dom.agent_list.replaceChildren(emptyState("No agent or runtime activity was reported."));
  } else {
    dom.agent_list.replaceChildren(
      ...activity.map((record) =>
        element("article", { className: "agent-row" }, [
          element("span", { className: "agent-symbol", text: "A" }),
          element("div", {}, [
            element("strong", { text: shortId(record.subjectId) }),
            element("span", { text: `${prettyToken(record.type)} · ${formatEvidenceValue(record.value)}` }),
          ]),
          element("span", { className: "claim-badge", text: prettyToken(record.claimState) }),
        ]),
      ),
    );
  }
  renderActions();
}

function renderActions() {
  if (!state.snapshot) {
    dom.action_list.replaceChildren(emptyState("Refresh the project map to review management actions."));
    return;
  }
  if (state.snapshot.actions.length === 0) {
    dom.action_list.replaceChildren(emptyState("No management actions are available for this project."));
    return;
  }
  dom.action_list.replaceChildren(
    ...state.snapshot.actions.map((action) => {
      const button = element("button", {
        type: "button",
        text: action.availability ? "Review action" : "Unavailable",
        dataset: { available: action.availability ? "true" : "false" },
      });
      button.disabled =
        state.busy || state.requiresOutcomeReview || !action.availability;
      button.addEventListener("click", () => reviewAction(action));
      return element("article", { className: "action-row" }, [
        element("h3", { text: action.label }),
        element("p", {
          text: action.description ||
            (action.availability
              ? "Review the precondition before asking the local OWI host to apply this action."
              : action.unavailableReason || "The current local state does not allow this action."),
        }),
        element("div", { className: "action-meta" }, [
          element("span", { text: prettyToken(action.authority) }),
          element("span", { text: prettyToken(action.approval) }),
          action.targetId ? element("span", { text: `Target ${shortId(action.targetId)}` }) : null,
        ]),
        button,
      ]);
    }),
  );
}

function emptyState(message) {
  return element("div", { className: "empty-state" }, [element("p", { text: message })]);
}

function reviewAction(action) {
  if (
    state.busy ||
    state.requiresOutcomeReview ||
    !action.availability ||
    !state.snapshot ||
    !state.projectId ||
    !state.hostSnapshotId
  ) return;
  state.pendingAction = {
    action,
    projectId: state.projectId,
    beforeSnapshotId: state.hostSnapshotId,
    idempotencyKey: idempotencyKey(),
  };
  dom.dialog_title.textContent = action.label;
  dom.dialog_description.textContent =
    action.description || "Apply this trusted local OWI management action?";
  dom.dialog_authority.textContent = prettyToken(action.authority);
  dom.dialog_state.textContent = shortId(action.expectedState);
  dom.dialog_state.title = action.expectedState;
  dom.dialog_snapshot.textContent = shortId(state.hostSnapshotId);
  dom.dialog_snapshot.title = state.hostSnapshotId;
  // HTMLDialogElement.returnValue persists across openings. Reset it so an
  // Escape close after an earlier confirmation can never replay "confirm".
  dom.action_dialog.returnValue = "cancel";
  dom.action_dialog.showModal();
}

function idempotencyKey() {
  if (typeof crypto.randomUUID === "function") {
    return `action_${crypto.randomUUID().replaceAll("-", "")}`;
  }
  return `action_${Array.from(crypto.getRandomValues(new Uint8Array(16)), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")}`;
}

async function invokePendingAction() {
  const context = state.pendingAction;
  state.pendingAction = null;
  if (!context) return;
  await invokeActionContext(context);
}

async function invokeActionContext(context, { replay = false } = {}) {
  const { action, projectId, beforeSnapshotId } = context;
  if (
    !replay &&
    (!state.snapshot ||
      state.projectId !== projectId ||
      state.hostSnapshotId !== beforeSnapshotId)
  ) {
    setConnection("error", "Review expired");
    renderRejectedAction(action.label, "The project map changed. Review the action again.");
    announce(`${action.label} was not sent because its review expired`);
    return;
  }
  const epoch = ++state.requestEpoch;
  setConnection("loading", "Applying action");
  setBusy(true);
  let result;
  try {
    result = await request("invoke_action", {
      projectId,
      actionId: action.id,
      expectedSnapshotId: beforeSnapshotId,
      expectedState: action.expectedState,
      idempotencyKey: context.idempotencyKey,
      confirmed: true,
    });
  } catch (error) {
    if (epoch !== state.requestEpoch || state.projectId !== projectId) return;
    if (actionFailureOutcome(error?.code) === "unknown") {
      state.requiresOutcomeReview = true;
      state.uncertainAction = context;
      setConnection("error", "Outcome needs review");
      renderUncertainAction(context, safeErrorMessage(error));
      announce(`${action.label} may have been applied; refresh and inspect the state`);
    } else {
      state.requiresOutcomeReview = false;
      state.uncertainAction = null;
      setConnection("error", "Action not applied");
      renderRejectedAction(action.label, safeErrorMessage(error));
      announce(`${action.label} was not applied`);
    }
    if (epoch === state.requestEpoch) setBusy(false);
    return;
  }

  if (epoch !== state.requestEpoch || state.projectId !== projectId) return;
  let normalized;
  try {
    normalized = normalizeActionResult(result, {
      projectId,
      actionId: action.id,
      beforeSnapshotId,
    });
  } catch (error) {
    state.requiresOutcomeReview = true;
    state.uncertainAction = context;
    setConnection("error", "Outcome needs review");
    renderUncertainAction(context, safeErrorMessage(error));
    announce(`${action.label} returned an unverifiable result; refresh and inspect the state`);
    return;
  } finally {
    if (epoch === state.requestEpoch) setBusy(false);
  }

  state.requiresOutcomeReview = false;
  state.uncertainAction = null;
  const refreshed = normalized.snapshot !== null;
  if (refreshed) {
    state.snapshot = normalized.snapshot.map;
    state.hostSnapshotId = normalized.snapshot.snapshotId;
    state.selectedNodeId = null;
    renderSnapshot();
  } else {
    showFatal(
      "Action applied; map unavailable",
      "The local host recorded the action, but it could not build the latest project map.",
    );
  }
  renderReceipt(normalized.receipt, action.label, refreshed ? "" :
    "The action was applied, but the latest map is unavailable.");
  setConnection(
    refreshed ? "ready" : "error",
    refreshed ? "Action applied" : "Applied; map unavailable",
  );
  announce(
    refreshed
      ? `${action.label} was applied and audited`
      : `${action.label} was applied; the map needs a refresh`,
  );
}

function renderUncertainAction(context, message) {
  const retry = element("button", {
    type: "button",
    className: "secondary-button",
    text: "Retry the same request safely",
  });
  retry.addEventListener("click", () => {
    if (state.uncertainAction !== context || state.busy) return;
    invokeActionContext(context, { replay: true });
  });
  dom.receipt_panel.hidden = false;
  dom.receipt_panel.replaceChildren(
    element("strong", { text: `${context.action.label}: outcome needs review` }),
    element("p", { text: message }),
    element("p", {
      text: "Refresh and inspect the state, or retry this exact idempotent request without creating a second action.",
    }),
    retry,
  );
}

function renderRejectedAction(actionLabel, errorMessage) {
  dom.receipt_panel.hidden = false;
  dom.receipt_panel.replaceChildren(
    element("strong", { text: `${actionLabel} was not applied` }),
    element("p", { text: errorMessage }),
  );
}

function renderReceipt(receipt, actionLabel, noticeMessage = "") {
  dom.receipt_panel.hidden = false;
  dom.receipt_panel.replaceChildren(
    element("strong", { text: `${actionLabel}: ${prettyToken(receipt.outcome)}` }),
    element("p", {
      text: noticeMessage
        ? `Receipt ${shortId(receipt.receiptId)}. The result was recorded by the local host.`
        : `Receipt ${shortId(receipt.receiptId)}. The map now reflects the host's latest snapshot.`,
    }),
    noticeMessage ? element("p", { text: noticeMessage }) : null,
  );
}

function activateTab(name, focus = false) {
  const selected = name === "activity" ? "activity" : "map";
  for (const button of document.querySelectorAll("[role=tab]")) {
    const active = button.dataset.tab === selected;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-selected", active ? "true" : "false");
    button.tabIndex = active ? 0 : -1;
    if (active && focus) button.focus();
  }
  document.getElementById("map-panel").hidden = selected !== "map";
  document.getElementById("activity-panel").hidden = selected !== "activity";
  history.replaceState(null, "", selected === "map" ? "#map" : "#activity");
  if (selected === "map") scheduleEdges();
}

for (const tab of document.querySelectorAll("[role=tab]")) {
  tab.addEventListener("click", () => activateTab(tab.dataset.tab));
  tab.addEventListener("keydown", (event) => {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    activateTab(tab.dataset.tab === "map" ? "activity" : "map", true);
  });
}

dom.project_select.addEventListener("change", async () => {
  const projectId = dom.project_select.value;
  if (!state.projects.some((project) => project.id === projectId)) return;
  state.projectId = projectId;
  const refresh = loadSnapshot(projectId);
  await storageSet({ selectedProjectId: projectId }).catch(() => undefined);
  await refresh;
});
dom.refresh_button.addEventListener("click", () => loadSnapshot());
dom.retry_button.addEventListener("click", loadProjects);
dom.action_dialog.addEventListener("close", () => {
  if (dom.action_dialog.returnValue === "confirm") invokePendingAction();
  else state.pendingAction = null;
});
window.addEventListener("resize", scheduleEdges);
new ResizeObserver(scheduleEdges).observe(dom.map_content);

activateTab(location.hash === "#activity" ? "activity" : "map");
loadProjects();
