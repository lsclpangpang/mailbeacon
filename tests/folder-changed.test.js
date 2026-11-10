const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const message = (uid, extra = {}) => ({ accountId: 'a', receiveEpoch: 1, folder: 'special', uidValidity: 7, uid, date: uid, seen: true, ...extra });
const saved = (items, extra = {}) => ({ items, coverage: [], hasMore: false, nextCursor: null, headersComplete: true, revision: 1, ...extra });
const changed = (extra = {}) => ({ type: 'folderChanged', accountId: 'a', folder: 'special', receiveEpoch: 1, connectionRevision: 2, ...extra });

function app(respond = () => ({ items: [], complete: false, more: false })) {
  const calls = [], sidebars = [], errors = [], tasks = new Map();
  let timerId = 0;
  const timers = {
    setTimeout(fn, delay) { tasks.set(++timerId, { fn, delay }); return timerId; },
    clearTimeout(id) { tasks.delete(id); },
    async run(delay) {
      for (const [id, task] of [...tasks]) if (task.delay === delay && tasks.delete(id)) await task.fn();
    },
  };
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'),
      MailBeaconViews: require('../ui/views.js'), MailBeaconThreads: require('../ui/threads.js'),
      MailBeaconAccountRepair: require('../ui/account-repair.js'),
      __TAURI__: {
        core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return respond(cmd, input); } },
        event: { listen: async () => {} },
      },
    },
    document: { activeElement: null, querySelector: () => null, querySelectorAll: () => [], addEventListener() {} },
    console, setTimeout: timers.setTimeout, clearTimeout: timers.clearTimeout, sidebars, errors,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = renderSidebar = updateBadge = revealSelectedRow = redrawNotifySection = hideReaderHint = cancelReaderScroll = () => {};
    loadPreviews = () => {};
    scheduleSidebarCounts = input => sidebars.push(input);
    toast = text => errors.push(String(text));
    globalThis.api = { S, loadList, moreSavedMessages, onMailEvent, requestServerSearch };
  `, context);
  const { S } = context.api;
  Object.assign(S, {
    accounts: ['a', 'b'].map(id => ({ id, receiveEpoch: 1, connectionRevision: 2, receiveProtocol: 'imap' })),
    acct: 'a', folder: 'special',
    folders: Object.fromEntries(['a', 'b'].map(id => [id, [
      { path: 'INBOX', role: 'inbox' }, { path: 'special', role: 'other' }, { path: 'Sent', role: 'sent' },
    ]])),
  });
  return { ...context.api, calls, sidebars, errors, timers };
}

test('a changed visible rule folder fetches its destination UID immediately without adding it to INBOX', async () => {
  const previous = message(89), destination = message(901);
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous]) : { items: [destination], complete: false, more: false });
  t.S.lists.set('a|special', [previous]);
  t.S.lists.set('a|INBOX', [message(200, { folder: 'INBOX' })]);
  t.S.sel = previous;
  t.S.msg = { text: 'keep the open reader' };
  t.onMailEvent(changed());
  await tick();
  const request = t.calls.find(c => c.cmd === 'list_messages');
  assert.equal(request?.input.folder, 'special');
  assert.equal(request.input.forceRefresh, true);
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [901, 89]);
  assert.deepEqual([...t.S.lists.get('a|INBOX')].map(m => m.uid), [200]);
  assert.equal(t.S.sel, previous);
  assert.equal(t.S.msg.text, 'keep the open reader');
  assert.ok(t.sidebars.some(input => input?.accountId === 'a' && input.remote && input.force));
});

test('a complete response started before the folder change cannot erase the fresh destination row', async () => {
  const oldResponse = deferred();
  let requests = 0;
  const previous = message(89), destination = message(901);
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous])
    : ++requests === 1 ? oldResponse.promise : { items: [destination, previous], complete: true, more: false });
  t.S.lists.set('a|special', [previous]);
  const original = t.loadList('a', 'special');
  await tick();
  t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 2);
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [901, 89]);
  oldResponse.resolve({ items: [previous], complete: true, more: false });
  await original;
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [901, 89]);
});

test('a burst of folder changes shares one active fetch and one follow-up with the latest destination UID', async () => {
  const firstResponse = deferred(), previous = message(89);
  let requests = 0;
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous])
    : ++requests === 1 ? firstResponse.promise : { items: [message(909)], complete: false, more: false });
  t.S.lists.set('a|special', [previous]);
  t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 1);
  for (let i = 0; i < 8; i++) t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 1, 'repeated MOVE/STATUS signals must not start parallel target reads');
  firstResponse.resolve({ items: [message(901)], complete: true, more: false });
  await tick();
  assert.equal(requests, 2, 'the dirty folder needs exactly one latest read after the active snapshot');
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [909, 89]);
  await tick();
  assert.equal(requests, 2);
});

test('a change after the first server snapshot requests a follow-up instead of losing the newer arrival', async () => {
  const firstResponse = deferred(), secondResponse = deferred(), previous = message(89);
  let requests = 0;
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous])
    : ++requests === 1 ? firstResponse.promise : secondResponse.promise);
  t.S.lists.set('a|special', [previous]);
  t.onMailEvent(changed());
  await tick();
  // The server has captured UID 901, but UID 902 arrives before that reply is delivered.
  t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 1);
  firstResponse.resolve({ items: [message(901)], complete: true, more: false });
  await tick();
  assert.equal(requests, 2);
  secondResponse.resolve({ items: [message(902), message(901)], complete: false, more: false });
  await tick();
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [902, 901, 89]);
});

test('a single folder change does not add a follow-up after its fetch completes', async () => {
  const firstResponse = deferred();
  let requests = 0;
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([])
    : (++requests, firstResponse.promise));
  t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 1);
  firstResponse.resolve({ items: [message(901)], complete: false, more: false });
  await tick();
  await tick();
  assert.equal(requests, 1);
  assert.equal(t.S.lists.get('a|special')[0].uid, 901);
});

for (const [reason, leave] of [
  ['leaving the folder', S => { S.folder = 'INBOX'; }],
  ['selecting another account', S => { S.acct = 'b'; }],
  ['changing the mailbox epoch', S => { S.accounts[0].receiveEpoch = 2; }],
  ['replacing the connection', S => { S.accounts[0].connectionRevision = 3; }],
]) {
  test(`a pending folder follow-up stops after ${reason}`, async () => {
    const firstResponse = deferred(), previous = message(89);
    let requests = 0;
    const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous])
      : ++requests === 1 ? firstResponse.promise : { items: [message(902)], complete: false, more: false });
    t.S.lists.set('a|special', [previous]);
    t.onMailEvent(changed());
    await tick();
    t.onMailEvent(changed());
    leave(t.S);
    firstResponse.resolve({ items: [message(901)], complete: true, more: false });
    await tick();
    assert.equal(requests, 1, 'an obsolete target must not continue its pending network refresh');
    assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [89]);
  });
}

test('a hidden changed folder cancels its old response and keeps loaded rows until it is opened', async () => {
  const oldResponse = deferred();
  let requests = 0;
  const previous = message(89), destination = message(901);
  const t = app(cmd => cmd === 'list_saved_messages' ? saved([previous])
    : ++requests === 1 ? oldResponse.promise : { items: [destination], complete: false, more: false });
  t.S.folder = 'INBOX';
  t.S.lists.set('a|special', [previous]);
  const original = t.loadList('a', 'special');
  await tick();
  t.onMailEvent(changed());
  await tick();
  assert.equal(requests, 1, 'an unrelated visible folder must not start a special fetch');
  oldResponse.resolve({ items: [], complete: true, more: false });
  await original;
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [89]);
  assert.equal(t.S.loading.has('a|special'), false);
  t.S.folder = 'special';
  await t.loadList('a', 'special');
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [901, 89]);
});

test('a failed folder refresh retains all loaded offline history and invalidates the old local cursor', async () => {
  const previous = Array.from({ length: 2700 }, (_, i) => message(2700 - i));
  const t = app(cmd => {
    if (cmd === 'list_saved_messages') return saved(previous.slice(0, 100), { hasMore: true, nextCursor: 'fresh-cursor' });
    throw new Error('offline');
  });
  t.S.lists.set('a|special', previous);
  t.S.localSnapshots.set('a|special', saved(previous, { hasMore: true, nextCursor: 'obsolete-cursor' }));
  t.S.localPageSeq.set('a|special', 4);
  t.S.exhausted.add('a|special');
  t.onMailEvent(changed());
  assert.equal(t.S.lists.get('a|special').length, 2700);
  assert.ok(t.S.localPageSeq.get('a|special') > 4);
  await tick();
  assert.equal(t.S.lists.get('a|special').length, 2700);
  assert.equal(t.S.exhausted.has('a|special'), false);
  assert.notEqual(t.S.localSnapshots.get('a|special')?.nextCursor, 'obsolete-cursor');
  assert.match(t.S.listErrors.get('a|special'), /offline/);
});

test('a local history page started before the folder change cannot restore its obsolete cursor', async () => {
  const pageResponse = deferred(), previous = message(89);
  const t = app((cmd, input) => cmd === 'list_saved_messages'
    ? input.cursor ? pageResponse.promise : saved([previous], { hasMore: true, nextCursor: 'new-cursor' })
    : { items: [message(901)], complete: false, more: false });
  t.S.lists.set('a|special', [previous]);
  t.S.localSnapshots.set('a|special', saved([previous], { hasMore: true, nextCursor: 'old-cursor' }));
  const more = t.moreSavedMessages('a', 'special');
  await tick();
  t.onMailEvent(changed());
  await tick();
  pageResponse.resolve(saved([message(88)], { hasMore: true, nextCursor: 'obsolete-cursor' }));
  await more;
  assert.deepEqual([...t.S.lists.get('a|special')].map(m => m.uid), [901, 89]);
  assert.notEqual(t.S.localSnapshots.get('a|special')?.nextCursor, 'obsolete-cursor');
  assert.equal(t.S.localLoading.has('a|special'), false);
});

test('only current-account folders and virtual views containing the changed source refresh', async () => {
  const t = app((cmd, input) => cmd === 'list_saved_messages' ? saved([])
    : { items: [message(901, { folder: input.folder })], complete: false, more: false });
  t.onMailEvent(changed({ accountId: 'b' }));
  t.onMailEvent(changed({ folder: 'Sent' }));
  t.onMailEvent(changed({ accountId: 'removed' }));
  await tick();
  assert.equal(t.calls.length, 0);
  t.S.folder = '__unread';
  t.onMailEvent(changed());
  await tick();
  assert.equal(t.calls.length, 0, 'special must not expand the INBOX-only unread view');
  t.S.folder = '__sent';
  t.onMailEvent(changed({ folder: 'Sent' }));
  await tick();
  assert.ok(t.calls.some(c => c.cmd === 'list_messages' && c.input.folder === 'Sent' && c.input.forceRefresh));
  assert.equal(t.S.lists.has('a|INBOX'), false);
});

test('old connection and mailbox-epoch folder events cannot trigger a refresh or invalidate an active request', async () => {
  const t = app();
  t.S.listSeq.set('a|special', 3);
  t.onMailEvent(changed({ connectionRevision: 1 }));
  t.onMailEvent(changed({ receiveEpoch: 2 }));
  await tick();
  assert.equal(t.calls.length, 0);
  assert.equal(t.S.listSeq.get('a|special'), 3);
  assert.equal(t.sidebars.length, 0);
});

test('a folder change rejects an older matching search response and schedules the current search again', async () => {
  const previousSearch = deferred();
  const t = app(cmd => cmd === 'search_mail' ? previousSearch.promise : saved([]));
  t.S.query = 'salary';
  t.S.searching = true;
  t.S.searchScope = 'account';
  t.S.folder = 'INBOX';
  t.requestServerSearch();
  const started = t.timers.run(180);
  await tick();
  const seq = t.S.searchSeq;
  t.onMailEvent(changed());
  assert.ok(t.S.searchSeq > seq);
  previousSearch.resolve({ items: [message(89)], coverage: [], hasMore: true, nextCursor: 'old-search-cursor' });
  await started;
  assert.notEqual(t.S.results.get('__search')?.nextCursor, 'old-search-cursor');
  await t.timers.run(150);
  await t.timers.run(180);
  assert.equal(t.calls.filter(c => c.cmd === 'search_mail').length, 2);
});

test('a folder change outside the current folder search leaves its result and request generation intact', () => {
  const t = app();
  t.S.folder = 'INBOX';
  t.S.query = 'salary';
  t.S.searching = true;
  t.S.searchSeq = 8;
  t.S.results.set('__search', { items: [message(20, { folder: 'INBOX' })], query: 'salary', nextCursor: 'keep' });
  t.onMailEvent(changed());
  assert.equal(t.S.searchSeq, 8);
  assert.equal(t.S.results.get('__search').nextCursor, 'keep');
});

test('completion of the changed folder index refreshes an account search after an early empty result', async () => {
  let searches = 0;
  const t = app(cmd => cmd === 'search_mail'
    ? { items: ++searches === 1 ? [] : [message(901)], coverage: [], hasMore: false, nextCursor: null }
    : saved([]));
  t.S.folder = 'INBOX';
  t.S.query = 'salary';
  t.S.searching = true;
  t.S.searchScope = 'account';
  t.onMailEvent(changed());
  await t.timers.run(150);
  await t.timers.run(180);
  assert.equal(searches, 1);
  assert.equal(t.S.results.get('__search').items.length, 0);
  t.onMailEvent({ type: 'listIndex', accountId: 'a', folder: 'special', receiveEpoch: 1, connectionRevision: 2 });
  await t.timers.run(150);
  await t.timers.run(180);
  assert.equal(searches, 2);
  assert.equal(t.S.results.get('__search').items[0].uid, 901);
});
