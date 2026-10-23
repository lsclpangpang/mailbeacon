const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const message = (uid, extra = {}) => ({ accountId: 'a', receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid, date: uid, fromAddr: 'sender@example.invalid', subject: `Mail ${uid}`, seen: false, ...extra });
const messages = (count, first = count) => Array.from({ length: count }, (_, index) => message(first - index));
const settle = () => new Promise(resolve => setImmediate(resolve));

function clock() {
  let next = 0;
  const tasks = new Map();
  return {
    setTimeout(fn, delay) { tasks.set(++next, { fn, delay }); return next; },
    clearTimeout(id) { tasks.delete(id); },
    async run(delay) {
      for (const [id, task] of [...tasks]) if (task.delay === delay && tasks.delete(id)) await task.fn();
    },
  };
}

// Exercise real event dispatch, list loading, snapshots, grouping, list HTML and
// merge-button handling. The DOM only supplies the scroll container; unrelated
// reader/sidebar rendering and preview network requests are outside this test.
function app(respond = async () => [], timers = { setTimeout, clearTimeout }) {
  const calls = [], listeners = {}, errors = [];
  const newRows = () => ({
    scrollTop: 0, clientHeight: 600, scrollHeight: 20000, isConnected: true,
    addEventListener() {}, focus() {}, classList: { add() {}, remove() {} },
    getBoundingClientRect: () => ({ top: 100, bottom: 700 }), setPointerCapture() {}, releasePointerCapture() {},
  });
  const list = {
    html: '', rows: newRows(),
    get innerHTML() { return this.html; },
    set innerHTML(value) { this.html = value; this.rows = newRows(); },
    querySelector(selector) { return selector === '.rows' ? this.rows : null; },
    querySelectorAll() { return []; },
  };
  const overlay = { innerHTML: '', childElementCount: 0 };
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'),
      MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'),
      MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'),
      MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'),
      MailBeaconExtras: require('../ui/compose-extras.js'), MailBeaconReaderMotion: require('../ui/reader-motion.js'),
      MailBeaconProductivity: require('../ui/productivity-panels.js'), MailBeaconAccountRepair: require('../ui/account-repair.js'),
      __TAURI__: { core: { invoke: async (cmd, args) => { calls.push({ cmd, args }); return respond(cmd, args); } }, event: { listen: async () => {} } },
    },
    document: {
      activeElement: null,
      querySelector: selector => selector === '#list' ? list : ['#list .rows', '.rows'].includes(selector) ? list.rows : selector === '#overlay' ? overlay : null,
      querySelectorAll: () => [],
      addEventListener: (name, listener) => { listeners[name] = listener; },
    },
    console, setTimeout: timers.setTimeout, clearTimeout: timers.clearTimeout, errors,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = () => renderList();
    syncNativeMenu = updateBadge = loadPreviews = loadWindowPreviews = hideReaderHint = cancelReaderScroll = () => {};
    toast = (text, isError) => { if (isError) errors.push(String(text)); };
    globalThis.api = { S, onMailEvent, loadList, requestServerSearch, currentList, listGroups, renderList, dropMsg,
      enterBatchMode, startBatchDrag, moveBatchDrag, finishBatchDrag,
      get pending() { return visibleListPending; }, get units() { return listWindow?.units || []; },
      get rowHeight() { return ROW_H; } };
  `, context);
  const api = context.api;
  Object.assign(api.S, {
    accounts: [{ id: 'a', email: 'a@example.invalid', receiveEpoch: 1, connectionRevision: 1 }, { id: 'b', email: 'b@example.invalid', receiveEpoch: 1, connectionRevision: 1 }],
    acct: 'a', folder: 'INBOX',
    folders: { a: [{ path: 'INBOX', name: '收件箱', role: 'inbox' }], b: [{ path: 'INBOX', name: '收件箱', role: 'inbox' }] },
  });
  return {
    api, list, calls, errors,
    show(items, { reading = true } = {}) {
      api.S.lists.set('a|INBOX', items);
      api.S.sel = reading ? { ...items[0] } : null;
      api.renderList();
    },
    receive(items, fields = {}) { api.onMailEvent({ type: 'newMail', accountId: 'a', receiveEpoch: 1, connectionRevision: 1, messages: items, ...fields }); },
    async merge() {
      const target = { dataset: { action: 'merge-list' }, classList: { contains: () => false }, closest: selector => selector === '[data-action]' ? target : null };
      await listeners.click({ target, detail: 0 });
    },
  };
}

for (const type of ['searchIndex', 'listIndex']) {
  test(`${type} completion fills 100 historical rows to 3338 through loadList without a new-mail prompt`, async () => {
    const all = messages(3338);
    const mail = app(async cmd => { assert.equal(cmd, 'list_messages'); return { items: all, complete: true }; });
    mail.show(all.slice(0, 100));
    mail.list.rows.scrollTop = 30 * mail.api.rowHeight + 13;
    mail.api.onMailEvent({ type, accountId: 'a', receiveEpoch: 1, connectionRevision: 1, folder: 'INBOX', indexing: { running: false, complete: true, indexed: 3338, total: 3338 } });
    await settle();
    assert.equal(mail.calls.length, 1);
    assert.equal(mail.calls[0].args.forceRefresh, undefined, 'index completion reads the completed index instead of restarting it');
    assert.equal(mail.api.currentList().length, 3338);
    assert.equal(mail.api.units.length, 3338, 'all identities feed the virtual list, not only the first page');
    assert.equal(mail.api.pending, 0);
    assert.doesNotMatch(mail.list.innerHTML, /list-updates|封更新|封新邮件/);
    assert.equal(mail.list.rows.scrollTop, 30 * mail.api.rowHeight + 13);
    assert.deepEqual(mail.errors, []);
  });
}

test('loading older mail appends history while reading without a pending-update banner', async () => {
  const older = messages(100);
  const mail = app(async () => ({ items: older, complete: true }));
  mail.show(messages(100, 200));
  mail.list.rows.scrollTop = 20 * mail.api.rowHeight + 7;
  await mail.api.loadList('a', 'INBOX', { more: true });
  assert.equal(mail.calls[0].args.before, 101);
  assert.equal(mail.api.currentList().length, 200);
  assert.equal(mail.api.pending, 0);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);
  assert.equal(mail.list.rows.scrollTop, 20 * mail.api.rowHeight + 7);
});

test('mixed new-mail and history refresh counts only one arrival until a real merge click, without marking it read', async () => {
  const initial = messages(100, 200), fresh = message(201), history = messages(100);
  const mail = app(async () => ({ items: [fresh, ...initial, ...history], complete: true }));
  mail.show(initial);
  mail.list.rows.scrollTop = 20 * mail.api.rowHeight + 11;
  mail.receive([fresh]);
  assert.equal(mail.api.pending, 1);
  assert.equal(mail.api.currentList().some(m => m.uid === fresh.uid), false);
  assert.match(mail.list.innerHTML, />有 1 封新邮件 · 点击显示<\/button>/);
  await mail.api.loadList('a', 'INBOX');
  assert.equal(mail.api.currentList().length, 200, 'history appears while the actual arrival stays deferred');
  for (let i = 0; i < 3; i++) { mail.receive([fresh]); mail.api.renderList(); }
  assert.equal(mail.api.pending, 1, 'duplicate events and repeated renders do not increase the count');
  assert.equal(mail.list.rows.scrollTop, 20 * mail.api.rowHeight + 11);
  const selected = mail.api.S.sel.uid;
  await mail.merge();
  assert.equal(mail.api.currentList().length, 201);
  assert.equal(mail.api.pending, 0);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);
  assert.equal(mail.list.rows.scrollTop, 21 * mail.api.rowHeight + 11, 'merge preserves the previously visible row and intra-row offset');
  assert.equal(mail.api.S.sel.uid, selected);
  assert.equal(fresh.seen, false);
  assert.equal(mail.calls.some(call => /mark|flag|execute_mail_operation|^get_message(?:_ref)?$/.test(call.cmd)), false);
  mail.receive([fresh]);
  assert.equal(mail.api.pending, 0, 'an already displayed identity never becomes new again');
});

test('filtered, other-account, unaccepted-connection and already-removed arrivals do not contaminate the current prompt', () => {
  const mail = app();
  mail.api.S.filter = 'unread';
  mail.show(messages(3));
  mail.receive([message(10, { seen: true })]);
  mail.receive([message(11, { accountId: 'b' })], { accountId: 'b' });
  mail.receive([message(12, { receiveEpoch: 2 })], { receiveEpoch: 2 });
  mail.receive([message(13)], { connectionRevision: 2 });
  mail.api.S.removed.add('a|INBOX|14');
  mail.receive([message(14)]);
  assert.equal(mail.api.pending, 0);
  assert.deepEqual([...mail.api.currentList()].map(m => m.uid), [3, 2, 1]);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);
  mail.api.S.filter = 'all'; mail.api.renderList();
  assert.equal(mail.api.pending, 0, 'changing filters cannot revive a discarded pending mark');
  assert.equal(mail.api.currentList().some(m => m.uid === 10), true);
});

test('late receive events from an old account epoch or connection cannot create a new-mail prompt', () => {
  const mail = app();
  Object.assign(mail.api.S.accounts[0], { receiveEpoch: 2, connectionRevision: 3 });
  mail.show(messages(3).map(m => ({ ...m, receiveEpoch: 2 })));
  mail.receive([message(4)], { receiveEpoch: 1, connectionRevision: 3 });
  mail.receive([message(5, { receiveEpoch: 2 })], { receiveEpoch: 2, connectionRevision: 2 });
  assert.equal(mail.api.pending, 0);
  assert.deepEqual([...mail.api.currentList()].map(m => [m.uid, m.receiveEpoch]), [[3, 2], [2, 2], [1, 2]]);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);
});

test('removing a pending arrival and replacing UIDVALIDITY clears its old identity without a phantom prompt', () => {
  const mail = app();
  mail.show(messages(3));
  mail.receive([message(4)]);
  assert.equal(mail.api.pending, 1);
  mail.api.dropMsg('a', 'INBOX', 4); mail.api.renderList();
  assert.equal(mail.api.pending, 0);
  mail.receive([message(5)]);
  assert.equal(mail.api.pending, 1);
  mail.api.onMailEvent({ type: 'inbox', accountId: 'a', receiveEpoch: 1, connectionRevision: 1, messages: [message(5, { uidValidity: 8, subject: 'Replacement generation' })] });
  assert.equal(mail.api.pending, 0);
  assert.equal(mail.api.S.sel, null);
  assert.deepEqual([...mail.api.currentList()].map(m => [m.uid, m.uidValidity]), [[5, 8]]);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);
});

test('new replies in an existing conversation do not replace its representative before merge', async () => {
  const mail = app();
  mail.api.S.threads = true;
  const root = message(1, { messageId: '<root>', subject: 'Conversation' });
  const previous = message(2, { messageId: '<previous>', inReplyTo: '<root>', subject: 'Re: Conversation' });
  mail.show([previous, root]);
  assert.equal(mail.api.listGroups()[0].latest.uid, 2);
  mail.receive([message(3, { messageId: '<new>', inReplyTo: '<root>', subject: 'Re: Conversation' })]);
  assert.equal(mail.api.pending, 1);
  assert.equal(mail.api.listGroups().length, 1);
  assert.equal(mail.api.listGroups()[0].latest.uid, 2);
  assert.equal(mail.api.listGroups()[0].count, 2);
  await mail.merge();
  assert.equal(mail.api.pending, 0);
  assert.equal(mail.api.listGroups()[0].latest.uid, 3);
  assert.equal(mail.api.listGroups()[0].count, 3);
});

test('a real drag defers its last-moment arrival even when no snapshot read happens before pointer release', async () => {
  const initial = messages(3), fresh = message(4), older = message(0);
  const mail = app(async () => ({ items: [fresh, ...initial, older], complete: true }));
  mail.show(initial, { reading: false });
  mail.api.enterBatchMode();
  const rows = mail.list.rows;
  const target = { dataset: { open: 'a|3|INBOX' }, closest: selector => selector === '.rows' ? rows : selector === '.row[data-open]' ? target : null };
  const pointer = { target, button: 0, pointerId: 1, pointerType: 'mouse', clientX: 100, clientY: 110, preventDefault() {} };
  mail.api.startBatchDrag(pointer);
  mail.api.moveBatchDrag({ ...pointer, clientY: 190 });
  const frozenHtml = mail.list.innerHTML;
  mail.receive([fresh]);
  await mail.api.loadList('a', 'INBOX');
  assert.equal(mail.list.innerHTML, frozenHtml, 'neither history nor arrivals replace pointer hit targets during the drag');
  // Do not call currentList() here: release must work even without an extra
  // pointer move (and its incidental snapshot read) after the receive event.
  mail.api.finishBatchDrag(false);
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(mail.api.pending, 1);
  assert.deepEqual([...mail.api.currentList()].map(m => m.uid), [3, 2, 1, 0]);
  assert.match(mail.list.innerHTML, /有 1 封新邮件 · 点击显示/);
  await mail.merge();
  assert.deepEqual([...mail.api.currentList()].map(m => m.uid), [4, 3, 2, 1, 0]);
});

test('a received message waits for its asynchronous search match while historical hits merge without a prompt', async () => {
  const initial = messages(100, 200).map(m => ({ ...m, subject: `Contract ${m.uid}` }));
  const history = messages(100).map(m => ({ ...m, subject: `Contract ${m.uid}` }));
  const fresh = message(201, { subject: 'Contract just received' });
  const timers = clock();
  let result = { items: initial, coverage: [] };
  const mail = app(async (cmd, args) => {
    assert.equal(cmd, 'search_mail');
    assert.equal(args.input.query, 'Contract');
    return result;
  }, timers);
  Object.assign(mail.api.S, { searching: true, query: 'Contract' });
  mail.show(initial);
  mail.api.requestServerSearch();
  await timers.run(180);
  assert.equal(mail.api.currentList().length, 100);
  const selected = mail.api.S.sel.uid;
  const scroll = 20 * mail.api.rowHeight + 9;
  mail.list.rows.scrollTop = scroll;

  mail.receive([fresh]);
  mail.receive([fresh]);
  mail.api.renderList();
  assert.equal(mail.api.pending, 0, 'a receive event is not a search match until the result contains that identity');
  assert.equal(mail.api.currentList().length, 100);
  assert.doesNotMatch(mail.list.innerHTML, /list-updates/);

  const progress = (indexed, complete = false) => mail.api.onMailEvent({
    type: 'searchIndex', accountId: 'a', receiveEpoch: 1, connectionRevision: 1, folder: 'INBOX',
    indexing: { indexed, total: 201, complete, running: !complete },
  });
  result = { items: [...initial, ...history], coverage: [] };
  progress(200);
  await timers.run(150); await timers.run(180);
  assert.equal(mail.api.currentList().length, 200, 'an intermediate index result only adds historical matches');
  assert.equal(mail.api.pending, 0);
  assert.equal(mail.list.rows.scrollTop, scroll);

  let deliver;
  result = new Promise(resolve => { deliver = resolve; });
  progress(201, true);
  await timers.run(150);
  const searching = timers.run(180);
  assert.equal(mail.calls.length, 3, 'initial search and both real index-triggered refreshes reach the search bridge');
  mail.api.renderList();
  assert.equal(mail.api.pending, 0, 'in-flight results cannot be counted before they arrive');
  deliver({ items: [fresh, ...initial, ...history], coverage: [] });
  await searching;
  assert.equal(mail.api.pending, 1, 'the earlier receive event remains known until its matching search result arrives');
  assert.equal(mail.api.currentList().length, 200);
  assert.equal(mail.api.currentList().some(m => m.uid === 201), false);
  assert.match(mail.list.innerHTML, /有 1 封新邮件 · 点击显示/);
  assert.equal(mail.list.rows.scrollTop, scroll);
  assert.equal(mail.api.S.sel.uid, selected);
  mail.receive([fresh]); mail.api.renderList();
  assert.equal(mail.api.pending, 1);

  await mail.merge();
  assert.equal(mail.api.pending, 0);
  assert.equal(mail.api.currentList().length, 201);
  assert.equal(mail.list.rows.scrollTop, scroll + mail.api.rowHeight);
  assert.equal(mail.api.S.sel.uid, selected);
  assert.equal(fresh.seen, false);
  assert.ok(mail.calls.every(call => call.cmd === 'search_mail'), 'neither refresh nor merge opens a message or marks it read');
  assert.deepEqual(mail.errors, []);
});
