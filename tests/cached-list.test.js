const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const message = (uid, extra = {}) => ({ accountId: 'a', receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid, date: uid, seen: true, ...extra });
const page = (items, extra = {}) => ({ items, coverage: [], hasMore: false, nextCursor: null, headersComplete: false, revision: 1, ...extra });
function app(respond, accounts = ['a']) {
  const calls = [], errors = [];
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'),
      MailBeaconViews: require('../ui/views.js'), MailBeaconThreads: require('../ui/threads.js'),
      __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return respond(cmd, input); } }, event: { listen: async () => {} } },
    },
    document: { activeElement: null, querySelector: () => null, querySelectorAll: () => [], addEventListener() {} },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = renderSidebar = updateBadge = revealSelectedRow = redrawNotifySection = hideReaderHint = cancelReaderScroll = () => {};
    loadPreviews = scheduleSidebarCounts = () => {};
    toast = text => errors.push(String(text));
    globalThis.api = { S, loadList, moreSavedMessages, onMailEvent };
  `, context);
  const api = context.api;
  Object.assign(api.S, { accounts: accounts.map(id => ({ id, receiveEpoch: 1, receiveProtocol: 'imap' })), acct: 'a', folder: 'INBOX' });
  return { ...api, calls, errors };
}

test('local next page is available while the server is still waiting and survives its partial reply', async () => {
  const network = deferred();
  const t = app((cmd, input) => cmd === 'list_saved_messages'
    ? input.cursor ? page([message(200)], { headersComplete: false }) : page([message(300)], { hasMore: true, nextCursor: 'older' })
    : network.promise);
  const loading = t.loadList('a', 'INBOX');
  await tick();
  assert.equal(t.S.lists.get('a|INBOX')[0].localOnly, undefined, 'local list rows must retain normal cache-first online opening');
  await t.moreSavedMessages();
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [300, 200]);
  assert.equal(t.calls.filter(c => c.cmd === 'list_messages').length, 1, 'local pagination must not borrow the IMAP lane');
  network.resolve({ items: [message(300)], complete: false, more: true });
  await loading;
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [300, 200]);
  assert.equal(t.S.localSnapshots.get('a|INBOX').headersComplete, false);
});

test('refresh keeps 2700 cached historical rows when online first page returns only 40', async () => {
  const all = Array.from({ length: 2700 }, (_, i) => message(2700 - i));
  const t = app(cmd => cmd === 'list_saved_messages' ? page(all.slice(0, 100), { hasMore: true, nextCursor: 'older' }) : { items: all.slice(0, 40), complete: false, more: true });
  t.S.lists.set('a|INBOX', all);
  await t.loadList('a', 'INBOX', { force: true, forceRefresh: true });
  assert.equal(t.S.lists.get('a|INBOX').length, 2700);
  assert.ok(t.calls.some(c => c.cmd === 'list_saved_messages'), 'refresh reads local history even when an in-memory list exists');
});

test('refresh retains the furthest local cursor so the next click appends unseen history', async () => {
  const local = Array.from({ length: 300 }, (_, i) => message(300 - i));
  const cursors = [];
  const t = app((cmd, input) => {
    if (cmd !== 'list_saved_messages') return { items: local.slice(0, 40), complete: false, more: true };
    cursors.push(input.cursor);
    const start = input.cursor ? Number(input.cursor) : 0;
    return page(local.slice(start, start + 100), { hasMore: start + 100 < local.length, nextCursor: String(start + 100) });
  });
  await t.loadList('a', 'INBOX');
  await t.moreSavedMessages();
  assert.equal(t.S.lists.get('a|INBOX').length, 200);
  await t.loadList('a', 'INBOX', { force: true, forceRefresh: true });
  await t.moreSavedMessages();
  assert.equal(t.S.lists.get('a|INBOX').length, 300);
  assert.deepEqual(cursors, [null, '100', null, '200']);
});

test('incomplete local coverage does not mean the server history has ended', async () => {
  const t = app(cmd => cmd === 'list_saved_messages' ? page([message(1)]) : { items: [message(100)], complete: false, more: false });
  await t.loadList('a', 'INBOX');
  assert.equal(t.S.exhausted.has('a|INBOX'), false);
  assert.equal(t.S.localSnapshots.get('a|INBOX').headersComplete, false);
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [100, 1]);
});

test('all-account inbox paginates each cached source without another network request', async () => {
  const network = deferred();
  const t = app((cmd, input) => cmd === 'list_saved_messages'
    ? page([message(input.cursor ? 10 : 20, { accountId: input.accountId })], { hasMore: !input.cursor, nextCursor: input.cursor ? null : input.accountId + '-next' })
    : network.promise, ['a', 'b']);
  t.S.acct = 'all';
  const first = t.loadList('a', 'INBOX'), second = t.loadList('b', 'INBOX');
  await tick();
  await t.moreSavedMessages();
  for (const id of ['a', 'b']) assert.equal(t.S.lists.get(id + '|INBOX').length, 2);
  assert.equal(t.calls.filter(c => c.cmd === 'list_messages').length, 2);
  network.resolve({ items: [], complete: false, more: true });
  await Promise.all([first, second]);
});

test('virtual starred view filters locally paged source rows while the server is waiting', async () => {
  const network = deferred();
  const t = app((cmd, input) => cmd === 'list_saved_messages'
    ? page([message(input.cursor ? 10 : 20, { flagged: true }), message(input.cursor ? 9 : 19)], { hasMore: !input.cursor, nextCursor: input.cursor ? null : 'next' })
    : network.promise);
  t.S.folder = '__starred';
  const first = t.loadList('a', '__starred');
  await tick();
  await t.moreSavedMessages();
  assert.deepEqual([...t.S.lists.get('a|__starred')].map(m => m.uid), [20, 10]);
  network.resolve({ items: [], complete: false, more: true });
  await first;
});

test('new mailbox generation discards old local rows and old cached pagination', async () => {
  const t = app(cmd => cmd === 'list_saved_messages'
    ? page([message(1)], { hasMore: true, nextCursor: 'old' })
    : { items: [message(1, { uidValidity: 8 })], complete: false, more: true });
  await t.loadList('a', 'INBOX');
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uidValidity), [8]);
  assert.equal(t.S.localSnapshots.has('a|INBOX'), false);
});

test('an empty cache page with a newer mailbox generation removes old rows before the server responds', async () => {
  const network = deferred();
  const t = app(cmd => cmd === 'list_saved_messages' ? page([], { headersComplete: true, coverage: [{ accountId: 'a', folder: 'INBOX', uidValidity: 8 }] }) : network.promise);
  t.S.lists.set('a|INBOX', [message(1)]);
  t.S.localSnapshots.set('a|INBOX', page([message(1)], { hasMore: true, nextCursor: 'old' }));
  const loading = t.loadList('a', 'INBOX');
  await tick();
  assert.deepEqual([...t.S.lists.get('a|INBOX')], []);
  assert.equal(t.S.localSnapshots.has('a|INBOX'), false);
  network.resolve({ items: [], complete: true, more: false });
  await loading;
});

test('an empty server page carries the newly selected mailbox generation and clears old cached mail', async () => {
  const t = app(cmd => cmd === 'list_saved_messages' ? page([message(20)], { hasMore: true, nextCursor: 'old' })
    : { items: [], complete: false, more: false, uidValidity: 8 });
  t.S.sel = message(20); t.S.msg = { text: 'old generation' };
  await t.loadList('a', 'INBOX');
  assert.deepEqual([...t.S.lists.get('a|INBOX')], []);
  assert.equal(t.S.sel, null);
  assert.equal(t.S.localSnapshots.has('a|INBOX'), false);
});

test('an empty changed source generation removes only its rows from a virtual folder', async () => {
  const t = app((cmd, input) => {
    if (cmd === 'list_saved_messages') return page([message(20, { folder: input.folder })], { hasMore: true, nextCursor: 'old' });
    return input.folder === 'Sent-A' ? { items: [], complete: false, more: false, uidValidity: 8 }
      : { items: [message(20, { folder: 'Sent-B' })], complete: false, more: true, uidValidity: 7 };
  });
  t.S.folder = '__sent';
  t.S.folders.a = [{ path: 'Sent-A', role: 'sent' }, { path: 'Sent-B', role: 'sent' }];
  await t.loadList('a', '__sent');
  assert.deepEqual([...t.S.lists.get('a|__sent')].map(m => m.folder), ['Sent-B']);
  assert.equal(t.S.localSnapshots.get('a|__sent')?.hasMore, true, 'another source must retain its local cursor');
});

test('a failed background rebuild preserves local history and waits for an explicit retry', async () => {
  const t = app(cmd => cmd === 'list_saved_messages' ? page([message(20)]) : { items: [message(30)], complete: false, more: true });
  await t.loadList('a', 'INBOX');
  const calls = t.calls.length;
  t.onMailEvent({ type: 'listIndex', accountId: 'a', receiveEpoch: 1, folder: 'INBOX', error: 'server timeout' });
  await tick();
  assert.equal(t.calls.length, calls, 'failed indexing must not immediately start the same network work again');
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [30, 20]);
  assert.equal(t.S.listErrors.get('a|INBOX'), 'server timeout');
});

test('a newly completed index supersedes the old pending server page without another forced refresh', async () => {
  const network = deferred(); let requests = 0;
  const all = [message(30), message(20), message(10)];
  const t = app(cmd => cmd === 'list_saved_messages' ? page([message(30)], { hasMore: true, nextCursor: 'older' })
    : ++requests === 1 ? network.promise : { items: all, complete: true, more: false });
  const old = t.loadList('a', 'INBOX'); await tick();
  t.onMailEvent({ type: 'listIndex', accountId: 'a', folder: 'INBOX', receiveEpoch: 1 });
  await tick();
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [30, 20, 10]);
  network.resolve({ items: [message(30)], complete: false, more: true }); await old;
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [30, 20, 10]);
  assert.equal(t.S.exhausted.has('a|INBOX'), true);
  assert.equal(t.calls.filter(c => c.cmd === 'list_messages')[1].input.forceRefresh, undefined);
});

test('local pagination cannot roll back newer online flags or the validated reader state', async () => {
  const t = app((cmd, input) => cmd === 'list_saved_messages'
    ? input.cursor ? page([message(10)]) : page([message(20, { seen: false, flagged: false, localOnly: true })], { hasMore: true, nextCursor: 'older' })
    : { items: [message(20, { seen: true, flagged: true, localOnly: false })], complete: false, more: true });
  await t.loadList('a', 'INBOX');
  await t.moreSavedMessages();
  const row = t.S.lists.get('a|INBOX').find(m => m.uid === 20);
  assert.equal(row.seen, true); assert.equal(row.flagged, true); assert.equal(row.localOnly, false);
});

test('a completed list keeps mail received during its request while removing obsolete historical rows', async () => {
  const network = deferred();
  const t = app(cmd => cmd === 'list_saved_messages' ? page([message(42), message(2)]) : network.promise);
  t.S.lists.set('a|INBOX', [message(1)]);
  const loading = t.loadList('a', 'INBOX'); await tick();
  t.onMailEvent({ type: 'newMail', accountId: 'a', receiveEpoch: 1, messages: [message(43)] });
  network.resolve({ items: [message(42)], complete: true, more: false });
  await loading;
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [43, 42]);
});

test('duplicate local pagination clicks are ignored and late pages cannot cross a receiving endpoint change', async () => {
  const next = deferred();
  const t = app(() => next.promise);
  t.S.lists.set('a|INBOX', [message(20)]);
  t.S.localSnapshots.set('a|INBOX', page([message(20)], { hasMore: true, nextCursor: 'older' }));
  const first = t.moreSavedMessages(), duplicate = t.moreSavedMessages();
  await tick();
  assert.equal(t.calls.length, 1);
  t.S.accounts[0].receiveEpoch = 2;
  t.S.lists.clear(); t.S.localSnapshots.clear();
  next.resolve(page([message(10)]));
  await Promise.all([first, duplicate]);
  assert.equal(t.S.lists.has('a|INBOX'), false);
});

test('without local data the normal server list still loads and a completed index replaces obsolete history', async () => {
  const t = app(cmd => cmd === 'list_saved_messages' ? page([]) : { items: [message(10)], complete: true, more: false });
  t.S.lists.set('a|INBOX', [message(5)]);
  await t.loadList('a', 'INBOX');
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [10]);
  assert.equal(t.S.exhausted.has('a|INBOX'), true);
  assert.equal(t.S.localSnapshots.has('a|INBOX'), false);
});
