const test = require('node:test');
const assert = require('node:assert/strict');
const T = require('../ui/threads.js');

const plain = (v) => JSON.parse(JSON.stringify(v));
let uid = 100;
const mail = (extra) => ({ accountId: 'a', folder: 'INBOX', uid: uid--, fromName: '', fromAddr: 'x@y.com', subject: '', date: 0, seen: true, flagged: false, attachment: false, messageId: '', inReplyTo: '', references: [], ...extra });

test('reply and forward prefixes are ignored when comparing subjects', () => {
  for (const s of ['Re: 报价', 'RE：报价', '回复: 报价', 'Fwd: Re: 报价', '答复：转发：报价', 'Aw: 报价', '  报价 ']) assert.equal(T.normalizeSubject(s), '报价', s);
  assert.equal(T.isReply('Re: x'), true);
  assert.equal(T.isReply('报价'), false);
});

test('messages are grouped by their ids even when the first mail is not loaded', () => {
  const items = [
    mail({ messageId: 'd@x', inReplyTo: 'c@x', references: ['a@x', 'c@x'], subject: 'Re: 报价', date: 4, seen: false, fromName: '王总' }),
    mail({ messageId: 'other@x', subject: '无关', date: 3 }),
    mail({ messageId: 'c@x', inReplyTo: 'b@x', references: ['a@x', 'b@x'], subject: 'Re: 报价', date: 2, fromName: '李四', attachment: true }),
    // a@x、b@x（在已发送里）没加载，c 和 d 仍然通过根邮件 a@x 归到一起
  ];
  const groups = T.groupThreads(items, () => false);
  assert.equal(groups.length, 2);
  assert.deepEqual(groups[0].items.map((m) => m.messageId), ['d@x', 'c@x'], '新的在前');
  assert.equal(groups[0].latest.messageId, 'd@x');
  assert.equal(groups[0].count, 2);
  assert.equal(groups[0].unread, 1);
  assert.equal(groups[0].attachment, true);
  assert.deepEqual(plain(groups[0].participants), ['李四', '王总'], '按时间先后');
  assert.equal(groups[1].latest.messageId, 'other@x');
});

test('mail without threading headers only joins by subject when it is a reply', () => {
  const items = [
    mail({ subject: 'Re: 周报', date: 5 }),
    mail({ subject: '周报', date: 4 }),
    mail({ subject: '周报', date: 1 }),
  ];
  const groups = T.groupThreads(items, () => false);
  assert.deepEqual(groups.map((g) => g.count), [2, 1], '每周同名的周报不合并，回复只挂到最近那封');
  assert.equal(groups[0].items[1].date, 4);
});

test('different accounts never share a conversation', () => {
  const items = [mail({ messageId: 'b@x', references: ['a@x'], date: 2 }), mail({ accountId: 'b', messageId: 'c@x', references: ['a@x'], date: 1 })];
  assert.equal(T.groupThreads(items, () => false).length, 2);
});

test('my own address shows as 我 among participants', () => {
  const items = [mail({ messageId: 'b@x', references: ['a@x'], fromAddr: 'me@x.com', fromName: 'Me', date: 2 }), mail({ messageId: 'a@x', fromName: '王总', date: 1 })];
  const g = T.groupThreads(items, (m) => m.fromAddr === 'me@x.com')[0];
  assert.deepEqual(plain(g.participants), ['王总', '我']);
});

test('a conversation from the server merges with what the list already has', () => {
  const listed = [mail({ messageId: 'c@x', folder: 'INBOX', uid: 7, date: 3 })];
  const server = [mail({ messageId: 'a@x', date: 1 }), mail({ messageId: 'b@x', folder: 'Sent', date: 2 }), mail({ messageId: 'c@x', folder: 'INBOX', uid: 7, date: 3, seen: false })];
  const merged = T.mergeConversation(server, listed);
  assert.deepEqual(merged.map((m) => m.messageId), ['a@x', 'b@x', 'c@x'], '从早到晚');
  assert.equal(merged[2], listed[0], '同一封用列表里的对象，已读等状态保持一致');
});

const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke) {
  const el = (extra = {}) => ({ innerHTML: '', dataset: {}, querySelector: () => ({ scrollTop: 0, scrollIntoView() {} }), querySelectorAll: () => [], ...extra });
  const elements = { '#sidebar': el(), '#list': el(), '#reader': el(), '#overlay': el() };
  const store = new Map();
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), MailBeaconThreads: T, MailBeaconExtras: require('../ui/compose-extras.js'),
      __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: { activeElement: null, querySelector: (s) => elements[s] || null, querySelectorAll: () => [], addEventListener: () => {} },
    localStorage: { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, String(v)) },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    renderSidebar = updateBadge = loadPreviews = function () {};
    toast = () => {};
    globalThis.api = { S, renderList, renderReader, openMessage, step, dropMsg, listEntries, toggleThreads: typeof toggleThreads === 'function' ? toggleThreads : null, loadThreadPrefs: typeof loadThreadPrefs === 'function' ? loadThreadPrefs : null };
  `, context);
  const api = context.api;
  api.S.accounts = [{ id: 'a', email: 'me@x.com', name: 'Me' }];
  api.S.acct = 'a';
  api.S.folders.a = [{ path: 'INBOX', name: '收件箱', role: 'inbox' }, { path: 'Sent', name: '已发送', role: 'sent' }];
  return { ...api, elements, store };
}

const listed = () => [
  mail({ uid: 9, messageId: 'c@x', inReplyTo: 'b@x', references: ['a@x', 'b@x'], subject: 'Re: 报价', fromName: '王总', date: 3, seen: false }),
  mail({ uid: 8, messageId: 'z@x', subject: '无关', fromName: '路人', date: 2 }),
  mail({ uid: 7, messageId: 'a@x', subject: '报价', fromName: '王总', date: 1 }),
];

test('conversation mode shows one row per conversation with its size', () => {
  const t = app(async () => []);
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  t.renderList();
  const html = t.elements['#list'].innerHTML;
  assert.equal((html.match(/class="row /g) || []).length, 2);
  assert.match(html, /data-open="a\|9\|INBOX"[\s\S]*thread-count[^>]*>2</);
  assert.equal(t.store.get('mb.threads'), '1', '记住选择');
  t.toggleThreads();
  t.renderList();
  assert.equal((t.elements['#list'].innerHTML.match(/class="row /g) || []).length, 3);
});

const body = { subject: '', from: { name: '', addr: '' }, to: [], cc: [], date: 0, html: null, text: '', attachments: [] };

test('J/K move between conversations rather than individual mails', async () => {
  const t = app(async (cmd) => (cmd === 'get_message' ? body : []));
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  t.S.sel = { accountId: 'a', folder: 'INBOX', uid: 7 };
  t.step(1);
  assert.equal(t.S.sel.uid, 8, '从会话里较早的一封往下也是下一个会话');
});

test('the reader lists the whole conversation including my reply found in Sent', async () => {
  const sent = mail({ uid: 3, folder: 'Sent', messageId: 'b@x', inReplyTo: 'a@x', references: ['a@x'], subject: 'Re: 报价', fromName: 'Me', fromAddr: 'me@x.com', date: 2 });
  const t = app(async (cmd, input) => {
    if (cmd === 'get_message') return { subject: 'Re: 报价', from: { name: '王总', addr: 'x@y.com' }, to: [], cc: [], date: 3, html: null, text: 'hi', attachments: [], uidValidity: 1 };
    if (cmd === 'thread_messages') { assert.equal(input.uid, 9); return [listed()[2], sent, listed()[0]]; }
    return [];
  });
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  await new Promise((r) => setImmediate(r));
  t.renderReader();
  const html = t.elements['#reader'].innerHTML;
  assert.match(html, /此会话共 3 封/);
  assert.match(html, /data-thread-open="a\|3\|Sent"[\s\S]*我[\s\S]*已发送/);
  assert.match(html, /data-thread-open="a\|9\|INBOX"[^>]*aria-current="true"/);
  // 打开会话里已发送的那封：能找到它，不会显示「没有选中邮件」
  await t.openMessage({ accountId: 'a', folder: 'Sent', uid: 3 });
  t.renderReader();
  assert.doesNotMatch(t.elements['#reader'].innerHTML, /没有选中邮件/);
  assert.match(t.elements['#reader'].innerHTML, /data-thread-open="a\|3\|Sent"[^>]*aria-current="true"/);
});

test('without conversation mode the reader does not ask for the conversation', async () => {
  const calls = [];
  const t = app(async (cmd) => { calls.push(cmd); return cmd === 'get_message' ? { subject: '', from: { name: '', addr: '' }, to: [], cc: [], date: 0, html: null, text: '', attachments: [] } : []; });
  t.S.lists.set('a|INBOX', listed());
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  assert.ok(!calls.includes('thread_messages'));
});

const reply = () => mail({ uid: 3, folder: 'Sent', messageId: 'b@x', inReplyTo: 'a@x', references: ['a@x'], subject: 'Re: 报价', fromName: 'Me', fromAddr: 'me@x.com', date: 2 });

test('the conversation is looked up after the mail body, with the headers the list already has', async () => {
  const calls = [];
  const t = app(async (cmd, input) => {
    calls.push([cmd, input]);
    if (cmd === 'get_message') return body;
    if (cmd === 'thread_messages') return [listed()[2], reply(), listed()[0]];
    return [];
  });
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  await new Promise((r) => setImmediate(r));
  const order = calls.map(([c]) => c).filter((c) => c === 'get_message' || c === 'thread_messages');
  assert.deepEqual(order, ['get_message', 'thread_messages'], '正文先出来');
  const hint = plain(calls.find(([c]) => c === 'thread_messages')[1].hint);
  assert.deepEqual(hint, { messageId: 'c@x', inReplyTo: 'b@x', references: ['a@x', 'b@x'] });
  // 在同一会话里切换不再重复搜索
  await t.openMessage({ accountId: 'a', folder: 'Sent', uid: 3 });
  await new Promise((r) => setImmediate(r));
  assert.equal(calls.filter(([c]) => c === 'thread_messages').length, 1);
});

test('stepping quickly only looks up the conversation the user stopped on', async () => {
  const calls = [];
  const t = app(async (cmd, input) => { calls.push([cmd, input]); return cmd === 'get_message' ? body : []; });
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  const first = t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  const second = t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 8 });
  await Promise.all([first, second]);
  await new Promise((r) => setImmediate(r));
  const looked = calls.filter(([c]) => c === 'thread_messages').map(([, i]) => i.uid);
  assert.deepEqual(looked, [8]);
});

test('a deleted conversation mail leaves the strip and the conversation is looked up again', async () => {
  let server = [listed()[2], reply(), listed()[0]];
  const calls = [];
  const t = app(async (cmd) => { calls.push(cmd); return cmd === 'get_message' ? body : cmd === 'thread_messages' ? server : []; });
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  await new Promise((r) => setImmediate(r));
  server = [listed()[2], listed()[0]];
  t.dropMsg('a', 'Sent', 3);
  assert.ok(!t.S.thread.items.some((m) => m.folder === 'Sent'), '删掉的不再显示');
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 7 });
  await new Promise((r) => setImmediate(r));
  assert.equal(calls.filter((c) => c === 'thread_messages').length, 2, '会话变了要重新查');
});

test('the conversation row stays selected after the list is reloaded', async () => {
  const t = app(async (cmd) => (cmd === 'get_message' ? body : cmd === 'thread_messages' ? [listed()[2], reply(), listed()[0]] : []));
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  await new Promise((r) => setImmediate(r));
  await t.openMessage({ accountId: 'a', folder: 'Sent', uid: 3 });
  t.S.lists.set('a|INBOX', listed()); // 刷新后是新的对象
  assert.equal(t.listEntries().index, 0);
  t.renderList();
  assert.match(t.elements['#list'].innerHTML, /class="row[^"]*\bsel\b[^"]*"[^>]*data-open="a\|9\|INBOX"/);
});

test('opening a conversation marks all of its listed unread mail read', async () => {
  const flags = [];
  const t = app(async (cmd, input) => { if (cmd === 'set_flag') flags.push(plain(input)); return cmd === 'get_message' ? body : []; });
  const items = listed();
  items[2].seen = false;
  t.S.lists.set('a|INBOX', items);
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  assert.ok(flags.some((f) => f.uids.includes(7) && f.value === true && f.flag === 'seen'), JSON.stringify(flags));
  t.renderList();
  assert.doesNotMatch(t.elements['#list'].innerHTML, /class="row unread[^"]*"[^>]*data-open="a\|9\|INBOX"/);
});

test('opening my reply from a stale conversation keeps it readable even if the lookup fails', async () => {
  let fail = false;
  const t = app(async (cmd) => {
    if (cmd === 'get_message') return body;
    if (cmd === 'thread_messages') { if (fail) throw '连接断开'; return [listed()[2], reply(), listed()[0]]; }
    return [];
  });
  t.S.lists.set('a|INBOX', listed());
  t.toggleThreads();
  await t.openMessage({ accountId: 'a', folder: 'INBOX', uid: 9 });
  await new Promise((r) => setImmediate(r));
  t.S.thread.stale = true; // 例如刚收到新邮件
  fail = true;
  const opening = t.openMessage({ accountId: 'a', folder: 'Sent', uid: 3 });
  t.renderReader();
  assert.doesNotMatch(t.elements['#reader'].innerHTML, /没有选中邮件/, '打开的那一刻就能看到');
  await opening;
  await new Promise((r) => setImmediate(r));
  t.renderReader();
  assert.doesNotMatch(t.elements['#reader'].innerHTML, /没有选中邮件/, '查不到会话也不能丢');
  assert.match(t.elements['#reader'].innerHTML, /data-thread-open="a\|3\|Sent"/);
});
