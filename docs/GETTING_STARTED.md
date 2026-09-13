# Your first task with Open Workforce Index

OWI helps you choose an AI worker for a task, compare its estimated cost,
and learn from whether the result worked. A worker is a model together with
its role and tools. Choosing a model and running it are separate actions.

## Choose where to start

| What you opened | What you can do | Where feedback goes |
|---|---|---|
| Ask page in browser mode | Compare models; copy a task into your AI app; paste an answer back for supported checks | This browser and device |
| Ask page in connected mode | Compare models and run tasks through configured commands on your server | The server's private ledger |
| Project console or staffing board | Inspect assignments and estimated costs | These reports do not execute tasks or collect task feedback |

The ask page tells you its mode near the top. “How it works” opens a short
walkthrough. You can reopen it at any time without clearing your work.
Contextual guidance appears when you run, check, or review a result.

Using a Claude subscription? Open **Help when you need it → My plan &
self-updating agents** to declare the billing method and remaining allowance.
The same panel controls automatic instruction updates and rollback. See
[usage and plans](USAGE_AND_PLANS.md) and [adaptive agents](ADAPTIVE_AGENTS.md).

## 1. Describe the work

Include the input and say what the output should look like. For example:

> Rewrite this email in a polite, professional tone: Hello Sam, our delivery
> has not arrived. Please confirm whether it will arrive by Friday. Thanks, Alex.

You can also choose **Write an email**, **Extract JSON**, or **Make a plan**
to load an example. Loading an example does not run a model. If you already
have a draft, clear it and its requirements first to use an example.

Open **Options** to add requirements or mark the task as confidential.
Describe a good result with one requirement per line. Plain language is fine.
Simple automatic checks include:

- `contains:Friday` — the answer must contain Friday.
- `min-words:60` — the answer must contain at least 60 words.
- `json` — the answer must contain valid JSON.

Browser mode can check these rules against a pasted answer. Subjective
requirements need your review, or an available separate checker in connected
mode. In connected mode, an empty requirements box lets the server create a
checklist for you.

## 2. Compare, then run

Select **Choose a model**. The displayed USD amount is an estimate of the
cost of a usable result, including possible retries. It is not a bill.
Starting ability estimates are assumptions; feedback changes them. Open
**Compare models & details** to see alternatives and choose a quality option.

In browser mode, copy the task into your AI app and select the recommended
model if it is available. Desktop links may open a provider's chat; they
cannot guarantee that the app selects that exact model. On a phone, use the
copy-and-paste path.

In connected mode, select **Run task**. This sends the task through the
server's configured model command. It may consume provider usage or
subscription allowance. A configured command also needs working provider
access. If a connection is missing, use **Connect a model** for setup help.

To open connected mode on your computer, install Git, Python 3, and Rust 1.87
or newer, clone this repository, and run `tools/owi-serve` from its root.
Open the address printed in the terminal. The initial build can take a few
minutes. See [model connection instructions](ROUTER.md) for the supported
CLIs and the default `.owi-quick/runners.json` configuration.

## 3. Review and give feedback

Read the answer against your requirements. In browser mode with a checklist,
open **Check your result**, paste the answer, and choose **Check result**.
Automatic checks do not guarantee that the whole answer is correct.

Choose **Worked** after reviewing a usable result. If it did not work,
choose **Needs work**, then the cause: **the model**, **unclear task**, or
**my setup**. Only model-caused failures should reduce that worker's estimate.

Connected mode records decisive checklist outcomes automatically. If saving
fails, the page tells you and keeps the output visible; check the connection
and retry the feedback. Browser feedback stays separate from server feedback.
If browser storage is unavailable, it lasts only until the page is reloaded.

For requests split into parts, run and review each part separately. The
shared requirements box applies to single-part tasks; connected mode creates
a checklist per part. Results are not automatically passed between parts:
include an earlier answer in a later task if it needs that context.

## When you get stuck

- **No suitable model:** check the task's skills, tools, privacy requirement,
  and evidence. A higher budget does not remove these requirements.
- **No connection:** configure a model command on the server and check that
  its CLI has working access to the provider.
- **Confidential task:** only workers explicitly marked with sufficient
  clearance are eligible. The checkbox does not configure a private model.
  A connected run still sends the task to its server and configured worker.
- **Connection lost during a run:** keep your task, check the server, and
  check whether the command completed before starting another run.
- **Want a fresh browser comparison:** open **Help when you need it → Where
  does my feedback go? → Reset browser feedback**. This keeps your current
  task and requirements. It does not erase the server ledger.

For advanced project planning and measurement, continue to
[the full workflow](WORKFLOW.md).
