import { NativeMapTransport } from "./native-transport.js";
import {
  ProtocolError,
  createGetSnapshotRequest,
  createInvokeActionRequest,
  createListProjectsRequest,
  parseClientMessage,
} from "./protocol.js";

const extensionApi = globalThis.browser ?? globalThis.chrome;
const transport = new NativeMapTransport(extensionApi.runtime);
const DASHBOARD_TAB_KEY = "dashboardTabId";
let openingDashboard = null;

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

function tabGet(tabId) {
  if (globalThis.browser?.tabs) return globalThis.browser.tabs.get(tabId);
  return new Promise((resolve, reject) => {
    extensionApi.tabs.get(tabId, (tab) => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve(tab);
    });
  });
}

function tabUpdate(tabId, properties) {
  if (globalThis.browser?.tabs) return globalThis.browser.tabs.update(tabId, properties);
  return new Promise((resolve, reject) => {
    extensionApi.tabs.update(tabId, properties, (tab) => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve(tab);
    });
  });
}

function tabCreate(properties) {
  if (globalThis.browser?.tabs) return globalThis.browser.tabs.create(properties);
  return new Promise((resolve, reject) => {
    extensionApi.tabs.create(properties, (tab) => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve(tab);
    });
  });
}

function windowFocus(windowId) {
  if (!Number.isInteger(windowId)) return Promise.resolve();
  if (globalThis.browser?.windows) {
    return globalThis.browser.windows.update(windowId, { focused: true });
  }
  return new Promise((resolve, reject) => {
    extensionApi.windows.update(windowId, { focused: true }, () => {
      const error = extensionApi.runtime.lastError;
      if (error) reject(new Error(error.message));
      else resolve();
    });
  });
}

async function openDashboard() {
  const stored = await storageGet(DASHBOARD_TAB_KEY).catch(() => ({}));
  if (Number.isInteger(stored?.[DASHBOARD_TAB_KEY])) {
    try {
      const existing = await tabGet(stored[DASHBOARD_TAB_KEY]);
      await tabUpdate(existing.id, { active: true });
      await windowFocus(existing.windowId);
      return existing;
    } catch {
      await storageSet({ [DASHBOARD_TAB_KEY]: null }).catch(() => undefined);
    }
  }
  const tab = await tabCreate({ url: extensionApi.runtime.getURL("dashboard.html") });
  if (Number.isInteger(tab?.id)) {
    await storageSet({ [DASHBOARD_TAB_KEY]: tab.id }).catch(() => undefined);
  }
  return tab;
}

function opaqueRequestId() {
  if (typeof crypto.randomUUID === "function") {
    return `request:${crypto.randomUUID()}`;
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `request:${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

async function dispatch(message) {
  const input = parseClientMessage(message);
  const id = opaqueRequestId();
  if (input.type === "list_projects") {
    return transport.request(createListProjectsRequest(id));
  }
  if (input.type === "get_snapshot") {
    return transport.request(
      createGetSnapshotRequest({ id, projectId: input.projectId }),
    );
  }
  return transport.request(
    createInvokeActionRequest({
      id,
      projectId: input.projectId,
      actionId: input.actionId,
      expectedSnapshotId: input.expectedSnapshotId,
      expectedState: input.expectedState,
      idempotencyKey: input.idempotencyKey,
      confirmed: input.confirmed,
    }),
  );
}

extensionApi.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (sender?.id && sender.id !== extensionApi.runtime.id) {
    sendResponse({
      ok: false,
      error: { code: "untrusted_sender", message: "Untrusted message sender" },
    });
    return false;
  }
  dispatch(message)
    .then((result) => ({ ok: true, result }))
    .catch((error) => ({
      ok: false,
      error: {
        code: error instanceof ProtocolError ? error.code : "client_error",
        message:
          error instanceof Error
            ? error.message.slice(0, 500)
            : "The project-map request failed",
      },
    }))
    .then(sendResponse);
  // Keep the response channel open on Chromium versions that do not support
  // returning a Promise from runtime.onMessage listeners. Firefox supports
  // the callback form as well.
  return true;
});

extensionApi.action.onClicked.addListener(() => {
  if (!openingDashboard) {
    openingDashboard = openDashboard()
      .catch(() => undefined)
      .finally(() => {
        openingDashboard = null;
      });
  }
});

extensionApi.tabs.onRemoved.addListener((tabId) => {
  storageGet(DASHBOARD_TAB_KEY)
    .then((stored) => {
      if (stored?.[DASHBOARD_TAB_KEY] === tabId) {
        return storageSet({ [DASHBOARD_TAB_KEY]: null });
      }
      return undefined;
    })
    .catch(() => undefined);
});
