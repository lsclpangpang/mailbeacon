const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// 「收取」按钮：让每个账号立刻检查新邮件，并告诉用户结果
function app(invoke) {
  const listeners = {};
  const calls = [];
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } }, event: { listen: async () => {} } } },
    document: {
      activeElement: null,
      querySelector: (selector) => (selector === '#overlay' ? { innerHTML: '' } : null),
      querySelectorAll: () => [],
      addEventListener: (name, handler) => { listeners[name] = handler; },
    },
    console, setTimeout, clearTimeout,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    const toasts = [], reloads = [];
    let renders = 0;
    renderList = () => { renders++; };
    render = () => {};
    toast = (text, isErr = false) => toasts.push({ text, isErr });
    loadList = (accountId, folder, opts) => reloads.push({ accountId, folder, force: !!opts?.force });
    openCompose = () => toasts.push({ text: 'compose' });
    globalThis.api = { S, checkMail, toasts, reloads, get renders() { return renders; } };
  `, context);
  return { ...context.api, calls, listeners };
}

const accounts = [{ id: 'a', email: 'a@163.com' }, { id: 'b', email: 'b@qq.com' }];

test('checking mail asks every account and reports how many new messages arrived', async () => {
  const mail = app(async (cmd, { accountId }) => ({ a: 2, b: 1 })[accountId]);
  Object.assign(mail.S, { accounts, acct: 'all', folder: 'INBOX' });
  const done = mail.checkMail();
  assert.equal(mail.S.checking, true, '收取中按钮转圈');
  await done;
  assert.equal(mail.S.checking, false);
  assert.deepEqual(mail.calls.filter((c) => c.cmd === 'check_mail').map((c) => c.input.accountId), ['a', 'b']);
  assert.deepEqual([...mail.toasts].map((t) => ({ ...t })), [{ text: '收到 3 封新邮件', isErr: false }]);
  assert.deepEqual([...mail.reloads], [], '收件箱的新邮件由推送事件加进列表，不用重新读');
});

test('only the selected account is checked, and no news is said plainly', async () => {
  const mail = app(async () => 0);
  Object.assign(mail.S, { accounts, acct: 'b', folder: 'INBOX' });
  await mail.checkMail();
  assert.deepEqual(mail.calls.map((c) => c.input.accountId), ['b']);
  assert.equal(mail.toasts[0].text, '没有新邮件');
});

test('a failing account is named while the others still report', async () => {
  const mail = app(async (cmd, { accountId }) => {
    if (accountId === 'b') throw '邮箱暂时连不上，正在重新连接';
    return 4;
  });
  Object.assign(mail.S, { accounts, acct: 'all', folder: 'INBOX' });
  await mail.checkMail();
  const [t] = mail.toasts;
  assert.equal(t.isErr, true);
  assert.match(t.text, /收到 4 封新邮件/);
  assert.match(t.text, /b@qq\.com/);
  assert.match(t.text, /连不上/);
  assert.equal(mail.S.checking, false, '失败后按钮恢复');
});

test('clicking again while checking does not send a second round', async () => {
  let release;
  const mail = app(() => new Promise((resolve) => { release = resolve; }));
  Object.assign(mail.S, { accounts: [accounts[0]], acct: 'a', folder: 'INBOX' });
  const first = mail.checkMail();
  await mail.checkMail();
  release(1);
  await first;
  assert.equal(mail.calls.length, 1);
});

test('other folders and views are reloaded after checking', async () => {
  const mail = app(async () => 1);
  Object.assign(mail.S, { accounts, acct: 'all', folder: '__unread' });
  await mail.checkMail();
  assert.deepEqual([...mail.reloads].map((r) => ({ ...r })), [
    { accountId: 'a', folder: '__unread', force: true },
    { accountId: 'b', folder: '__unread', force: true },
  ]);
});

test('Shift-Command-N checks mail and Command-N still composes', async () => {
  const mail = app(async () => 0);
  Object.assign(mail.S, { accounts, acct: 'a', folder: 'INBOX' });
  const key = (extra) => mail.listeners.keydown({ key: extra.shiftKey ? 'N' : 'n', metaKey: true, target: { id: '' }, preventDefault() {}, ...extra });
  key({ shiftKey: true });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(mail.calls.map((c) => c.cmd), ['check_mail']);
  key({ shiftKey: false });
  assert.equal(mail.toasts.at(-1).text, 'compose');
  assert.equal(mail.calls.length, 1);
});

test('switching folders mid-check reloads the folder the check started in', async () => {
  let release;
  const mail = app(() => new Promise((resolve) => { release = resolve; }));
  Object.assign(mail.S, { accounts: [accounts[0]], acct: 'a', folder: '__unread' });
  const done = mail.checkMail();
  mail.S.folder = 'INBOX';
  release(0);
  await done;
  assert.deepEqual([...mail.reloads].map((r) => ({ ...r })), [{ accountId: 'a', folder: '__unread', force: true }]);
});
