const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const C = require('../ui/body-cache.js');

const MB = 1024 * 1024;
const usage = { cachedCount: 412, cachedBytes: 350 * MB, limitBytes: 2048 * MB };

test('settings default to on, 2048 MB in total and 5 MB per mail', () => {
  assert.deepEqual({ ...C.settingsOf({}) }, { enabled: true, limitMb: 2048, maxMailMb: 5 });
  assert.deepEqual({ ...C.settingsOf({ bodyCache: { enabled: false, limitMb: 4096, maxMailMb: 10 } }) }, { enabled: false, limitMb: 4096, maxMailMb: 10 });
});

test('the form is read as whole megabytes and nonsense is explained before saving', () => {
  assert.deepEqual({ ...C.fromForm({ enabled: true, limitMb: '4096', maxMailMb: '8' }) }, { settings: { enabled: true, limitMb: 4096, maxMailMb: 8 } });
  assert.match(C.fromForm({ enabled: true, limitMb: '10', maxMailMb: '5' }).error, /64～102400/);
  assert.match(C.fromForm({ enabled: true, limitMb: '2048', maxMailMb: '0' }).error, /1～32/);
  assert.match(C.fromForm({ enabled: true, limitMb: '2048', maxMailMb: '2.5' }).error, /整数/);
  assert.match(C.fromForm({ enabled: true, limitMb: '64', maxMailMb: '100' }).error, /1～32/);
  assert.match(C.fromForm({ enabled: true, limitMb: '', maxMailMb: '5' }).error, /64～102400/);
});

test('status says what the background cache is doing and how much space it uses', () => {
  const settings = { enabled: true, limitMb: 2048, maxMailMb: 5 };
  assert.equal(C.statusText({ state: 'running', done: 120, total: 860, ...usage }, settings), '正在缓存收件箱 120 / 860 封 · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText({ state: 'waiting', ...usage }, settings), '等待空闲后开始缓存 · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText({ state: 'done', done: 860, total: 860, ...usage }, settings), '收件箱已全部缓存（不含超过 5 MB 的邮件） · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText({ state: 'full', ...usage }, settings), '已到缓存总量上限，不再预先下载 · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText({ state: 'error', detail: '连接服务器超时', ...usage }, settings), '缓存中断：连接服务器超时 · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText({ state: 'off', ...usage }, { ...settings, enabled: false }), '未开启预先缓存，打开过的邮件照样缓存 · 已用 350 MB / 2.0 GB（412 封）');
  assert.equal(C.statusText(undefined, settings), '正在读取缓存状态…');
});

test('the progress bar shows only while caching', () => {
  assert.deepEqual({ ...C.bar({ state: 'running', done: 120, total: 860 }) }, { visible: true, value: 120, max: 860 });
  assert.deepEqual({ ...C.bar({ state: 'running', done: 0, total: 0 }) }, { visible: true, value: null, max: 1 });
  assert.equal(C.bar({ state: 'done', done: 860, total: 860 }).visible, false);
  assert.equal(C.bar(undefined).visible, false);
});

// 设置页：保存、立即更新、事件更新状态
function app(invoke) {
  const calls = [];
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), MailBeaconBodyCache: C, __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } }, event: { listen: async () => {} } } },
    document: { activeElement: null, querySelector: () => null, querySelectorAll: () => [], addEventListener: () => {} },
    console, setTimeout, clearTimeout,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    const toasts = [], statusFor = [];
    toast = (text, isErr = false) => toasts.push({ text, isErr });
    renderBodyCacheStatus = (id) => statusFor.push(id);
    globalThis.api = { S, onMailEvent, saveBodyCache, refreshBodyCache, toasts, statusFor };
  `, context);
  return { ...context.api, calls };
}

function form(values) {
  const status = { textContent: '', classList: { toggle() {} } };
  const button = { disabled: false };
  return {
    dataset: { bodyCacheAccount: 'a' },
    elements: { enabled: { checked: values.enabled }, limitMb: { value: values.limitMb }, maxMailMb: { value: values.maxMailMb } },
    querySelector: (sel) => (sel === '.body-cache-status' ? status : button),
    status,
  };
}

test('saving sends the settings and keeps the updated account', async () => {
  const mail = app(async (cmd, { settings }) => ({ id: 'a', email: 'a@example.com', bodyCache: settings }));
  mail.S.accounts = [{ id: 'a', email: 'a@example.com' }];
  await mail.saveBodyCache(form({ enabled: true, limitMb: '4096', maxMailMb: '8' }));
  assert.deepEqual(mail.calls.map((c) => c.cmd), ['set_body_cache']);
  assert.deepEqual({ ...mail.calls[0].input.settings }, { enabled: true, limitMb: 4096, maxMailMb: 8 });
  assert.equal(mail.S.accounts[0].bodyCache.limitMb, 4096);
  assert.equal(mail.toasts[0].text, '缓存设置已保存');
});

test('invalid sizes are not sent', async () => {
  const mail = app(async () => null);
  mail.S.accounts = [{ id: 'a', email: 'a@example.com' }];
  const f = form({ enabled: true, limitMb: '10', maxMailMb: '5' });
  await mail.saveBodyCache(f);
  assert.equal(mail.calls.length, 0);
  assert.match(f.status.textContent, /64～102400/);
});

test('refreshing starts right away and progress events update the status', async () => {
  const mail = app(async () => ({ state: 'waiting', cachedCount: 0, cachedBytes: 0, limitBytes: 2048 * MB }));
  await mail.refreshBodyCache('a');
  assert.deepEqual(mail.calls.map((c) => c.cmd), ['refresh_body_cache']);
  assert.equal(mail.S.bodyCache.a.state, 'waiting');
  mail.onMailEvent({ type: 'bodyCache', accountId: 'a', state: 'running', done: 3, total: 10, ...usage });
  assert.equal(mail.S.bodyCache.a.done, 3);
  assert.deepEqual([...mail.statusFor], ['a', 'a']);
});

test('a status reply that arrives after a newer progress event does not roll it back', async () => {
  let answer;
  const mail = app((cmd) => new Promise((r) => { answer = r; }));
  const pending = mail.refreshBodyCache('a');
  mail.onMailEvent({ type: 'bodyCache', accountId: 'a', state: 'running', done: 5, total: 10, ...usage });
  answer({ state: 'waiting', cachedCount: 0, cachedBytes: 0, limitBytes: 2048 * MB });
  await pending;
  assert.equal(mail.S.bodyCache.a.state, 'running');
});

test('the update button is only disabled while caching is actually running', () => {
  assert.equal(C.canRefresh({ state: 'running' }), false);
  for (const state of ['waiting', 'done', 'full', 'error', 'off']) assert.equal(C.canRefresh({ state }), true, state);
  assert.equal(C.canRefresh(undefined), true);
});
