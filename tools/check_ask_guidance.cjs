#!/usr/bin/env node
// Browser regression checks for the first-task flow. No models are called.
// Requires Node.js, Playwright, and its Chromium browser:
//   node tools/check_ask_guidance.cjs
// OWI_CHROMIUM_PATH optionally selects an existing Chromium executable.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {chromium} = require('playwright');

const template = fs.readFileSync(path.join(__dirname, 'owi_ask_template.html'), 'utf8');
const skills = ['skill:text-editing', 'skill:structured-extraction', 'skill:planning-decomposition'];
const ids = ['worker:haiku-4-5/text', 'worker:opus-4-5/text'];
function fixture(connected = false, runnable = ['haiku-4-5', 'opus-4-5']) {
  return {
    local: connected, runnable, floor: 0.24, fallbackMicros: 20000,
    workers: ids.map((id, i) => ({
      id, skills, tools: ['json-schema-validator'], clearance: 'private_metadata',
      inRate: i ? 10000000 : 1000000, outRate: i ? 10000000 : 1000000,
    })),
    skillTools: Object.fromEntries(skills.map(s => [s, []])),
    posteriors: Object.fromEntries(skills.map(s => [s, {
      [ids[0]]: {a: 8, b: 2}, [ids[1]]: {a: 95, b: 5},
    }])),
  };
}
let passed = 0;
function runtimeFixture(profiles = {}) {
  return {settings:{billing:profiles, learning:true, formats:{}},
    billing:Object.fromEntries(Object.entries(profiles).map(([model,p]) => [model,{
      ...p, eligible:p.remaining_percent !== 0, expires_at:p.expires_at || Date.now()/1000+3600,
    }])), agents:{}, updates:[]};
}
async function main() {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.OWI_CHROMIUM_PATH ? {executablePath: process.env.OWI_CHROMIUM_PATH} : {}),
  });
  async function scenario(name, options, check) {
    const data = fixture(options.connected, options.runners);
    if (options.runtime) data.runtime = options.runtime;
    const context = await browser.newContext({
      viewport: options.mobile ? {width: 390, height: 844} : {width: 1280, height: 900},
      isMobile: Boolean(options.mobile), hasTouch: Boolean(options.mobile),
      colorScheme: options.dark ? 'dark' : 'light',
    });
    const errors = [], calls = {data: 0, runs: [], outcomes: []};
    if (options.storageBlocked) await context.addInitScript(() => {
      Object.defineProperty(window, 'localStorage', {
        get() { throw new DOMException('Storage is blocked', 'SecurityError'); }
      });
    });
    if (options.browserFeedback) await context.addInitScript(value => {
      localStorage.setItem('owi-ask-v1', JSON.stringify(value));
    }, options.browserFeedback);
    await context.route('**/*', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== 'http://owi.test') return route.abort();
      if (url.pathname === '/') return route.fulfill({
        contentType: 'text/html',
        body: template.replace('__DATA__', JSON.stringify(data)).replace('__BUILT__', 'test fixture'),
      });
      if (url.pathname === '/api/data') {
        calls.data++;
        return route.fulfill({json: data});
      }
      if (url.pathname === '/api/run') {
        calls.runs.push(route.request().postDataJSON());
        if (options.runWait) await options.runWait;
        return route.fulfill(options.failRun
          ? {status: 500, json: {error: 'Model command is unavailable'}}
          : {json: {exit: 0, output: 'The delivery will arrive Friday.', ...options.runResult}});
      }
      if (url.pathname === '/api/outcome') {
        calls.outcomes.push(route.request().postDataJSON());
        return route.fulfill(options.failSave
          ? {status: 500, json: {error: 'Save failed'}}
          : {json: {ok: true, data}});
      }
      if (url.pathname === '/api/settings') {
        const value = route.request().postDataJSON();
        calls.settings = value;
        if (options.failSettings) return route.fulfill({status:400, json:{error:'Settings were not saved'}});
        data.runtime = {...runtimeFixture(value.billing), settings:value};
        return route.fulfill({json:{ok:true,data}});
      }
      if (url.pathname === '/api/agents/rollback') {
        calls.rollback = route.request().postDataJSON();
        data.runtime.agents[calls.rollback.model].active = 0;
        return route.fulfill({json:{ok:true,data}});
      }
      return route.fulfill({status: 404, body: 'Not found'});
    });
    const page = await context.newPage();
    page.on('pageerror', error => errors.push(error.message));
    try {
      await page.goto('http://owi.test');
      await page.waitForFunction(() => typeof render === 'function');
      if (process.env.OWI_GUIDANCE_SCREENSHOTS && passed === 0) {
        fs.mkdirSync(process.env.OWI_GUIDANCE_SCREENSHOTS, {recursive: true});
        await page.screenshot({path: path.join(process.env.OWI_GUIDANCE_SCREENSHOTS, 'guide-desktop.png'), fullPage: true});
      }
      await check(page, calls, options);
      assert.deepEqual(errors, [], 'No JavaScript errors');
      passed++;
      console.log('PASS: ' + name);
    } finally { await context.close(); }
  }
  const choose = async page => {
    await page.locator('#chooseButton').click();
    await page.locator('#answer.show').waitFor();
  };
  const task = 'Rewrite this email politely: Please confirm delivery by Friday.';
  try {
    await scenario('simple first visit, optional keyboard guide, multiline task, and draft protection', {}, async (page, calls) => {
      assert.equal(await page.locator('#guide').isVisible(), false);
      assert.equal(await page.locator('#checkOptions').isVisible(), false);
      assert.equal(await page.locator('#nextStep').isVisible(), false);
      await page.locator('#guideTrigger').focus();
      await page.keyboard.press('Enter');
      assert.equal(await page.locator('#guideTrigger').getAttribute('aria-expanded'), 'true');
      await page.locator('#guideNext').focus();
      await page.keyboard.press('Enter');
      assert.match(await page.locator('#guideTitle').textContent(), /recommendation/);
      await page.locator('#guideNext').click();
      assert.match(await page.locator('#guideText').textContent(), /AI app/);
      await page.locator('#guideDone').click();
      assert.equal(await page.locator('#q').evaluate(e => e === document.activeElement), true);
      await page.reload();
      assert.equal(await page.locator('#guide').evaluate(e => e.open), false);
      await page.locator('#q').fill('My unfinished draft');
      await page.locator('[data-example="email"]').click();
      assert.equal(await page.locator('#q').inputValue(), 'My unfinished draft');
      assert.match(await page.locator('#exampleNotice').textContent(), /draft is still here/);
      await page.locator('#guideTrigger').click();
      assert.equal(await page.locator('#q').inputValue(), 'My unfinished draft');
      await page.locator('#guideDone').click();
      await page.locator('#q').press('End');
      await page.keyboard.press('Enter');
      assert.equal(await page.locator('#q').inputValue(), 'My unfinished draft\n');
      assert.equal(await page.locator('#answer').isVisible(), false);
      await page.keyboard.press('Control+Enter');
      assert.equal(await page.locator('#answer').isVisible(), true);
      assert.equal(calls.runs.length, 0);
    });
    await scenario('examples classify correctly and compare without execution', {}, async (page, calls) => {
      for (const [example, skill] of [['email', skills[0]], ['json', skills[1]], ['plan', skills[2]]]) {
        await page.locator('#q').fill('');
        await page.locator('#checks').evaluate(e => {e.value = '';});
        await page.locator('[data-example="' + example + '"]').click();
        assert.equal(await page.locator('#checkOptions').isVisible(), false);
        await choose(page);
        assert.equal(await page.evaluate(() => last.skill), skill);
        assert.match(await page.locator('#nextText').textContent(), /Copy the task/);
        assert.equal(await page.locator('#comparison').evaluate(e => e.open), false);
        assert.match(await page.locator('.pick').textContent(), /Claude Haiku/);
        if (example === 'email' && process.env.OWI_GUIDANCE_SCREENSHOTS) {
          await page.screenshot({path: path.join(process.env.OWI_GUIDANCE_SCREENSHOTS, 'result-desktop.png'), fullPage: true});
        }
      }
      assert.deepEqual(calls, {data: 0, runs: [], outcomes: []});
    });
    await scenario('pasted result checks once and reset preserves the task', {}, async page => {
      await page.locator('[data-example="email"]').click();
      await choose(page);
      assert.equal(await page.locator('#reviewDetails').evaluate(e => e.open), false);
      await page.locator('#reviewDetails > summary').click();
      await page.locator('#checkbtn').click();
      assert.match(await page.locator('#nextTitle').textContent(), /Paste the answer/);
      await page.locator('#pasteback').fill('Hello Sam, please confirm delivery by Friday.');
      await page.locator('#checkbtn').click();
      assert.match(await page.locator('#nextTitle').textContent(), /checks passed/);
      assert.equal(await page.locator('#checkbtn').isDisabled(), true);
      const before = await page.locator('#q').inputValue();
      await page.locator('#help > summary').click();
      await page.locator('#feedbackHelp > summary').click();
      await page.locator('#reset').click();
      assert.equal(await page.locator('#q').inputValue(), before);
      assert.equal(await page.evaluate(() => localStorage.getItem('owi-ask-v1')), null);
    });
    await scenario('blocked storage still supports onboarding and temporary feedback', {storageBlocked: true}, async page => {
      await page.locator('#guideTrigger').click();
      await page.locator('#guideDone').click();
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#yes').click();
      assert.match(await page.locator('.learned').textContent(), /until reload/);
    });
    await scenario('connected mode identifies itself immediately and explains missing setup', {connected: true, runners: []}, async (page, calls) => {
      assert.match(await page.locator('#modeBadge').textContent(), /Connected/);
      assert.match(await page.locator('#modeNote').textContent(), /0 model commands/);
      assert.doesNotMatch(await page.locator('#privacyHint').textContent(), /stays in this browser/);
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#connectModel').click();
      assert.equal(await page.locator('#setupHelp').evaluate(e => e.open), true);
      assert.equal(calls.runs.length, 0);
    });
    let releaseRun;
    const runWait = new Promise(resolve => { releaseRun = resolve; });
    await scenario('chosen quality model runs once and preserves connected feedback', {
      connected: true, runWait,
      browserFeedback: {[skills[0]]: {[ids[0]]: {s: 0, f: 10000}}},
    }, async (page, calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      // Stale browser feedback must not contaminate the connected ledger.
      assert.equal(await page.evaluate(() => last.id), ids[0]);
      await page.locator('#comparison > summary').click();
      await page.locator('#preferq').click();
      await page.locator('#go').click();
      assert.equal(await page.locator('#go').isDisabled(), true);
      assert.equal(await page.locator('#q').isDisabled(), true);
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Running your task');
      releaseRun();
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Review the result');
      assert.equal(calls.runs.length, 1);
      assert.equal(calls.runs[0].model, 'opus-4-5');
      await page.locator('#yes').click();
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Feedback updated');
      assert.equal(calls.outcomes.length, 1);
      assert.equal(calls.outcomes[0].worker, ids[1]);
    });
    await scenario('failed feedback save is reported without browser fallback', {connected: true, failSave: true}, async (page, calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#yes').click();
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Feedback was not saved');
      assert.equal(calls.outcomes.length, 1);
      assert.equal(await page.evaluate(() => localStorage.getItem('owi-ask-v1')), null);
      assert.equal(await page.locator('#yes').isVisible(), true);
    });
    await scenario('run errors show a recovery action and re-enable the task', {connected: true, failRun: true}, async page => {
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#go').click();
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Check the model connection');
      assert.equal(await page.locator('#q').isEnabled(), true);
      assert.match(await page.locator('#out').textContent(), /unavailable/);
    });
    await scenario('automatic checklist save failure remains retryable', {
      connected: true, failSave: true,
      runResult: {check: {verdict: 'accepted', items: [{item: 'contains:Friday', pass: true, note: 'found'}]}},
    }, async page => {
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#go').click();
      await page.waitForFunction(() => document.getElementById('nextTitle').textContent === 'Feedback was not saved');
      assert.match(await page.locator('#runstate').textContent(), /not saved/);
      assert.equal(await page.locator('#yes').isVisible(), true);
    });
    await scenario('confidential no-match state explains why without lowering the gate', {}, async (page, calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#optionsToggle').click();
      await page.locator('#confid').check();
      assert.match(await page.locator('#optionsToggle').getAttribute('aria-label'), /Confidential/);
      await page.setViewportSize({width: 320, height: 780});
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      await page.locator('#optionsToggle').click();
      assert.equal(await page.locator('#answer').isVisible(), false);
      await choose(page);
      assert.match(await page.locator('#nextTitle').textContent(), /No suitable model/);
      assert.equal(await page.locator('#copybtn').count(), 0);
      assert.equal(calls.runs.length, 0);
    });
    await scenario('multipart feedback asks for the cause and does not blame setup on models', {}, async page => {
      await page.locator('#q').fill('Rewrite this email politely, then extract the delivery date as JSON');
      await choose(page);
      assert.match(await page.locator('#nextText').textContent(), /not passed between parts/);
      await page.locator('[data-bad="0"]').click();
      await page.locator('[data-pcause="environment"][data-part="0"]').click();
      const saved = await page.evaluate(() => JSON.parse(localStorage.getItem('owi-ask-v1')));
      assert.equal(saved[skills[0]][ids[0]].f, 0);
      assert.match(await page.locator('#pstate-0').textContent(), /unchanged/);
      assert.equal(await page.locator('[data-bad="0"]').isVisible(), false);
    });
    await scenario('mobile dark-mode guide and recommendation fit the screen', {mobile: true, dark: true}, async page => {
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      assert.equal(await page.locator('#chooseButton').evaluate(e => getComputedStyle(e).color), 'rgb(24, 41, 29)');
      const action = await page.locator('#chooseButton').boundingBox();
      assert.ok(action.y + action.height <= 844, 'Primary action is visible without scrolling');
      if (process.env.OWI_GUIDANCE_SCREENSHOTS) {
        fs.mkdirSync(process.env.OWI_GUIDANCE_SCREENSHOTS, {recursive: true});
        await page.screenshot({path: path.join(process.env.OWI_GUIDANCE_SCREENSHOTS, 'guide-mobile.png'), fullPage: true});
      }
      await page.locator('[data-example="email"]').click();
      await choose(page);
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      assert.equal(await page.locator('#answer a.golink').count(), 0);
      assert.equal(await page.locator('#copybtn').isVisible(), true);
      assert.equal(await page.locator('#reviewDetails').evaluate(e => e.open), false);
      if (process.env.OWI_GUIDANCE_SCREENSHOTS) {
        await page.screenshot({path: path.join(process.env.OWI_GUIDANCE_SCREENSHOTS, 'result-mobile.png'), fullPage: true});
      }
    });
    await scenario('subscription allowance changes the recommendation without claiming a cash bill', {
      connected:true, runtime:runtimeFixture({'opus-4-5':{
        mode:'subscription', remaining_percent:50, quota_value_micros:0,
      }}),
    }, async (page,calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      assert.equal(await page.evaluate(() => last.id), ids[1]);
      assert.equal(await page.locator('#answer .price').textContent(), 'Included');
      await page.locator('#comparison > summary').click();
      assert.match(await page.locator('#comparison').textContent(), /not a cash charge/);
      assert.equal(calls.runs.length, 0);
    });
    await scenario('expired subscription allowance cannot produce a runnable recommendation', {
      connected:true, runtime:runtimeFixture(Object.fromEntries(['haiku-4-5','opus-4-5'].map(m => [m,{
        mode:'subscription', remaining_percent:50, expires_at:Date.now()/1000-1,
      }]))),
    }, async (page,calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      assert.equal(await page.locator('#go').count(), 0);
      assert.match(await page.locator('#nextTitle').textContent(), /plan allowance/);
      assert.equal(calls.runs.length, 0);
    });
    await scenario('mobile billing controls save exact quota value, preserve draft, and pause updates', {
      connected:true,mobile:true,dark:true,runtime:runtimeFixture(),
    }, async (page,calls) => {
      await page.locator('#q').fill(task);
      await page.locator('#help > summary').click();
      await page.locator('#runtimeHelp > summary').click();
      await page.locator('#billingMode').selectOption('subscription');
      await page.locator('#billingPlan').fill('Claude Pro');
      await page.locator('#remainingAllowance').fill('50');
      const reset = new Date(Date.now()+7200000).toISOString().slice(0,16);
      await page.locator('#allowanceReset').fill(reset);
      await page.locator('#quotaValue').fill('1.2345e-2');
      await page.locator('#learningEnabled').uncheck();
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
      if (process.env.OWI_GUIDANCE_SCREENSHOTS) {
        await page.screenshot({path:path.join(process.env.OWI_GUIDANCE_SCREENSHOTS,'runtime-mobile.png'),fullPage:true});
      }
      await page.locator('#saveRuntime').click();
      await page.waitForFunction(() => document.getElementById('runtimeStatus').textContent.startsWith('Saved.'));
      assert.equal(calls.settings.learning, false);
      assert.equal(calls.settings.billing['haiku-4-5'].quota_value_micros, 12345);
      assert.equal(calls.settings.billing['opus-4-5'].remaining_percent, 50);
      assert.equal(await page.locator('#q').inputValue(),task);
      calls.settings = null;
      await page.locator('#refreshAllowance').click();
      await page.waitForFunction(() => !document.getElementById('saveRuntime').disabled);
      assert.equal(calls.settings.billing['haiku-4-5'].verified_at, undefined);
      assert.equal(calls.settings.billing['haiku-4-5'].quota_value_micros, 12345);
    });
    await scenario('a failed settings save preserves the current billing and unsaved choice', {
      connected:true,runtime:runtimeFixture(),failSettings:true,
    }, async page => {
      await page.locator('#help > summary').click();
      await page.locator('#runtimeHelp > summary').click();
      await page.locator('#billingMode').selectOption('api');
      await page.locator('#saveRuntime').click();
      await page.waitForFunction(() => document.getElementById('runtimeStatus').textContent.includes('not saved'));
      assert.equal(await page.locator('#billingMode').inputValue(),'api');
      assert.equal(await page.locator('#saveRuntime').isEnabled(),true);
      assert.deepEqual(await page.evaluate(() => data().runtime.settings.billing),{});
    });
    const revisions = {active:1,stable:0,revisions:[
      {id:0,parent:null,state:'stable'},{id:1,parent:0,state:'probation'},
    ]};
    await scenario('usage reports stay distinct from charges and agent versions can roll back', {
      connected:true,runtime:{...runtimeFixture(),agents:{'haiku-4-5':revisions}},
      runResult:{usage:{runs:[{model:'haiku-4-5',role:'worker',revision:1,usage:{
        input_tokens:100,output_tokens:20,cache_read_input_tokens:900,
        api_equivalent_micros:12000,reported_charge_micros:null,
      }}]}},
    }, async (page,calls) => {
      await page.locator('#q').fill(task);
      await choose(page);
      await page.locator('#go').click();
      await page.locator('#out-usage > summary').click();
      assert.match(await page.locator('#out-usage').textContent(), /reported charge: unknown/);
      await page.locator('#help > summary').click();
      await page.locator('#runtimeHelp > summary').click();
      await page.locator('[data-rollback="haiku-4-5"]').click();
      await page.waitForFunction(() => document.getElementById('runtimeStatus').textContent.startsWith('Saved.'));
      assert.equal(calls.rollback.model,'haiku-4-5');
      assert.match(await page.locator('#agentVersions').textContent(), /v0/);
    });
    console.log('\n' + passed + ' browser guidance scenarios passed; no provider calls.');
  } finally { await browser.close(); }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
