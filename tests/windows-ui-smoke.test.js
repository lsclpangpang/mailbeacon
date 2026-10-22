const test = require('node:test');
const assert = require('node:assert/strict');
const { mkdtempSync, readFileSync, writeFileSync, rmSync } = require('node:fs');
const { tmpdir } = require('node:os');
const { join } = require('node:path');
const { spawnSync } = require('node:child_process');
const load = () => import('../scripts/windows-ui-smoke.mjs');
const expected = { handle: '1234', processId: 4321 };
const rect = (x = 180, y = 260, width = 400, height = 35) => ({ x, y, width, height });
const item = (name, controlType, extra = {}) => ({ name, controlType, bounds: rect(), offscreen: false,
  enabled: true, isPassword: false, readOnly: false, invoke: controlType === 'Button', ...extra });
const snapshot = (stage = 'qq') => ({
  window: { ...expected, name: 'MailBeacon', bounds: rect(10, 10, 1000, 700), offscreen: false, enabled: true, nativeVisible: true, minimized: false },
  elements: [item('欢迎使用 MailBeacon', 'Text'), item('you@qq.com', 'Edit'),
    item('不是网页登录密码', 'Edit', { isPassword: true }), item('登录并添加', 'Button'),
    item('Gmail', 'Button'), item('QQ 邮箱', 'Button'), item('显示名称', 'Edit'),
    item(stage === 'qq' ? '网页版 设置 → 账号 → 开启 IMAP/SMTP 服务 → 生成授权码。' :
      '先开启两步验证，再到 myaccount.google.com/apppasswords 生成应用专用密码。', 'Text')],
});

test('native welcome must include visible editable account fields and a login button', async () => {
  const { assessSnapshot } = await load();
  assert.equal(assessSnapshot(snapshot(), expected, 'qq').ready, true);
  for (const control of ['欢迎使用 MailBeacon', 'you@qq.com', '不是网页登录密码', '登录并添加']) {
    const state = snapshot();
    state.elements = state.elements.filter(e => e.name !== control);
    assert.equal(assessSnapshot(state, expected, 'qq').ready, false, control);
  }
  assert.equal(assessSnapshot({ ...snapshot(), elements: [] }, expected, 'qq').ready, false, 'white page');
});

test('zero width, offscreen, outside-window, disabled or readonly controls cannot satisfy readiness', async () => {
  const { assessSnapshot } = await load();
  for (const change of [{ bounds: rect(180, 260, 0) }, { bounds: rect(1200, 260) }, { offscreen: true },
    { enabled: false }, { readOnly: true }, { offscreen: null }, { bounds: rect(180, 260, NaN) }]) {
    const state = snapshot();
    Object.assign(state.elements[1], change);
    assert.equal(assessSnapshot(state, expected, 'qq').ready, false, JSON.stringify(change));
  }
  const collapsed = snapshot(); collapsed.window.bounds.width = 0;
  assert.equal(assessSnapshot(collapsed, expected, 'qq').ready, false);
});

test('probe refuses another native window or process even when text matches', async () => {
  const { assessSnapshot } = await load();
  for (const changed of [{ handle: '5678' }, { processId: 9999 }, { name: 'Preview' }]) {
    const state = snapshot(); Object.assign(state.window, changed);
    assert.throws(() => assessSnapshot(state, expected, 'qq'), /window identity/);
  }
});

test('switching mailbox type requires the new form hint and removal of the old hint', async () => {
  const { assessSnapshot } = await load();
  assert.equal(assessSnapshot(snapshot(), expected, 'gmail').ready, false, 'click with no effect');
  assert.equal(assessSnapshot(snapshot('gmail'), expected, 'gmail').ready, true);
  assert.equal(assessSnapshot(snapshot('gmail'), expected, 'qq').ready, false, 'must restore QQ');
  const duplicate = snapshot('gmail'); duplicate.elements.push(snapshot().elements.at(-1));
  assert.equal(assessSnapshot(duplicate, expected, 'gmail').ready, false, 'stale QQ form still exists');
});

test('mailbox button must be enabled, visible, unique and support the real InvokePattern', async () => {
  const { assessSnapshot } = await load();
  for (const change of [{ enabled: false }, { offscreen: true }, { invoke: false }, { bounds: rect(0, 0, 0) }]) {
    const state = snapshot(); Object.assign(state.elements[4], change);
    assert.equal(assessSnapshot(state, expected, 'qq').ready, false, JSON.stringify(change));
  }
  const duplicate = snapshot(); duplicate.elements.push({ ...duplicate.elements[4] });
  assert.equal(assessSnapshot(duplicate, expected, 'qq').ready, false);
  assert.equal(assessSnapshot(snapshot(), expected, 'qq').nextButton, 'Gmail');
  assert.equal(assessSnapshot(snapshot('gmail'), expected, 'gmail').nextButton, 'QQ 邮箱');
});

test('UIA rule CLI consumes Unicode JSON files without depending on the Windows console code page', () => {
  const directory = mkdtempSync(join(tmpdir(), 'uia-'));
  try {
    const input = join(directory, 'state.json'), output = join(directory, 'assessment.json');
    writeFileSync(input, JSON.stringify({ snapshot: snapshot(), expected, stage: 'qq' }));
    const result = spawnSync(process.execPath, [join(__dirname, '../scripts/windows-ui-smoke.mjs'), input, output], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(readFileSync(output)), { ready: true, nextButton: 'Gmail', missing: [] });
    writeFileSync(input, JSON.stringify({ snapshot: snapshot('gmail'), expected, stage: 'gmail' }));
    assert.equal(spawnSync(process.execPath, [join(__dirname, '../scripts/windows-ui-smoke.mjs'), input, output]).status, 0);
    assert.equal(JSON.parse(readFileSync(output)).nextButton, 'QQ 邮箱');
  } finally { rmSync(directory, { recursive: true, force: true }); }
});


test('captured native UIA root needs independent Win32 visibility when IsOffscreen is unsupported', async () => {
  const { assessSnapshot } = await load();
  const captured = require('./fixtures/windows-uia-welcome.json').snapshot;
  const identity = { handle: captured.window.handle, processId: captured.window.processId };
  assert.equal(captured.window.offscreen, null, 'actual provider returned NotSupported');
  assert.equal(assessSnapshot(captured, identity, 'qq').ready, false, 'capture alone has no native visibility evidence');
  const state = structuredClone(captured);
  Object.assign(state.window, { nativeVisible: true, minimized: false });
  assert.equal(assessSnapshot(state, identity, 'qq').ready, true);
  state.window.offscreen = true;
  assert.equal(assessSnapshot(state, identity, 'qq').ready, false, 'known offscreen is still rejected');
});

test('native hidden, minimized, absent or unknown window flags never pass visible controls', async () => {
  const { assessSnapshot } = await load();
  const changes = [{ nativeVisible: false }, { nativeVisible: null }, { nativeVisible: undefined },
    { minimized: true }, { minimized: null }, { minimized: undefined }];
  for (const change of changes) {
    const state = snapshot(); Object.assign(state.window, change);
    assert.equal(assessSnapshot(state, expected, 'qq').ready, false, JSON.stringify(change));
  }
  const childUnknown = snapshot();
  childUnknown.window.offscreen = null;
  childUnknown.elements[1].offscreen = null;
  assert.equal(assessSnapshot(childUnknown, expected, 'qq').ready, false, 'Win32 evidence never replaces child IsOffscreen');
});

const localFixture = { version: 1, host: '127.0.0.1', imapPort: 31415, smtpHost: '127.0.0.1', smtpPort: 9,
  email: 'windows-smoke@example.invalid', name: 'Windows Smoke' };
const fixtureServer = { authRejected: 1, authAccepted: 1, selectAccepted: 1, commands: { SELECT: 1 } };
function customState({ rejected = false } = {}) {
  const state = snapshot();
  state.elements = state.elements.filter(e => !e.name.includes('生成授权码'));
  state.elements.push(item('填写服务器地址。SMTP 端口 587 走 STARTTLS，465 等其他端口走 SSL。', 'Text'));
  state.elements.push(item('imap.example.com', 'Edit', { bounds: rect(180, 450, 280) }),
    item('', 'Edit', { bounds: rect(470, 450, 80) }),
    item('smtp.example.com', 'Edit', { bounds: rect(180, 500, 280) }),
    item('', 'Edit', { bounds: rect(470, 500, 80) }));
  if (rejected) state.elements.push(item('登录失败：fixture-auth-rejected（请确认授权码）', 'Text'));
  return state;
}
function inboxState() {
  return { window: snapshot().window, elements: [item(localFixture.email, 'Text'), item('收件箱', 'Text'),
    item('这里没有邮件', 'Text'), item('没有选中邮件', 'Text')] };
}

test('account smoke accepts only an isolated loopback fixture and exposes explicit UI selectors', async () => {
  const { accountPlan } = await load();
  const plan = accountPlan(localFixture);
  assert.equal(plan.provider, '其他');
  assert.equal(plan.submit, '登录并添加');
  assert.equal(plan.fields.imapHost, 'imap.example.com');
  for (const change of [{ host: 'imap.gmail.com' }, { smtpHost: '192.168.1.1' }, { email: 'real@gmail.com' },
    { imapPort: 0 }, { smtpPort: 65536 }, { version: 2 }]) {
    assert.throws(() => accountPlan({ ...localFixture, ...change }), /isolated loopback/);
  }
});

test('authentication failure requires server evidence, visible error and a usable custom form', async () => {
  const { assessSnapshot } = await load();
  const options = { fixture: localFixture, server: fixtureServer };
  assert.equal(assessSnapshot(customState(), expected, 'custom', options).ready, true);
  assert.equal(assessSnapshot(customState(), expected, 'auth-failed', options).ready, false);
  assert.equal(assessSnapshot(customState({ rejected: true }), expected, 'auth-failed', options).ready, true);
  assert.equal(assessSnapshot(customState({ rejected: true }), expected, 'auth-failed', { ...options, server: { authRejected: 0 } }).ready, false);
  for (const type of ['Edit', 'Button']) {
    const disabled = customState({ rejected: true });
    for (const element of disabled.elements) if (element.controlType === type) element.enabled = false;
    assert.equal(assessSnapshot(disabled, expected, 'auth-failed', options).ready, false);
  }
  const missingPort = customState(); missingPort.elements = missingPort.elements.filter(e => e.name || e.bounds.y !== 450);
  assert.equal(assessSnapshot(missingPort, expected, 'custom', options).ready, false);
});

test('successful account smoke and restart require a real selected inbox, not a welcome page or stale error', async () => {
  const { assessSnapshot } = await load();
  const options = { fixture: localFixture, server: fixtureServer };
  assert.equal(assessSnapshot(inboxState(), expected, 'inbox', options).ready, true);
  for (const change of [{ authAccepted: 0 }, { selectAccepted: 0 }, { selectAccepted: undefined }]) {
    assert.equal(assessSnapshot(inboxState(), expected, 'inbox', { ...options, server: { ...fixtureServer, ...change } }).ready, false);
  }
  for (const name of [localFixture.email, '收件箱', '这里没有邮件']) {
    const missing = inboxState(); missing.elements = missing.elements.filter(e => e.name !== name);
    assert.equal(assessSnapshot(missing, expected, 'inbox', options).ready, false, name);
  }
  for (const name of ['欢迎使用 MailBeacon', '登录失败：fixture-auth-rejected']) {
    const stale = inboxState(); stale.elements.push(item(name, 'Text'));
    assert.equal(assessSnapshot(stale, expected, 'inbox', options).ready, false, name);
  }
});
