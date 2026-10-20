const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke) {
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: { querySelector: () => null, addEventListener: () => {} },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = function () {};
    openMessage = async function (sel) { S.sel = sel; S.msg = null; };
    loadList = async function () {};
    globalThis.app = { S, moveCurrent, onMailEvent, downloadMessage: typeof downloadMessage === 'function' ? downloadMessage : null };
  `, context);
  const api = context.app;
  api.S.accounts = [{ id: 'a', email: 'a@example.com' }];
  api.S.acct = 'a';
  api.S.folders.a = [{ path: 'INBOX', role: 'inbox' }, { path: 'Work', name: '工作', role: 'other' }];
  api.S.lists.set('a|INBOX', [1, 2, 3].map(uid => ({ accountId: 'a', folder: 'INBOX', uid, date: 4 - uid, seen: true })));
  api.S.sel = { accountId: 'a', folder: 'INBOX', uid: 1 };
  return api;
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('a move keeps the source visible until the server confirms success', async () => {
  const request = deferred();
  const { S, moveCurrent } = app(() => request.promise);
  const moving = moveCurrent('other', 'Work');
  assert.equal(S.lists.get('a|INBOX').length, 3);
  assert.equal(S.sel.uid, 1);
  request.resolve();
  await moving;
  assert.equal(S.lists.get('a|INBOX').some(m => m.uid === 1), false);
});

test('failed move retains the source and the selected message', async () => {
  const { S, moveCurrent } = app(async () => { throw new Error('网络中断'); });
  await moveCurrent('other', 'Work');
  assert.equal(S.lists.get('a|INBOX').length, 3);
  assert.equal(S.sel.uid, 1);
});

test('captured source stays stable and completing a move does not steal a newer selection', async () => {
  const request = deferred();
  let args;
  const { S, moveCurrent } = app((cmd, input) => { args = input; return request.promise; });
  const original = { ...S.sel };
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 2 };
  const moving = moveCurrent('other', 'Work', original);
  assert.equal(args.uids[0], 1);
  request.resolve();
  await moving;
  assert.equal(S.sel.uid, 2);
});

test('repeated move is ignored while the same message is already moving', async () => {
  const request = deferred();
  let calls = 0;
  const { moveCurrent } = app(() => { calls++; return request.promise; });
  const first = moveCurrent('other', 'Work');
  const second = moveCurrent('other', 'Work');
  assert.equal(calls, 1);
  request.resolve();
  await Promise.all([first, second]);
});

test('old inbox snapshots cannot resurrect successfully moved messages', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  const oldSnapshot = S.lists.get('a|INBOX').map(m => ({ ...m }));
  await moveCurrent('other', 'Work');
  onMailEvent({ type: 'inbox', accountId: 'a', messages: oldSnapshot });
  assert.equal(S.lists.get('a|INBOX').some(m => m.uid === 1), false);
});

test('download saves the captured message and rejects duplicate clicks', async () => {
  const request = deferred();
  const calls = [];
  const { S, downloadMessage } = app((cmd, input) => { calls.push({ cmd, input }); return request.promise; });
  assert.equal(typeof downloadMessage, 'function');
  const original = { ...S.sel };
  const first = downloadMessage(original);
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 2 };
  const second = downloadMessage(original);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, 'save_message');
  assert.equal(calls[0].input.uid, 1);
  request.resolve('/Users/me/Downloads/邮件.eml');
  await Promise.all([first, second]);
});

test('invalidating an in-flight destination load releases its loading state', async () => {
  const { S, moveCurrent } = app(async () => {});
  S.loading.add('a|Work');
  S.loading.add('a|__starred');
  await moveCurrent('other', 'Work');
  assert.equal(S.loading.has('a|Work'), false);
  assert.equal(S.loading.has('a|__starred'), false);
});

test('reconnecting clears stale moved-UID filters', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  await moveCurrent('other', 'Work');
  assert.equal(S.removed.size, 1);
  onMailEvent({ type: 'status', accountId: 'a', state: 'online' });
  assert.equal(S.removed.size, 0);
});

test('reconnection accepts a reused UID in the initial inbox snapshot before online', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  await moveCurrent('other', 'Work');
  onMailEvent({ type: 'status', accountId: 'a', state: 'connecting' });
  onMailEvent({ type: 'inbox', accountId: 'a', messages: [{ accountId: 'a', folder: 'INBOX', uid: 1, subject: '新的合法邮件' }] });
  onMailEvent({ type: 'status', accountId: 'a', state: 'online' });
  assert.equal(S.lists.get('a|INBOX')[0]?.subject, '新的合法邮件');
});
