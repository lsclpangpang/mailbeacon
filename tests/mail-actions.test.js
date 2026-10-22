const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke, { realOutcome = false } = {}) {
  const bridge = async (cmd, args) => { const value = await invoke(cmd, args); return cmd === "execute_mail_operation" && value === undefined ? {operationId:args.request.operationId,items:args.request.items.map(source=>({source,status:"succeeded"}))} : value; };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const outcomeBox = { html: '', hidden: false, insertAdjacentHTML(_position, html) { this.html += html; }, querySelector() { return null; } };
  const context = vm.createContext({
    window: { MailBeaconSelection: require("../ui/mail-selection.js"), MailBeaconOperations: require("../ui/mail-operations.js"), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: bridge }, event: { listen: async () => {} } } },
    document: { querySelector: (selector) => selector === '#toast' ? outcomeBox : null, addEventListener: () => {} },
    console, setTimeout, clearTimeout, realOutcome,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = function () {};
    if (!realOutcome) showOperationOutcome = function(result) { S.lastOutcome = result; };
    confirmPermanent = async function() { return S.confirmDelete === true; };
    openMessage = async function (sel) { S.sel = sel; S.msg = null; };
    loadList = async function () {};
    globalThis.app = { S, moveCurrent, setFlag, markAllRead, runMailOperation, undoLastOperation, onMailEvent, downloadMessage: typeof downloadMessage === 'function' ? downloadMessage : null };
  `, context);
  const api = context.app;
  api.S.accounts = [{ id: 'a', email: 'a@example.com' }];
  api.S.acct = 'a';
  api.S.folders.a = [{ path: 'INBOX', role: 'inbox' }, { path: 'Work', name: '工作', role: 'other' }];
  api.S.lists.set('a|INBOX', [1, 2, 3].map(uid => ({ accountId: 'a', folder: 'INBOX', receiveEpoch: 1, uidValidity: 7, uid, date: 4 - uid, seen: true })));
  api.S.sel = { accountId: 'a', folder: 'INBOX', uid: 1 };
  return { ...api, outcomeBox };
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
  assert.equal(args.request.items[0].uid, 1);
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


test('failed flag change keeps confirmed local state', async () => {
  const { S, setFlag } = app(async (cmd,args) => ({items:args.request.items.map(source=>({source,status:'failed',message:'拒绝'}))}));
  const mail = S.lists.get('a|INBOX')[0];
  await setFlag(mail,'seen',false);
  assert.equal(mail.seen,true);
  assert.equal(S.lastOutcome.items[0].status,'failed');
});
test('mark loaded read reports partial failures and changes only confirmed messages', async () => {
  const { S, markAllRead } = app(async (cmd,args) => ({items:args.request.items.map((source,i)=>({source,status:i===0?'succeeded':'unknown'}))}));
  for (const m of S.lists.get('a|INBOX')) m.seen=false;
  await markAllRead();
  assert.equal(S.lists.get('a|INBOX').filter(m=>m.seen).length,1);
  assert.equal(S.lastOutcome.items.filter(m=>m.status==='unknown').length,2);
});
test('permanent delete cancellation sends no mutation command', async () => {
  let calls=0;
  const { S, moveCurrent } = app(async () => { calls++; });
  S.folders.a.find(f=>f.path==='INBOX').role='trash';
  await moveCurrent('trash');
  assert.equal(calls,0);
  assert.equal(S.lists.get('a|INBOX').length,3);
});
test('unknown move never removes a local row or schedules a retry', async () => {
  let calls=0;
  const { S, moveCurrent } = app(async (cmd,args) => { calls++; return {items:args.request.items.map(source=>({source,status:'unknown',message:'断线'}))}; });
  await moveCurrent('other','Work');
  assert.equal(calls,1);
  assert.equal(S.lists.get('a|INBOX').length,3);
});

for (const changed of [{ receiveEpoch: 2 }, { uidValidity: 8 }]) {
  test(`a failed operation never binds its recorded identity to a replacement ${Object.keys(changed)[0]}`, async () => {
    let calls = 0;
    const { S, runMailOperation } = app(async () => { calls++; });
    const original = { ...S.lists.get('a|INBOX')[0] };
    Object.assign(S.lists.get('a|INBOX')[0], changed);
    if (changed.receiveEpoch) S.accounts[0].receiveEpoch = changed.receiveEpoch;
    await runMailOperation('trash', [original]);
    assert.equal(calls, 0, 'must require refresh/reselection instead of mutating the new mail');
    assert.equal(S.lists.get('a|INBOX').length, 3);
  });
}

test('a failed undo clears the forward action and offers no generic retry', async () => {
  const source = { accountId: 'a', receiveEpoch: 1, folder: 'Trash', uidValidity: 7, uid: 9 };
  const { S, undoLastOperation, outcomeBox } = app(async () => ({ operationId:'undo:move', items:[{source,status:'failed',message:'mock failure'}] }), { realOutcome:true });
  S.lastRequest = { action:'trash', target:null };
  S.lastOperation = { operationId:'move', undoToken:'move', expiresAt:Date.now()+9000, items:[{source}] };
  await undoLastOperation();
  assert.equal(S.lastRequest, null);
  assert.doesNotMatch(outcomeBox.html, /operation-retry/);
  assert.match(outcomeBox.html, /operation-refresh/);
});
