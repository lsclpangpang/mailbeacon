const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const modulePath = path.join(__dirname, '../ui/sidebar-counts.js');
const api = fs.existsSync(modulePath) ? require(modulePath) : {};
const views = require('../ui/views.js');
const productivity = require('../ui/productivity-panels.js');

const count = (value, exact = true, snapshotAt = 100) => ({ value, exact, snapshotAt });
const unknown = () => count(null, false, null);
const folder = (name, value, starred = 0) => ({ folder: name, uidValidity: 12, count: count(value), starred: count(starred), attachments: count(0) });
function state() {
  return {
    accounts: [{ id: 'a', receiveEpoch: 2 }, { id: 'b', receiveEpoch: 3 }], acct: 'all',
    live: { a: { unread: 4 }, b: { unread: 6 } },
    folders: {
      a: [{ path: 'INBOX', role: 'inbox' }, { path: 'Drafts', role: 'drafts' }, { path: '__local_drafts', role: 'local-drafts' }, { path: 'Sent', role: 'sent' }, { path: 'Archive', role: 'archive' }],
      b: [{ path: 'INBOX', role: 'inbox' }, { path: '__local_drafts', role: 'local-drafts' }],
    },
    sidebarCounts: new Map([
      ['a', { accountId: 'a', receiveEpoch: 2, folders: [folder('INBOX', 800, 3), folder('Drafts', 12), folder('__local_drafts', 5), folder('Sent', 1040), folder('Archive', 90, 40)], starred: count(3), localDrafts: count(5), attachments: count(50), sendRecords: count(300), outbox: count(99) }],
      ['b', { accountId: 'b', receiveEpoch: 3, folders: [folder('INBOX', 900, 7), folder('__local_drafts', 2)], starred: count(7), localDrafts: count(2), attachments: count(20), sendRecords: count(200), outbox: count(99) }],
    ]),
    outbox: [{ accountId: 'a', id: 1 }, { accountId: 'a', id: 2 }, { accountId: 'b', id: 1 }],
    mailReminders: [], remindersLoaded: true,
  };
}

test('SC01: counts are resolved independently of first pages, search, filters and thread grouping', () => {
  assert.equal(typeof api.resolve, 'function');
  const s = state();
  for (const options of [{}, { query: 'missing', searching: true, filter: 'unread', threads: true }, { query: '', filter: 'attachment', threads: false }]) {
    Object.assign(s, options, { lists: new Map([['a|INBOX', []]]), results: new Map(), exhausted: new Set(), morePages: new Map([['a|INBOX', true]]) });
    assert.deepEqual(api.resolve(s, '__starred'), count(10));
    assert.deepEqual(api.resolve(s, '__drafts'), count(19));
    assert.deepEqual(api.resolve(s, '__sent'), count(1040));
    assert.deepEqual(api.resolve(s, '__attachments'), count(70));
    assert.deepEqual(api.resolve(s, 'send-records'), count(500));
  }
});

test('SC02: inbox and unread retain live unread semantics, not total messages or snooze visibility', () => {
  const s = state();
  for (const key of ['INBOX', '__unread']) {
    assert.deepEqual(api.resolve(s, key), count(10, true, null));
    s.acct = 'a';
    assert.deepEqual(api.resolve(s, key), count(4, true, null));
    s.live.a.unread = 0;
    assert.deepEqual(api.resolve(s, key), count(0, true, null));
    s.acct = 'all'; s.live.a.unread = 4;
  }
});

test('SC03: absent, invalid or epoch-mismatched live unread values remain unknown', () => {
  for (const live of [{}, { unread: null }, { unread: NaN }, { unread: -1 }, { unread: '0' }, { unread: 9, receiveEpoch: 1 }]) {
    const s = state(); s.acct = 'a'; s.live.a = live;
    assert.deepEqual(api.resolve(s, '__unread'), unknown());
  }
});

test('SC04: missing account data cannot make a mixed aggregate look like an exact zero', () => {
  const s = state(); s.sidebarCounts.delete('b');
  assert.deepEqual(api.resolve(s, '__attachments'), count(50, false));
  s.sidebarCounts.get('a').attachments = count(0);
  assert.deepEqual(api.resolve(s, '__attachments'), count(null, false, 100));
  s.live = { a: { unread: 0 } };
  assert.deepEqual(api.resolve(s, '__unread'), unknown());
  s.live.a.unread = 8;
  assert.deepEqual(api.resolve(s, '__unread'), count(8, false, null));
});

test('SC05: stale account epochs and mismatched account identities invalidate backend counts', () => {
  const s = state(); s.acct = 'a';
  s.sidebarCounts.get('a').receiveEpoch = 1;
  for (const key of ['__starred', '__drafts', '__sent', '__attachments', 'send-records', 'Archive', '__local_drafts']) assert.deepEqual(api.resolve(s, key), unknown(), key);
  s.sidebarCounts.get('a').receiveEpoch = 2;
  s.sidebarCounts.get('a').accountId = 'b';
  assert.deepEqual(api.resolve(s, '__starred'), unknown());
});

test('SC06: actual folders and local drafts use complete scoped counts', () => {
  const s = state(); s.acct = 'a';
  assert.deepEqual(api.resolve(s, 'Archive'), count(90));
  assert.deepEqual(api.resolve(s, '__local_drafts'), count(5));
  s.sidebarCounts.get('a').folders.find(f => f.folder === 'Archive').count = count(0);
  assert.deepEqual(api.resolve(s, 'Archive'), count(0));
  s.sidebarCounts.get('a').folders = [];
  assert.deepEqual(api.resolve(s, 'Archive'), unknown());
});

test('SC07: draft and sent aggregates follow views.sources and never add local drafts twice', () => {
  const s = state();
  s.folders.a.push({ path: '__local_drafts', role: 'drafts' });
  assert.equal(views.sources('__drafts', s.folders.a).filter(f => f.folder === '__local_drafts').length, 2);
  assert.deepEqual(api.resolve(s, '__drafts'), count(19));
  s.folders.a = s.folders.a.filter(f => f.path !== '__local_drafts');
  assert.deepEqual(api.resolve(s, '__drafts'), count(14));
  delete s.folders.b;
  assert.deepEqual(api.resolve(s, '__sent'), count(1040, false));
  s.acct = 'b'; s.folders.b = [];
  assert.deepEqual(api.resolve(s, '__sent'), count(0, true, null));
});

test('SC08: starred count uses the backend INBOX-derived value, never other folders', () => {
  const s = state(); s.acct = 'a';
  assert.deepEqual(api.resolve(s, '__starred'), count(3));
  s.sidebarCounts.get('a').starred = count(0);
  assert.deepEqual(api.resolve(s, '__starred'), count(0));
});

test('SC09: partial snapshots remain partial and preserve the oldest represented snapshot time', () => {
  const s = state(); s.sidebarCounts.get('a').attachments = count(11, false, 80);
  s.sidebarCounts.get('b').attachments = count(7, true, 120);
  assert.deepEqual(api.resolve(s, '__attachments'), count(18, false, 80));
  s.acct = 'a'; s.sidebarCounts.get('a').attachments = count(null, false, 80);
  assert.deepEqual(api.resolve(s, '__attachments'), count(null, false, 80));
});

test('SC10: full outbox records are scoped by account and do not borrow cached backend counts', () => {
  const s = state();
  assert.deepEqual(api.resolve(s, '__outbox'), count(3, true, null));
  s.acct = 'b'; assert.deepEqual(api.resolve(s, '__outbox'), count(1, true, null));
  s.outbox = []; assert.deepEqual(api.resolve(s, '__outbox'), count(0, true, null));
});

test('SC11: reminder badges match the reminder panel for every supported state, kind and scope', () => {
  const s = state();
  for (const accountId of ['a', 'b']) for (const kind of ['snooze', 'followup']) for (const status of ['scheduled', 'checking', 'due', 'orphaned', 'completed', 'cancelled']) {
    s.mailReminders.push({ id: `${accountId}-${kind}-${status}`, accountId, kind, state: status, dueAt: 1 });
  }
  for (const acct of ['all', 'a', 'b']) for (const [key, kind] of [['__snoozed', 'snoozed'], ['__reminders', 'followup']]) {
    s.acct = acct;
    const html = productivity.reminderList(s.mailReminders, { kind, accountId: acct === 'all' ? null : acct, formatDate: String });
    const panelCount = (html.match(/data-reminder-id=/g) || []).length;
    assert.deepEqual(api.resolve(s, key), count(panelCount, true, null), `${acct} ${key}`);
  }
});

test('SC12: reminder initialization is unknown until the full read succeeds; intervening events are partial', () => {
  const s = state(); s.remindersLoaded = false;
  assert.deepEqual(api.resolve(s, '__snoozed'), unknown());
  assert.deepEqual(api.resolve(s, '__reminders'), unknown());
  s.mailReminders.push({ id: 'event', accountId: 'a', kind: 'followup', state: 'scheduled' });
  assert.deepEqual(api.resolve(s, '__reminders'), count(1, false, null));
  s.remindersLoaded = true;
  assert.deepEqual(api.resolve(s, '__reminders'), count(1, true, null));
});

test('SC13: badge models display exact zero, partial lower bounds and unknown distinctly', () => {
  assert.equal(typeof api.badge, 'function');
  assert.deepEqual(api.badge(count(0)), { text: '0', title: '共 0 项', ariaLabel: '共 0 项', partial: false });
  assert.deepEqual(api.badge(count(128, false)), { text: '128+', title: '已知至少 128 项，完整数量尚未确认', ariaLabel: '已知至少 128 项，完整数量尚未确认', partial: true });
  assert.deepEqual(api.badge(unknown()), { text: '—', title: '数量尚未确认', ariaLabel: '数量尚未确认', partial: true });
  assert.equal(api.badge(count(0, false)).text, '—');
  assert.equal(api.badge(null), null);
});

test('SC14: invalid count inputs cannot become markup, NaN or untrustworthy exact zero', () => {
  for (const value of ['<img onerror=alert(1)>', '9', -1, 1.5, Infinity, NaN, Number.MAX_SAFE_INTEGER + 1]) {
    const model = api.badge(count(value));
    assert.equal(model.text, '—');
    assert.doesNotMatch(JSON.stringify(model), /<|>|NaN|Infinity/);
  }
});

test('SC15: non-quantity actions have no badge and an empty account scope has exact zero', () => {
  const s = state();
  for (const action of ['search', 'contacts', 'rules', 'settings', 'compose', 'create-folder', 'toggle-sidebar', '__saved_view', null, undefined]) assert.equal(api.resolve(s, action), null);
  s.accounts = []; s.sidebarCounts.clear();
  assert.deepEqual(api.resolve(s, '__starred'), count(0, true, null));
  s.acct = 'removed-account';
  assert.deepEqual(api.resolve(s, '__starred'), unknown());
});

test('SC16: the browser module exposes the same UMD API', () => {
  assert.ok(fs.existsSync(modulePath));
  const context = vm.createContext({ window: { MailBeaconViews: views } });
  vm.runInContext(fs.readFileSync(modulePath, 'utf8'), context);
  assert.equal(typeof context.window.MailBeaconSidebarCounts.resolve, 'function');
  assert.equal(context.window.MailBeaconSidebarCounts.badge(count(4)).text, '4');
});

test('SC17: attachment badges name discovered files and their scope without changing other partial counts', () => {
  const partial = api.badge(count(4, false), '__attachments');
  assert.equal(partial.text, '已发现 4');
  assert.match(partial.title, /4 个附件/);
  assert.match(partial.title, /IMAP/);
  assert.match(partial.title, /不含草稿、垃圾邮件和已删除/);
  assert.match(partial.title, /完整数量尚未确认/);
  assert.equal(partial.ariaLabel, partial.title);
  assert.equal(api.badge(count(4), '__attachments').text, '4');
  assert.match(api.badge(count(4), '__attachments').title, /共 4 个附件/);
  assert.match(api.badge(unknown(), '__attachments').title, /附件数量尚未确认/);
  assert.equal(api.badge(count(4, false), '__starred').text, '4+');
});

test('SC18: attachment scope excludes POP3 just like the attachment center', () => {
  const s = state(); s.accounts[1].receiveProtocol = 'pop3';
  s.sidebarCounts.delete('b');
  assert.deepEqual(api.resolve(s, '__attachments'), count(50));
  s.sidebarCounts.set('b', { accountId: 'b', receiveEpoch: 3, attachments: count(700) });
  assert.deepEqual(api.resolve(s, '__attachments'), count(50));
  s.acct = 'b';
  assert.deepEqual(api.resolve(s, '__attachments'), count(0, true, null));
  s.acct = 'removed';
  assert.deepEqual(api.resolve(s, '__attachments'), unknown());
});
