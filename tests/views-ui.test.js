const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const viewsHelper = require('../ui/views.js');

function app(invoke = async () => []) {
  const listeners = {};
  const calls = [];
  const elements = { '#sidebar': { innerHTML: '' }, '#list': { innerHTML: '' }, '#reader': { innerHTML: '' }, '#overlay': { innerHTML: '', querySelectorAll: () => [] } };
  const context = vm.createContext({
    window: { MailBeaconViews: viewsHelper, MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } }, event: { listen: async () => {} } } },
    document: {
      activeElement: null,
      querySelector: (sel) => elements[sel] || null,
      querySelectorAll: () => [],
      addEventListener: (name, handler) => { listeners[name] = handler; },
    },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = () => {};
    globalThis.api = { S, loadList, currentList, renderSidebar, onMailEvent, draftSaved, loadFolders, go, listWindowRange };
  `, context);
  const api = context.api;
  api.S.accounts = [{ id: 'a', email: 'me@x.com', name: 'Me' }, { id: 'b', email: 'b@y.com', name: 'B' }];
  return { ...api, calls, listeners, elements, list: () => [...api.currentList()] };
}

const msg = (accountId, folder, uid, extra = {}) => ({ accountId, folder, uid, fromName: '', fromAddr: '', to: '', subject: `s${uid}`, date: uid, seen: true, flagged: false, attachment: false, recipients: [], ...extra });
const plain = (v) => JSON.parse(JSON.stringify(v));

test('the drafts view merges the drafts folder and local drafts', async () => {
  const t = app(async (cmd, input) => {
    if (cmd === 'list_folders') return [{ path: 'INBOX', role: 'inbox' }, { path: 'Drafts', role: 'drafts' }, { path: '__local_drafts', role: 'local-drafts' }];
    if (cmd === 'list_messages') return input.folder === 'Drafts' ? [msg('a', 'Drafts', 5)] : [msg('a', '__local_drafts', 9)];
    return [];
  });
  await t.loadList('a', '__drafts');
  const requests = t.calls.filter((c) => c.cmd === 'list_messages').map((c) => plain(c.input));
  assert.deepEqual(requests.map((r) => r.folder), ['Drafts', '__local_drafts']);
  assert.deepEqual([...t.S.lists.get('a|__drafts')].map((m) => m.uid), [9, 5], '按时间排序合并');
  assert.ok(t.calls.findIndex((c) => c.cmd === 'list_folders') < t.calls.findIndex((c) => c.cmd === 'list_messages'), '先加载文件夹列表');
});

test('the unread view asks the server for unread mail and hides mail read meanwhile', async () => {
  const t = app(async (cmd) => cmd === 'list_messages' ? [msg('a', 'INBOX', 1, { seen: false }), msg('a', 'INBOX', 2, { seen: false })] : []);
  t.S.folders.a = [{ path: 'INBOX', role: 'inbox' }];
  t.S.acct = 'a';
  t.S.folder = '__unread';
  await t.loadList('a', '__unread');
  assert.deepEqual(plain(t.calls.find((c) => c.cmd === 'list_messages').input), { accountId: 'a', folder: 'INBOX', before: null, flaggedOnly: false, unreadOnly: true });
  t.S.lists.get('a|__unread').find((m) => m.uid === 1).seen = true;
  t.S.sel = null;
  assert.deepEqual(t.list().map((m) => m.uid), [2]);
  t.S.sel = { accountId: 'a', folder: 'INBOX', uid: 1 };
  assert.deepEqual(t.list().map((m) => m.uid).sort(), [1, 2], '正在看的那封不会消失');
});

test('the @me filter uses each account\'s own address', () => {
  const t = app();
  t.S.acct = 'all';
  t.S.folder = 'INBOX';
  t.S.lists.set('a|INBOX', [msg('a', 'INBOX', 1, { recipients: ['me@x.com'] }), msg('a', 'INBOX', 2, { recipients: ['list@x.com'] })]);
  t.S.lists.set('b|INBOX', [msg('b', 'INBOX', 3, { recipients: ['B@Y.com'] })]);
  t.S.filter = 'mentions';
  assert.deepEqual(t.list().map((m) => m.uid), [3, 1]);
  t.S.filter = 'flagged';
  assert.deepEqual(t.list(), []);
});

test('the sidebar offers aggregated views for all accounts', () => {
  const t = app();
  t.S.acct = 'all';
  t.S.folder = 'INBOX';
  t.renderSidebar();
  const html = t.elements['#sidebar'].innerHTML;
  for (const view of ['__unread', '__starred', '__drafts', '__sent']) assert.match(html, new RegExp(`data-go="${view}"`));
  assert.match(html, /所有草稿/);
});

test('the filter bar switches filters', async () => {
  const t = app();
  t.S.acct = 'a';
  await t.listeners.click({ target: { closest: (sel) => sel === '[data-action]' ? { dataset: { action: 'filter', filter: 'attachment' }, disabled: false, closest: () => null } : null, classList: { contains: () => false } }, preventDefault() {} });
  assert.equal(t.S.filter, 'attachment');
});

test('sync events keep the unread and starred views current', () => {
  const t = app();
  t.S.acct = 'a';
  t.S.lists.set('a|__unread', [msg('a', 'INBOX', 1, { seen: false }), msg('a', 'INBOX', 2, { seen: false })]);
  t.S.lists.set('a|__starred', [msg('a', 'INBOX', 3, { flagged: true })]);
  t.onMailEvent({ type: 'flags', accountId: 'a', minUid: 1, items: [{ uid: 1, seen: true, flagged: false }, { uid: 3, seen: true, flagged: false }] });
  t.S.folder = '__unread';
  assert.deepEqual(t.list().map((m) => m.uid), [], '在别处读过的移出未读；没出现在 flags 里的（已删除）也移出');
  t.S.folder = '__starred';
  assert.deepEqual(t.list().map((m) => m.uid), [], '取消星标后移出');
  t.onMailEvent({ type: 'newMail', accountId: 'a', messages: [msg('a', 'INBOX', 7, { seen: false }), msg('a', 'INBOX', 8, { seen: true })] });
  t.S.folder = '__unread';
  assert.deepEqual(t.list().map((m) => m.uid), [7], '新到的未读邮件进入所有未读');
});

test('saving a draft reloads the aggregated drafts view', async () => {
  const t = app(async (cmd, input) => cmd === 'list_messages' ? [msg('a', 'Drafts', 6)] : []);
  t.S.folders.a = [{ path: 'Drafts', role: 'drafts' }];
  t.S.lists.set('a|__drafts', [msg('a', 'Drafts', 5)]);
  t.draftSaved({ accountId: 'a', folder: 'Drafts', uid: 6 });
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual([...t.S.lists.get('a|__drafts')].map((m) => m.uid), [6]);
});

test('missing folders never cache an empty drafts view, and loading them fills it in', async () => {
  let folders = null;
  const t = app(async (cmd) => {
    if (cmd === 'list_folders') { if (!folders) throw new Error('offline'); return folders; }
    if (cmd === 'list_messages') return [msg('a', 'Sent', 4)];
    return [];
  });
  t.S.acct = 'a';
  t.S.folder = '__sent';
  await t.loadList('a', '__sent');
  assert.equal(t.S.lists.has('a|__sent'), false, '文件夹列表读不到时不能当成「没有邮件」');
  folders = [{ path: 'Sent', role: 'sent' }];
  await t.loadFolders('a');
  await new Promise((r) => setTimeout(r, 0));
  assert.deepEqual([...(t.S.lists.get('a|__sent') || [])].map((m) => m.uid), [4]);
});

test('filters do not silently apply to search results', () => {
  const t = app();
  t.S.acct = 'a';
  t.S.folder = 'INBOX';
  t.S.lists.set('a|INBOX', [msg('a', 'INBOX', 1, { subject: 'report' }), msg('a', 'INBOX', 2, { subject: 'report', attachment: true })]);
  t.S.filter = 'attachment';
  t.S.query = 'report';
  assert.deepEqual(t.list().map((m) => m.uid), [2, 1]);
});

test('the unread view shows a count and POP3 accounts have no starred view', () => {
  const t = app();
  t.S.accounts = [{ id: 'p', email: 'p@x.com', name: 'P', receiveProtocol: 'pop3' }];
  t.S.live.p = { unread: 4 };
  t.S.acct = 'p';
  t.S.folder = 'INBOX';
  t.renderSidebar();
  const html = t.elements['#sidebar'].innerHTML;
  assert.match(html, /data-go="__unread"[\s\S]*?>4</);
  assert.doesNotMatch(html, /data-go="__starred"/);
});

test('an open but empty search box does not apply the hidden filter', () => {
  const t = app();
  t.S.acct = 'a';
  t.S.folder = 'INBOX';
  t.S.lists.set('a|INBOX', [msg('a', 'INBOX', 1), msg('a', 'INBOX', 2, { attachment: true })]);
  t.S.filter = 'attachment';
  t.S.searching = true;
  t.S.query = '';
  assert.deepEqual(t.list().map((m) => m.uid), [2, 1]);
});

test('loading folders does not start a second load of a view that is already loading', async () => {
  let release;
  const t = app(async (cmd) => {
    if (cmd === 'list_folders') return [{ path: 'Sent', role: 'sent' }];
    if (cmd === 'list_messages') return new Promise((r) => { release = () => r([msg('a', 'Sent', 1)]); });
    return [];
  });
  t.S.acct = 'a';
  t.S.folder = '__sent';
  const pending = t.loadList('a', '__sent');
  await new Promise((r) => setTimeout(r, 0));
  await t.loadFolders('a');
  release?.();
  await pending;
  assert.equal(t.calls.filter((c) => c.cmd === 'list_messages').length, 1);
});

test('a fully local list is rendered whole and no longer offers「加载更早」', async () => {
  const all = Array.from({ length: 250 }, (_, i) => msg('a', 'INBOX', 250 - i));
  const t = app(async (cmd) => cmd === 'list_messages' ? { items: all, complete: true } : []);
  await t.loadList('a', 'INBOX');
  assert.equal(t.list().length, 250);
  assert.ok(t.S.exhausted.has('a|INBOX'), '整个文件夹都在本地，不该再显示加载更早');
  assert.equal(t.calls.filter((c) => c.cmd === 'list_messages').length, 1);
});

test('a server-paged list keeps offering「加载更早」until the last page', async () => {
  const first = Array.from({ length: 100 }, (_, i) => msg('a', 'INBOX', 200 - i));
  const t = app(async (cmd) => cmd === 'list_messages' ? first : []);
  await t.loadList('a', 'INBOX');
  assert.equal(t.list().length, 100);
  assert.ok(!t.S.exhausted.has('a|INBOX'), '服务器分页还有更早的邮件');
});

test('entering a folder replaces a first page with the complete local list', async () => {
  const all = Array.from({ length: 150 }, (_, i) => msg('a', 'INBOX', 150 - i));
  const t = app(async (cmd) => cmd === 'list_messages' ? { items: all, complete: true } : []);
  t.S.accounts = [{ id: 'a', email: 'me@x.com', name: 'Me' }];
  t.S.acct = 'a';
  t.S.folders.a = [{ path: 'INBOX', role: 'inbox' }];
  t.S.lists.set('a|INBOX', all.slice(0, 100));
  t.go('INBOX', 'a');
  await new Promise((r) => setTimeout(r, 0));
  assert.equal(t.list().length, 150);
  assert.ok(t.S.exhausted.has('a|INBOX'));
});

test('a long list only renders the window around the scroll position', () => {
  const t = app();
  const first = t.listWindowRange(2713, 0, 776);
  assert.equal(first.start, 0);
  assert.ok(first.end >= 12 && first.end <= 60, `窗口不该一次渲染几千行：${JSON.stringify(first)}`);
  const middle = t.listWindowRange(2713, 106976, 776);
  assert.ok(middle.start <= 1614 && middle.end >= 1626, `中间位置要包含可见的行：${JSON.stringify(middle)}`);
  const bottom = t.listWindowRange(2713, 178000, 776);
  assert.equal(bottom.end, 2713, '滚到底时窗口贴着列表末尾');
  assert.ok(bottom.start > 2680, `底部窗口不该超出：${JSON.stringify(bottom)}`);
  // 小列表（不超过窗口）整表都在窗口里
  const small = t.listWindowRange(10, 0, 776);
  assert.deepEqual([small.start, small.end], [0, 10]);
});
