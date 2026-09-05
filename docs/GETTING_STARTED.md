# Start using OWI Office

OWI Office is a local application. Keep the terminal running while using the
page. Python 3.10+ is enough to create, edit and reopen draft projects; staffing
and execution additionally need the Rust engine and your configured workers.

## 1. Open your workspace

From the repository folder:

```bash
python3 tools/owi-serve
```

Open the complete URL printed after `open`, including its session token. If
the server is restarted, open the new link. Drafts are saved in `.owi-quick/`.
Use `--home /path/to/workspace` to open a different workspace. Do not delete
that directory: it holds your private projects, outputs and learning history.

Choose **Work**, enter a project name and goal, and press **Create project &
draft tasks**. Add tasks and acceptance checks, or edit the proposed draft.
This works before installing the engine and makes no provider calls. The
optional model-planning checkbox remains disabled until setup is complete.

Use **Saved projects** to reopen earlier work. The selected project is included
in the page URL, so reloading returns to it. **Refresh project** retrieves the
latest saved status, including a run started in another tab. Refreshing does
not run the task again.

## 2. Install the decision engine

With Rust 1.87+ installed, from this repository run:

```bash
cargo build --release --locked -p workforce-cli
```

The server finds `target/release/owi` automatically. An already installed
`owi` on PATH or an executable selected by `OWI_BIN` also works. A provider
API key does not install the OWI decision engine.

## 3. Choose sample data or your own measured workforce

To explore sample staffing in a separate workspace:

```bash
python3 tools/owi-serve --demo
```

This explicitly seeds `.owi-demo/`; it does not move your `.owi-quick/`
projects. The sample abilities are illustrative, and the bundled prices are
a dated snapshot. Do not present these as current prices or measured model
performance. Demo mode is not a free sandbox: if you configure runners and
press Run, it can invoke your provider and spend money.

For real staffing, select a workspace containing an imported public index
and a reviewed worker snapshot. The [README](../README.md) documents `owi
prices` and `owi seed`; `examples/index-seed.json` shows the seed schema but
contains sample evidence. Importing price data alone does not establish that
a model is qualified. Workers without sufficient evidence remain ineligible.

## 4. Connect a worker

In the selected workspace, `runners.json` maps each **exact worker ID** to your
local command. Use the full ID shown on the staffed task or in your reviewed
seed, including its `worker:` prefix and role. For example, the shape is:

```json
{
  "worker:your-model/your-role": "/absolute/path/to/your-worker-command"
}
```

Replace both placeholders with your verified configuration. The command must
read the task from standard input and return its result on standard output.
It runs in a fresh scratch directory, so use absolute paths for your adapter
and any required files. The graphical workflow does not accept model-only
keys or commands supplied by the browser. Authenticate the provider through
your own CLI; do not put credentials in project briefs or commit them to git.

Open **Workforce setup → Reload setup** after changing the index or runners.
This rereads the same workspace without restarting, seeding examples, running
a worker, or changing an existing task assignment. **Staff** again to apply
updated bindings. The diagnostics show configured bindings; they cannot prove
provider credentials, model availability or account balance without a run.

## 5. Staff, run and review

Press **Staff**, inspect the selected workers, rejection reasons and estimated
costs, then press **Run assigned worker**. Paid execution always remains an
explicit action. Mechanical acceptance checks run locally. Results needing
judgement stay **ready to review** until you accept or reject them.

The task budget limits allocator forecasts, not verified provider spending.
Actual spend is unknown without receipts. A missing runner, insufficient
evidence or a privacy mismatch remains a blocking condition.

## Release boundary

This is a single-user local application, not a public hosted service. Public
multi-user hosting needs a separate authentication, isolation and deployment
design. The existing Office bitmap pack also retains its pending
redistribution review; see `ui/assets/office/v2/LICENSE.md`. A public graphical
release must pass that review and the repository CI before publication.
