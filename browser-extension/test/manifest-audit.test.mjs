import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);
const manifestFiles = ["manifest.json", "manifest.firefox.json"];

for (const filename of manifestFiles) {
  test(`${filename} has a least-privilege full-page surface`, async () => {
    const manifest = JSON.parse(await readFile(new URL(filename, root), "utf8"));
    assert.equal(manifest.manifest_version, 3);
    assert.deepEqual(new Set(manifest.permissions), new Set(["storage", "nativeMessaging"]));
    assert.ok(manifest.action);
    assert.equal(manifest.action.default_popup, undefined);
    assert.equal(manifest.host_permissions, undefined);
    assert.equal(manifest.optional_host_permissions, undefined);
    assert.equal(manifest.optional_permissions, undefined);
    assert.equal(manifest.content_scripts, undefined);
    assert.equal(manifest.externally_connectable, undefined);
    assert.equal(manifest.web_accessible_resources, undefined);
    assert.match(manifest.content_security_policy.extension_pages, /script-src 'self'/);
    assert.doesNotMatch(manifest.content_security_policy.extension_pages, /https?:|unsafe-eval/);
  });
}

test("Firefox declares that the client collects no data", async () => {
  const manifest = JSON.parse(
    await readFile(new URL("manifest.firefox.json", root), "utf8"),
  );
  assert.deepEqual(
    manifest.browser_specific_settings.gecko.data_collection_permissions,
    { required: ["none"] },
  );
});

test("dashboard uses shared local assets and exposes both required views", async () => {
  const html = await readFile(new URL("dashboard.html", root), "utf8");
  assert.match(html, />Project map</);
  assert.match(html, />OWI activity &amp; management</);
  assert.match(html, /src="dashboard\.js"/);
  assert.match(html, /href="styles\.css"/);
  assert.doesNotMatch(html, /<script[^>]+src="https?:/i);
  assert.doesNotMatch(html, /\son[a-z]+=/i);
});

test("the bundled fixture is explicitly repository-neutral", async () => {
  const fixture = JSON.parse(
    await readFile(new URL("test/fixtures/system-map.json", root), "utf8"),
  );
  assert.equal(fixture.snapshot.projectName, "Sample workspace");
  assert.deepEqual(
    new Set(fixture.lanes.map((lane) => lane.name)),
    new Set(["Interface", "Service", "Assurance"]),
  );
});

test("the browser client stays neutral and resets every action review", async () => {
  const files = [
    "README.md",
    "dashboard.html",
    "dashboard.js",
    "model.js",
    "protocol.js",
    "native-transport.js",
  ];
  const source = (
    await Promise.all(files.map((file) => readFile(new URL(file, root), "utf8")))
  ).join("\n");
  assert.doesNotMatch(source, /battery|cell pack|contactor|simulation studio/i);
  assert.match(
    source,
    /action_dialog\.returnValue = "cancel";[\s\S]{0,200}action_dialog\.showModal\(\)/,
  );
});

test("the background response works before Chromium Promise-listener support", async () => {
  const source = await readFile(new URL("background.js", root), "utf8");
  assert.match(source, /onMessage\.addListener\(\(message, sender, sendResponse\)/);
  assert.match(source, /\.then\(sendResponse\);[\s\S]{0,300}return true;/);
  assert.match(source, /DASHBOARD_TAB_KEY = "dashboardTabId"/);
  assert.match(source, /tabGet\(stored\[DASHBOARD_TAB_KEY\]\)/);
  assert.match(source, /tabUpdate\(existing\.id, \{ active: true \}\)/);
});

test("an ambiguous action outcome is never presented as not applied", async () => {
  const source = await readFile(new URL("dashboard.js", root), "utf8");
  assert.match(
    source,
    /actionFailureOutcome\(error\?\.code\) === "unknown"[\s\S]{0,300}renderUncertainAction/,
  );
  assert.match(source, /outcome needs review/);
  assert.match(source, /idempotencyKey: context\.idempotencyKey/);
  assert.match(source, /Retry the same request safely/);
});

test("dashboard binds asynchronous results to the reviewed project and snapshot", async () => {
  const source = await readFile(new URL("dashboard.js", root), "utf8");
  assert.match(source, /const epoch = \+\+state\.requestEpoch/);
  assert.match(source, /state\.projectId !== requestedProjectId/);
  assert.match(source, /normalizeSnapshotEnvelope\(wrapper, expectedProjectId\)/);
  assert.match(source, /normalizeActionResult\(result, \{/);
  assert.match(source, /state\.hostSnapshotId !== beforeSnapshotId/);
});

test("responsive activity rows keep the evidence basis visible", async () => {
  const css = await readFile(new URL("styles.css", root), "utf8");
  assert.doesNotMatch(css, /\.evidence-basis\s*,\s*\.evidence-subject\s*\{\s*display:\s*none/);
  assert.match(css, /\.evidence-basis\s*\{\s*grid-column:\s*1 \/ -1/);
});
