import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  SnapshotError,
  actionFailureOutcome,
  formatEvidenceValue,
  normalizeActionResult,
  normalizeSnapshot,
  normalizeSnapshotEnvelope,
  summarizeSnapshot,
} from "../model.js";

const fixtureUrl = new URL("fixtures/system-map.json", import.meta.url);

async function fixture() {
  return JSON.parse(await readFile(fixtureUrl, "utf8"));
}

test("normalizes a repository-neutral system map", async () => {
  const snapshot = normalizeSnapshot(await fixture());
  assert.equal(snapshot.schemaVersion, "owi.system-map.v1");
  assert.equal(snapshot.snapshot.projectName, "Sample workspace");
  assert.deepEqual(snapshot.lanes.map((lane) => lane.id), [
    "lane:interface",
    "lane:service",
    "lane:assurance",
  ]);
  assert.equal(snapshot.nodes.find((node) => node.id === "component:local-agent").kind, "agent");
  assert.deepEqual(
    snapshot.nodes.find((node) => node.id === "component:service").facts,
    ["Validates every request", "Returns opaque action identifiers"],
  );
  assert.equal(snapshot.actions[0].targetId, "component:local-agent");
});

test("only directly changed nodes retain line churn", async () => {
  const snapshot = normalizeSnapshot(await fixture());
  const changed = snapshot.nodes.find((node) => node.id === "component:service");
  const affected = snapshot.nodes.find((node) => node.id === "component:client");
  assert.equal(changed.impact.direct, true);
  assert.equal(changed.churn.additions, 12);
  assert.equal(changed.churn.deletions, 3);
  assert.equal(affected.impact.direct, false);
  assert.equal(affected.churn.additions, null);
  assert.equal(affected.churn.deletions, null);
  assert.deepEqual(affected.impact.paths, [["component:service", "component:client"]]);
});

test("keeps unknown evidence unknown and reports explicit bases", async () => {
  const snapshot = normalizeSnapshot(await fixture());
  const unknown = snapshot.evidence.find((record) => record.id === "evidence:usage");
  const measured = snapshot.evidence.find((record) => record.id === "evidence:working-tree");
  assert.equal(formatEvidenceValue(unknown.value), "Unknown");
  assert.match(formatEvidenceValue(measured.value), /2 changed files/);
  assert.deepEqual(summarizeSnapshot(snapshot), {
    components: 4,
    direct: 1,
    affected: 2,
    evidenceBases: 3,
    actions: 1,
  });
  assert.equal(
    formatEvidenceValue({
      amount: 125000,
      unit: "currency-micros",
      currency: "USD",
      metrics: { reportedItems: 2, observedItems: 5 },
    }),
    "USD 0.125 · 2/5 reported",
  );
  assert.equal(
    formatEvidenceValue({
      amount: null,
      unit: "currency-micros",
      currency: "USD",
      metrics: { reportedItems: 0, observedItems: 0 },
    }),
    "Unknown",
  );
});

test("rejects duplicate IDs, dangling edges, and unsafe paths", async () => {
  const duplicate = await fixture();
  duplicate.nodes.push(structuredClone(duplicate.nodes[0]));
  assert.throws(() => normalizeSnapshot(duplicate), /duplicate id/);

  const dangling = await fixture();
  dangling.edges[0].to = "component:missing";
  assert.throws(() => normalizeSnapshot(dangling), /unknown node/);

  const unsafePath = await fixture();
  unsafePath.files[0].path = "/private/workspace/service.js";
  assert.throws(() => normalizeSnapshot(unsafePath), /must not be absolute/);
});

test("rejects unsupported schemas and executable action payloads", async () => {
  const version = await fixture();
  version.schemaVersion = "owi.system-map.v2";
  assert.throws(() => normalizeSnapshot(version), SnapshotError);

  const command = await fixture();
  command.actions[0].command = "run something";
  assert.throws(() => normalizeSnapshot(command), /forbidden authority field command/);

  const path = await fixture();
  path.actions[0].options = { workingDirectory: "/tmp/example" };
  assert.throws(() => normalizeSnapshot(path), /forbidden authority field workingDirectory/);

  const nestedArray = await fixture();
  nestedArray.actions[0].options = [{ command: "run something" }];
  assert.throws(() => normalizeSnapshot(nestedArray), /forbidden authority field command/);
});

test("binds every snapshot envelope to its requested project and inner map", async () => {
  const map = await fixture();
  const envelope = {
    projectId: map.snapshot.projectId,
    snapshotId: map.snapshot.id,
    map,
  };
  const normalized = normalizeSnapshotEnvelope(envelope, map.snapshot.projectId);
  assert.equal(normalized.snapshotId, map.snapshot.id);

  const wrongProject = structuredClone(envelope);
  wrongProject.projectId = "project:other";
  assert.throws(
    () => normalizeSnapshotEnvelope(wrongProject, map.snapshot.projectId),
    /different project/,
  );

  const wrongSnapshot = structuredClone(envelope);
  wrongSnapshot.snapshotId = "snapshot:other";
  assert.throws(
    () => normalizeSnapshotEnvelope(wrongSnapshot, map.snapshot.projectId),
    /does not match its envelope/,
  );
});

test("accepts only an action receipt bound to its confirmed request and snapshot", async () => {
  const map = await fixture();
  map.snapshot.id = "snapshot:after";
  const expected = {
    projectId: map.snapshot.projectId,
    actionId: "action:pause",
    beforeSnapshotId: "snapshot:before",
  };
  const result = {
    receipt: {
      schemaVersion: "owi.action-receipt.v1",
      receiptId: "receipt:example",
      projectId: expected.projectId,
      actionId: expected.actionId,
      outcome: "applied",
      createdAt: "2026-10-04T12:00:00Z",
      beforeSnapshotId: expected.beforeSnapshotId,
      afterSnapshotId: map.snapshot.id,
      snapshotState: "available",
    },
    snapshot: {
      projectId: expected.projectId,
      snapshotId: map.snapshot.id,
      map,
    },
  };
  const normalized = normalizeActionResult(result, expected);
  assert.equal(normalized.receipt.receiptId, "receipt:example");
  assert.equal(normalized.snapshot.snapshotId, "snapshot:after");

  const mismatched = structuredClone(result);
  mismatched.receipt.actionId = "action:other";
  assert.throws(() => normalizeActionResult(mismatched, expected), /confirmed request/);

  const missingReceipt = structuredClone(result);
  delete missingReceipt.receipt;
  assert.throws(() => normalizeActionResult(missingReceipt, expected), /unexpected fields/);
});

test("transport loss after dispatch is an unknown action outcome", () => {
  for (const code of [
    "action_outcome_unknown",
    "native_timeout",
    "native_disconnected",
    "response_mismatch",
    "internal_error",
  ]) {
    assert.equal(actionFailureOutcome(code), "unknown", code);
  }
  for (const code of [
    "confirmation_required",
    "stale_snapshot",
    "stale_state",
    "unknown_action",
    "action_unavailable",
    "action_failed",
  ]) {
    assert.equal(actionFailureOutcome(code), "not-applied", code);
  }
});
