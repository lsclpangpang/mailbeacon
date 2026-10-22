const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// 读信：大邮件不排在摘要后面，先出文字，下载有进度
function app(invoke) {
  const calls = [];
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } }, event: { listen: async () => {} } } },
    document: {
      activeElement: null,
      querySelector: () => null,
      querySelectorAll: () => [],
      addEventListener: () => {},
    },
    console, setTimeout, clearTimeout,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    const shown = [];
    renderList = () => {};
    render = () => shown.push(S.msg && { subject: S.msg.subject, partial: !!S.msg.partial });
    renderReader = render;
    globalThis.api = { S, openMessage, loadPreviews, onMailEvent, progressText, draftFor, shown };
  `, context);
  return { ...context.api, calls };
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const tick = () => new Promise((r) => setTimeout(r, 0));
const msgs = (n) => Array.from({ length: n }, (_, i) => ({ uid: i + 1 }));
const sel = { accountId: 'a', folder: 'INBOX', uid: 7 };

test('previews are fetched in batches of 20', async () => {
  const mail = app(async (cmd, { uids }) => (cmd === 'get_previews' ? uids.map((uid) => ({ uid, text: `p${uid}` })) : null));
  await mail.loadPreviews('a', 'INBOX', msgs(50));
  assert.deepEqual(mail.calls.filter((c) => c.cmd === 'get_previews').map((c) => c.input.uids.length), [20, 20, 10]);
});

test('previews wait while a message is downloading', async () => {
  const body = deferred();
  const mail = app(async (cmd, input) => {
    if (cmd === 'get_message') return body.promise;
    if (cmd === 'get_previews') return input.uids.map((uid) => ({ uid, text: '' }));
    return null;
  });
  mail.S.lists.set('a|INBOX', [{ accountId: 'a', folder: 'INBOX', uid: 7, seen: true }]);
  const opened = mail.openMessage(sel);
  await tick();
  const previews = mail.loadPreviews('a', 'INBOX', msgs(30));
  await tick();
  assert.equal(mail.calls.filter((c) => c.cmd === 'get_previews').length, 0, '读信时摘要先让路');
  body.resolve({ subject: '通知' });
  await opened;
  await previews;
  assert.equal(mail.calls.filter((c) => c.cmd === 'get_previews').length, 2);
});

test('another account keeps loading previews while a message downloads', async () => {
  const body = deferred();
  const mail = app(async (cmd, input) => {
    if (cmd === 'get_message') return body.promise;
    if (cmd === 'get_previews') return input.uids.map((uid) => ({ uid, text: '' }));
    return null;
  });
  const opened = mail.openMessage(sel);
  await tick();
  await mail.loadPreviews('b', 'INBOX', msgs(5));
  assert.equal(mail.calls.filter((c) => c.cmd === 'get_previews').length, 1, '别的账号走自己的连接');
  body.resolve({ subject: 'x' });
  await opened;
});

test('a slow download holds previews back for a few seconds at most', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const mail = app(async (cmd, input) => {
    if (cmd === 'get_message') return new Promise(() => {}); // 一直下不完
    if (cmd === 'get_previews') return input.uids.map((uid) => ({ uid, text: '' }));
    return null;
  });
  mail.openMessage(sel);
  await Promise.resolve();
  const previews = mail.loadPreviews('a', 'INBOX', msgs(5));
  for (let i = 0; i < 5; i++) await Promise.resolve();
  assert.equal(mail.calls.filter((c) => c.cmd === 'get_previews').length, 0);
  t.mock.timers.tick(5000);
  await previews;
  assert.equal(mail.calls.filter((c) => c.cmd === 'get_previews').length, 1);
});

function withList(mail) {
  Object.assign(mail.S, { accounts: [{ id: 'a', email: 'a@example.com' }], acct: 'a', folder: 'INBOX' });
  mail.S.lists.set('a|INBOX', [9, 7, 5].map((uid) => ({ accountId: 'a', folder: 'INBOX', uid, date: uid, seen: true })));
}

test('after a message opens, the next one in the list is prefetched', async () => {
  const mail = app(async (cmd) => (cmd === 'get_message' ? { subject: 'x' } : false));
  withList(mail);
  await mail.openMessage(sel);
  await tick();
  assert.deepEqual(mail.calls.filter((c) => c.cmd === 'prefetch_message').map((c) => ({ ...c.input })), [{ accountId: 'a', folder: 'INBOX', uid: 5 }]);
});

test('nothing is prefetched for a message the user already left', async () => {
  const first = deferred();
  const mail = app(async (cmd, { uid }) => (cmd === 'get_message' ? (uid === 7 ? first.promise : { subject: 'y' }) : false));
  withList(mail);
  const opened = mail.openMessage(sel);
  await tick();
  await mail.openMessage({ ...sel, uid: 9 });
  first.resolve({ subject: 'x' });
  await opened;
  await tick();
  assert.deepEqual(mail.calls.filter((c) => c.cmd === 'prefetch_message').map((c) => c.input.uid), [7], '只预取 9 的下一封');
});

test('download progress is tracked only for the message being opened', async () => {
  const body = deferred();
  const mail = app(async (cmd) => (cmd === 'get_message' ? body.promise : false));
  const opened = mail.openMessage(sel);
  await tick();
  mail.onMailEvent({ type: 'messageProgress', accountId: 'a', folder: 'INBOX', uid: 8, received: 999 });
  assert.equal(mail.S.msgProgress, 0, '别的邮件的进度不算');
  mail.onMailEvent({ type: 'messageProgress', accountId: 'a', folder: 'INBOX', uid: 7, received: 348160 });
  assert.equal(mail.S.msgProgress, 348160);
  body.resolve({ subject: 'x' });
  await opened;
  assert.equal(mail.S.msgProgress, 0, '下载完清零');
});

test('progress reads in KB/MB, with the total when known', () => {
  const mail = app(async () => null);
  assert.equal(mail.progressText(0), '正在下载…');
  assert.equal(mail.progressText(348160), '正在下载 340 KB');
  assert.equal(mail.progressText(348160, 910336), '正在下载 340 KB / 889 KB');
  assert.equal(mail.progressText(1572864, 1890304), '正在下载 1.5 MB / 1.8 MB');
  assert.equal(mail.progressText(1000000, 910336), '正在下载 889 KB / 889 KB', '不超过总量');
});

test('a large message shows its text first, then the full message replaces it', async () => {
  const full = deferred();
  const mail = app(async (cmd) => (cmd === 'get_message' ? full.promise : false));
  const opened = mail.openMessage(sel);
  await tick();
  mail.onMailEvent({ type: 'messagePartial', ...sel, view: { subject: '通知', partial: true, size: 910336 } });
  assert.equal(mail.S.msg.partial, true, '文字先出来');
  full.resolve({ subject: '通知' });
  await opened;
  assert.equal(!!mail.S.msg.partial, false);
  assert.deepEqual([...mail.shown].filter(Boolean).map((x) => x.partial), [true, false]);
});

test('a text-only view for another message, or one arriving after the full message, is ignored', async () => {
  const full = deferred();
  const mail = app(async (cmd) => (cmd === 'get_message' ? full.promise : false));
  const opened = mail.openMessage(sel);
  await tick();
  mail.onMailEvent({ type: 'messagePartial', ...sel, uid: 9, view: { subject: '别的', partial: true } });
  assert.equal(mail.S.msg, null);
  full.resolve({ subject: '通知' });
  await opened;
  mail.onMailEvent({ type: 'messagePartial', ...sel, view: { subject: '通知', partial: true } });
  assert.equal(!!mail.S.msg.partial, false, '整封已经到了，不能退回文字版');
});

test('the total size from the backend is used for progress', async () => {
  const mail = app(async (cmd) => (cmd === 'get_message' ? new Promise(() => {}) : false));
  mail.openMessage(sel);
  await tick();
  mail.onMailEvent({ type: 'messageProgress', ...sel, received: 348160, total: 910336 });
  assert.equal(mail.progressText(mail.S.msgProgress, mail.S.msgTotal), '正在下载 340 KB / 889 KB');
});

test('replying waits until the whole message has arrived', () => {
  const mail = app(async () => null);
  mail.S.accounts = [{ id: 'a', email: 'me@example.com' }];
  mail.S.lists.set('a|INBOX', [{ accountId: 'a', folder: 'INBOX', uid: 7, date: 1 }]);
  mail.S.sel = sel;
  mail.S.msg = { subject: 'x', partial: true, from: { name: '', addr: 'nic@example.com' }, to: [], cc: [], attachments: [] };
  assert.equal(mail.draftFor('reply'), null);
});
