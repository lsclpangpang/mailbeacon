const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const selection = require('../ui/mail-selection.js');

const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const mail = (id = 'a') => ({ accountId: id, receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid: 42, seen: false });

function setup(handler = async () => null) {
  const timers = new Map(); let timer = 0;
  const calls = [];
  const context = vm.createContext({
    window: { MailBeaconAccountRepair: require('../ui/account-repair.js'), MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: selection, MailBeaconViews: require('../ui/views.js'), MailBeaconThreads: require('../ui/threads.js'), __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return handler(cmd, input); } }, event: { listen: async () => {} } } },
    document: { querySelector: () => null, querySelectorAll: () => [], addEventListener() {} }, console,
    setTimeout: callback => { timers.set(++timer, callback); return timer; }, clearTimeout: id => timers.delete(id),
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render=renderList=renderReader=renderSidebar=updateBadge=paintPreviews=function(){};
    toast=function(){};
    globalThis.api={S,adoptRepairedAccount,loadList,loadPreviews,requestServerSearch,onMailEvent};
  `, context);
  const api = context.api;
  api.S.accounts = ['a', 'b'].map(id => ({ id, receiveEpoch: 1, connectionRevision: 1, receiveProtocol: 'imap', email: id + '@example.test' }));
  api.S.acct = 'a';
  return { ...api, calls, timers, flush: async () => { const callbacks = [...timers.values()]; timers.clear(); return Promise.all(callbacks.map(f => f())); } };
}

test('receive identity repair invalidates only that account cache and the combined search snapshot', () => {
  const t = setup(), { S } = t;
  for (const map of [S.lists, S.results, S.indexing, S.localSnapshots, S.morePages, S.previews]) {
    map.set('a|INBOX', ['old a']); map.set('b|INBOX', ['keep b']);
  }
  for (const set of [S.loading, S.exhausted, S.removed, S.showImages]) { set.add('a|INBOX'); set.add('b|INBOX'); }
  S.listSeq.set('a|INBOX', 4); S.listSeq.set('b|INBOX', 2);
  S.folderSeq.set('a', 3); S.folderSeq.set('b', 6);
  S.folders.a = [{ path: 'Old server' }]; S.folders.b = [{ path: 'INBOX' }];
  S.folderErrors.set('a', 'old failure');
  S.live.a = { state: 'online', unread: 17 }; S.live.b = { state: 'online', unread: 8 };
  S.query = 'old query'; S.searchSeq = 9; S.searchErrors = ['old error'];
  S.results.set('__search', { items: [mail()], nextCursor: 'old cursor' });
  S.searchPage = { coverage: ['old server'] };
  S.sel = mail(); S.msg = { localOnly: true }; S.msgLoading = true; S.msgError = 'old';
  S.thread = { items: [mail()] }; S.readerHintUntil = 5000;
  S.batch.add(selection.key(mail())); S.batchAnchor = selection.key(mail());
  t.adoptRepairedAccount({ ...S.accounts[0], receiveEpoch: 2, connectionRevision: 2 });
  for (const name of ['lists', 'results', 'indexing', 'localSnapshots', 'morePages', 'previews']) {
    assert.equal(S[name].has('a|INBOX'), false, name + ' retains old account');
    assert.equal(S[name].has('b|INBOX'), true, name + ' loses another account');
  }
  for (const name of ['loading', 'exhausted', 'removed', 'showImages']) {
    assert.equal(S[name].has('a|INBOX'), false, name); assert.equal(S[name].has('b|INBOX'), true, name);
  }
  assert.ok(S.listSeq.get('a|INBOX') > 4); assert.equal(S.listSeq.get('b|INBOX'), 2);
  assert.ok(S.folderSeq.get('a') > 3); assert.equal(S.folderSeq.get('b'), 6);
  assert.equal(S.folders.a, undefined); assert.equal(S.folders.b.length, 1); assert.equal(S.folderErrors.has('a'), false);
  assert.equal(S.results.has('__search'), false); assert.equal(S.searchPage, null);
  assert.ok(S.searchSeq > 9); assert.equal(S.searchErrors.length, 0); assert.equal(S.query, 'old query');
  assert.equal(S.sel, null); assert.equal(S.msg, null); assert.equal(S.thread, null);
  assert.equal(S.msgLoading, false); assert.equal(S.msgError, ''); assert.equal(S.readerHintUntil, 0);
  assert.equal(S.batch.size, 0); assert.equal(S.batchAnchor, null);
  assert.equal(S.live.a.unread, 0); assert.equal(S.live.a.state, 'connecting'); assert.equal(S.live.b.unread, 8);
});

test('password or SMTP repair with the same receive identity preserves reading and search state', () => {
  const t = setup(), { S } = t;
  const saved = { items: [mail()], nextCursor: 'keep' };
  S.lists.set('a|INBOX', saved.items); S.results.set('__search', saved); S.localSnapshots.set('a|INBOX', saved);
  S.searchPage = saved; S.searchSeq = 4; S.sel = mail(); S.msg = { text: 'keep body' };
  t.adoptRepairedAccount({ ...S.accounts[0], connectionRevision: 2 });
  assert.equal(S.lists.get('a|INBOX'), saved.items); assert.equal(S.results.get('__search'), saved);
  assert.equal(S.localSnapshots.get('a|INBOX'), saved); assert.equal(S.searchPage, saved);
  assert.equal(S.searchSeq, 4); assert.equal(S.msg.text, 'keep body'); assert.equal(S.sel.uid, 42);
});

test('late old list completion cannot refill the new account or release its loading flag', async () => {
  const old = deferred(), current = deferred(); let count = 0;
  const t = setup(async cmd => cmd === 'list_saved_messages' ? { items: [] } : cmd === 'list_messages' ? (++count === 1 ? old.promise : current.promise) : []);
  t.S.lists.set('a|INBOX', [mail()]);
  const before = t.loadList('a', 'INBOX'); await tick();
  t.adoptRepairedAccount({ ...t.S.accounts[0], receiveEpoch: 2 });
  const after = t.loadList('a', 'INBOX'); await tick();
  assert.equal(count, 2, 'old loading state must not block the new endpoint');
  old.resolve({ items: [mail()], complete: true }); await before;
  assert.equal(t.S.lists.has('a|INBOX'), false); assert.equal(t.S.loading.has('a|INBOX'), true);
  current.resolve({ items: [{ ...mail(), receiveEpoch: 2 }], complete: true }); await after;
  assert.equal(t.S.lists.get('a|INBOX')[0].receiveEpoch, 2); assert.equal(t.S.loading.has('a|INBOX'), false);
});

test('late search failures are ignored and queued search is canceled after receive identity repair', async () => {
  const pending = deferred(), t = setup(async () => pending.promise);
  t.S.query = 'invoice'; t.requestServerSearch(); const searching = t.flush(); await tick();
  t.adoptRepairedAccount({ ...t.S.accounts[0], receiveEpoch: 2 });
  pending.reject(new Error('old endpoint failed')); await searching;
  assert.equal(t.S.searchErrors.length, 0); assert.equal(t.S.results.has('__search'), false);
  t.requestServerSearch(); assert.equal(t.timers.size, 1);
  t.adoptRepairedAccount({ ...t.S.accounts[0], receiveEpoch: 3 });
  assert.equal(t.timers.size, 0, 'old queued query must not run under new credentials');
});

test('old preview responses cannot fill new same-UID mail or block its preview request', async () => {
  const old = deferred(), current = deferred(); let count = 0;
  const t = setup(async () => ++count === 1 ? old.promise : current.promise);
  const before = t.loadPreviews('a', 'INBOX', [mail()]);
  t.adoptRepairedAccount({ ...t.S.accounts[0], receiveEpoch: 2 });
  const after = t.loadPreviews('a', 'INBOX', [{ ...mail(), receiveEpoch: 2 }]);
  assert.equal(count, 2, 'old queue must not suppress the new endpoint request');
  old.resolve([{ uid: 42, text: 'old server text' }]); await before;
  assert.equal(t.S.previews.has('a|INBOX|42'), false);
  current.resolve([{ uid: 42, text: 'new server text' }]); await after;
  assert.equal(t.S.previews.get('a|INBOX|42|2|7'), 'new server text');
});

for (const type of ['inbox', 'newMail']) test(`${type} with new UIDVALIDITY discards old pages and cached previews`, async () => {
  const t = setup(async () => []), { S } = t;
  const old = { ...mail(), uid: 1 }, fresh = { ...mail(), uidValidity: 8 };
  S.lists.set('a|INBOX', [old]); S.lists.set('a|__unread', [old]);
  S.lists.set('a|Work', [{ ...mail(), folder: 'Work' }]);
  S.localSnapshots.set('a|INBOX', { items: [old], nextCursor: 'old cursor', hasMore: true });
  S.previews.set('a|INBOX|1', 'old preview'); S.previews.set('a|Work|42', 'keep work');
  S.results.set('__search', { items: [old], nextCursor: 'old search cursor' }); S.searchPage = { coverage: ['old mailbox'] };
  S.listSeq.set('a|INBOX', 3); S.loading.add('a|INBOX'); S.morePages.set('a|INBOX', true); S.exhausted.add('a|INBOX');
  S.sel = old; S.msg = { text: 'old body' }; S.thread = { items: [old] };
  t.onMailEvent({ type, accountId: 'a', messages: [fresh] }); await tick();
  assert.deepEqual(Array.from(S.lists.get('a|INBOX'), m => m.uid), [42]);
  assert.equal(S.lists.get('a|__unread').some(m => m.uidValidity === 7), false);
  assert.equal(S.lists.get('a|Work').length, 1);
  assert.equal(S.localSnapshots.has('a|INBOX'), false); assert.equal(S.morePages.has('a|INBOX'), false); assert.equal(S.exhausted.has('a|INBOX'), false);
  assert.equal(S.loading.has('a|INBOX'), false); assert.ok(S.listSeq.get('a|INBOX') > 3);
  assert.equal(S.previews.has('a|INBOX|1'), false); assert.equal(S.previews.get('a|Work|42'), 'keep work');
  assert.equal(S.results.has('__search'), false); assert.equal(S.searchPage, null);
  assert.equal(S.sel, null); assert.equal(S.msg, null); assert.equal(S.thread, null);
});

test('same UIDVALIDITY inbox update preserves previously loaded older mail and pagination', async () => {
  const t = setup(async () => []), { S } = t;
  S.lists.set('a|INBOX', [{ ...mail(), uid: 1 }]); S.localSnapshots.set('a|INBOX', { nextCursor: 'keep' }); S.listSeq.set('a|INBOX', 3);
  t.onMailEvent({ type: 'inbox', accountId: 'a', messages: [mail()] }); await tick();
  assert.deepEqual(Array.from(S.lists.get('a|INBOX'), m => m.uid), [42, 1]);
  assert.equal(S.localSnapshots.get('a|INBOX').nextCursor, 'keep'); assert.equal(S.listSeq.get('a|INBOX'), 3);
});

test('new UIDVALIDITY rejects old in-flight previews without blocking replacement same-UID previews', async () => {
  const old = deferred(), fresh = deferred(); let count = 0;
  const t = setup(async () => ++count === 1 ? old.promise : fresh.promise);
  t.S.lists.set('a|INBOX', [mail()]);
  const pending = t.loadPreviews('a', 'INBOX', [mail()]);
  t.onMailEvent({ type: 'inbox', accountId: 'a', messages: [{ ...mail(), uidValidity: 8 }] });
  assert.equal(count, 2);
  old.resolve([{ uid: 42, text: 'old mailbox text' }]); await pending;
  assert.equal(t.S.previews.has('a|INBOX|42'), false);
  fresh.resolve([{ uid: 42, text: 'new mailbox text' }]); await tick();
  assert.equal(t.S.previews.get('a|INBOX|42|1|8'), 'new mailbox text');
});
