const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const load = () => import('../scripts/windows-ui-smoke.mjs');

const ready = () => ({
  url: 'http://tauri.localhost/', native: true, platform: 'windows', documentReady: true,
  grid: true, heading: '欢迎使用 MailBeacon', headingVisible: true, formVisible: true,
  emailVisible: true, passwordVisible: true, submitVisible: true,
  readerWidth: 1024, welcomeWidth: 1024,
  selectedKind: 'qq', mailboxButton: { visible: true, unobscured: true, x: 120, y: 180 },
  emailPlaceholder: 'you@qq.com',
});
function fake(states) {
  let time = 0, index = 0;
  const clicks = [];
  return { clicks, sample: async () => states[Math.min(index++, states.length - 1)],
    click: async point => clicks.push(point), now: () => time, sleep: async ms => { time += ms; }, timeoutMs: 1000 };
}

test('a live native window with an empty page must time out, not pass UI smoke', async () => {
  const { waitForUi } = await load();
  const page = ready();
  page.headingVisible = page.formVisible = page.emailVisible = false;
  const harness = fake([page]);
  await assert.rejects(waitForUi(harness), /welcome page/);
  assert.equal(harness.clicks.length, 0);
});

test('UI smoke waits for visible layout, then verifies the mailbox switch changed the form', async () => {
  const { waitForUi } = await load();
  const hidden = { ...ready(), headingVisible: false };
  const switched = { ...ready(), selectedKind: 'gmail', emailPlaceholder: 'you@gmail.com' };
  const harness = fake([hidden, ready(), ready(), switched]);
  const result = await waitForUi(harness);
  assert.equal(result.ready, true);
  assert.equal(result.interaction, true);
  assert.deepEqual(harness.clicks, [{ x: 120, y: 180 }]);
});

test('unchanged form after a click fails even when the welcome page is visible', async () => {
  const { waitForUi } = await load();
  await assert.rejects(waitForUi(fake([ready()])), /mailbox type interaction/);
});

test('browser preview or missing native bridge cannot satisfy the installed-app check', async () => {
  const { waitForUi } = await load();
  await assert.rejects(waitForUi(fake([{ ...ready(), url: 'http://127.0.0.1:53605/' }])), /packaged app/);
  await assert.rejects(waitForUi(fake([{ ...ready(), native: false }])), /welcome page/);
});

test('a covered or offscreen mailbox button is not clicked', async () => {
  const { waitForUi } = await load();
  const harness = fake([{ ...ready(), mailboxButton: { visible: true, unobscured: false, x: 120, y: 180 } }]);
  await assert.rejects(waitForUi(harness), /mailbox type button/);
  assert.equal(harness.clicks.length, 0);
});

test('selected styling alone does not pass when the account form did not update', async () => {
  const { waitForUi } = await load();
  await assert.rejects(waitForUi(fake([ready(), { ...ready(), selectedKind: 'gmail' }])), /mailbox type interaction/);
});

test('text present inside a collapsed reader column cannot satisfy visible layout', async () => {
  const { waitForUi } = await load();
  const collapsed = { ...ready(), readerWidth: 0, welcomeWidth: 0 };
  const harness = fake([collapsed, { ...collapsed, selectedKind: 'gmail', emailPlaceholder: 'you@gmail.com' }]);
  await assert.rejects(waitForUi(harness), /welcome page/);
  assert.equal(harness.clicks.length, 0);
});

test('debug target URLs are restricted to the temporary loopback endpoint', async () => {
  const { validateDebugSocket } = await load();
  assert.equal(validateDebugSocket('ws://127.0.0.1:9222/devtools/page/1', 9222), 'ws://127.0.0.1:9222/devtools/page/1');
  for (const url of ['ws://example.com:9222/devtools/page/1', 'ws://127.0.0.1:9333/devtools/page/1', 'wss://127.0.0.1:9222/devtools/page/1']) {
    assert.throws(() => validateDebugSocket(url, 9222), /loopback/);
  }
});

test('initial WebView navigation may replace its context before the welcome page is ready', async () => {
  const { waitForUi } = await load();
  const harness = fake([ready(), { ...ready(), selectedKind: 'gmail', emailPlaceholder: 'you@gmail.com' }]);
  const sample = harness.sample;
  let first = true;
  harness.sample = async () => {
    if (first) { first = false; throw new Error('Execution context was destroyed.'); }
    return sample();
  };
  const result = await waitForUi(harness);
  assert.equal(result.interaction, true);
});

test('actual DOM sampling rejects controls inside an opacity-zero ancestor', async () => {
  const { inspectWelcome } = await load();
  const style = { display: 'grid', visibility: 'visible', opacity: '1', gridColumn: '1', gridTemplateColumns: '1024px' };
  const parent = { style: { ...style, opacity: '0' }, parentElement: null };
  const element = { style, parentElement: parent, dataset: { kind: 'qq' }, textContent: '欢迎使用 MailBeacon', placeholder: 'you@qq.com',
    getBoundingClientRect: () => ({ x: 10, y: 20, top: 20, left: 10, bottom: 220, right: 1010, width: 1000, height: 200 }),
    closest() { return this; } };
  const context = { document: { readyState: 'complete', documentElement: { dataset: { platform: 'windows' } },
    querySelector: () => element, elementFromPoint: () => element },
    location: { href: 'http://tauri.localhost/' }, innerWidth: 1024, innerHeight: 768,
    window: { __TAURI_INTERNALS__: {}, __TAURI__: { core: { invoke() {} } } }, getComputedStyle: node => node.style };
  const sample = () => vm.runInNewContext(`(${inspectWelcome.toString()})()`, context);
  let state = sample();
  assert.equal(state.headingVisible, false);
  assert.equal(state.emailVisible, false);
  assert.equal(state.mailboxButton.visible, false);
  parent.style.opacity = '1';
  state = sample();
  assert.equal(state.headingVisible, true);
  assert.equal(state.emailVisible, true);
  assert.equal(state.mailboxButton.visible, true);
});

test('uncaught exceptions after the diagnostic event limit still fail the UI smoke', async () => {
  const { recordEvent, assertNoStartupExceptions } = await load();
  const report = { events: [] };
  for (let i = 0; i < 200; i++) recordEvent(report, { method: 'Runtime.consoleAPICalled', params: { type: 'log' } });
  assert.doesNotThrow(() => assertNoStartupExceptions(report));
  const exception = { exceptionDetails: { text: 'Uncaught late startup failure' } };
  recordEvent(report, { method: 'Runtime.exceptionThrown', params: exception });
  assert.equal(report.events.length, 200);
  assert.throws(() => assertNoStartupExceptions(report), /uncaught JavaScript exception/);
  assert.equal(report.exceptionCount, 1);
  assert.deepEqual(report.firstException, exception.exceptionDetails);
  recordEvent(report, { method: 'Runtime.exceptionThrown', params: exception });
  assert.equal(report.exceptionCount, 2);
});
