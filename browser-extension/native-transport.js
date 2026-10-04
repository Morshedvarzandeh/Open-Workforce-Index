import {
  MAX_OUTBOUND_BYTES,
  NATIVE_HOST_NAME,
  ProtocolError,
  assertWireSize,
  parseNativeResponse,
} from "./protocol.js";

const DEFAULT_TIMEOUT_MS = 20_000;

/**
 * A bounded one-request-per-port Native Messaging client.
 *
 * Closing every port lets browser-launched hosts exit after a response, makes
 * durable host configuration visible on the next request, and prevents a
 * timed-out request from poisoning later retries.
 */
export class NativeMapTransport {
  constructor(runtime, options = {}) {
    if (!runtime || typeof runtime.connectNative !== "function") {
      throw new TypeError("A WebExtension runtime with connectNative is required");
    }
    this.runtime = runtime;
    this.hostName = options.hostName ?? NATIVE_HOST_NAME;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.active = new Set();
  }

  request(envelope) {
    assertWireSize(envelope, MAX_OUTBOUND_BYTES, "native request");
    let port;
    try {
      port = this.runtime.connectNative(this.hostName);
    } catch (error) {
      return Promise.reject(
        new ProtocolError(
          error instanceof Error ? error.message : "Could not contact the local OWI host",
          "native_unavailable",
        ),
      );
    }
    return new Promise((resolve, reject) => {
      const operation = { port, settled: false, timer: null, cancel: null };
      const finish = (handler, value) => {
        if (operation.settled) return;
        operation.settled = true;
        clearTimeout(operation.timer);
        this.active.delete(operation);
        try {
          port.disconnect();
        } catch {
          // The browser may already have closed the one-shot native port.
        }
        handler(value);
      };
      operation.cancel = (message = "Native connection closed") =>
        finish(reject, new ProtocolError(message, "native_disconnected"));
      this.active.add(operation);
      port.onMessage.addListener((message) => {
        try {
          finish(resolve, parseNativeResponse(message, envelope.id));
        } catch (error) {
          finish(reject, error);
        }
      });
      port.onDisconnect.addListener(() => {
        const runtimeError = this.runtime.lastError;
        operation.cancel(runtimeError?.message || "The local OWI host disconnected");
      });
      operation.timer = setTimeout(() => {
        finish(
          reject,
          new ProtocolError("The local OWI host did not respond", "native_timeout"),
        );
      }, this.timeoutMs);
      try {
        port.postMessage(envelope);
      } catch (error) {
        finish(
          reject,
          new ProtocolError(
            error instanceof Error ? error.message : "Could not contact the local OWI host",
            "native_unavailable",
          ),
        );
      }
    });
  }

  disconnect() {
    for (const operation of [...this.active]) {
      operation.cancel();
    }
  }
}
