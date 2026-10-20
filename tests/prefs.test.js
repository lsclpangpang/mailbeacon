const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const P = require('../ui/prefs.js');

const plain = (v) => JSON.parse(JSON.stringify(v));
const base = () => ({ autostart: false, tray: true, notify: true, notifyContent: 'full', dnd: false, dndStart: '22:00', dndEnd: '08:00', mutedAccounts: ['b'], notifyFolders: { b: ['Work'] } });

test('only folders worth a separate alert are offered', () => {
  const folders = [
    { path: 'INBOX', role: 'inbox' }, { path: 'Sent', role: 'sent' }, { path: 'Drafts', role: 'drafts' },
    { path: 'Trash', role: 'trash' }, { path: 'Junk', role: 'junk' }, { path: '[Gmail]/All Mail', role: 'all' },
    { path: '__local_drafts', role: 'local-drafts' }, { path: 'Archive', role: 'archive' }, { path: 'Work', role: 'other' },
  ];
  assert.deepEqual(P.notifiableFolders(folders).map((f) => f.path), ['Archive', 'Work']);
  assert.deepEqual(P.notifiableFolders(undefined), []);
});

test('form controls merge into the saved preferences without touching unrendered accounts', () => {
  const next = P.fromControls(base(), {
    notify: true, notifyContent: 'sender', dnd: true, dndStart: '23:00', dndEnd: '07:30', tray: false, autostart: true,
    accounts: { a: { notify: false, folders: ['Work', 'Archive'] } },
  });
  assert.deepEqual(plain(next), {
    autostart: true, tray: false, notify: true, notifyContent: 'sender', dnd: true, dndStart: '23:00', dndEnd: '07:30',
    mutedAccounts: ['b', 'a'], notifyFolders: { b: ['Work'], a: ['Work', 'Archive'] },
  });
  const unmuted = P.fromControls(next, { accounts: { b: { notify: true, folders: [] } } });
  assert.deepEqual(plain(unmuted.mutedAccounts), ['a']);
  assert.equal(unmuted.notifyFolders.b, undefined, '清空文件夹后不留空列表');
  assert.equal(unmuted.notifyContent, 'sender', '没给的控件保持原值');
  const keepFolders = P.fromControls(next, { accounts: { a: { notify: true, folders: null } } });
  assert.deepEqual(plain(keepFolders.notifyFolders.a), ['Work', 'Archive'], '文件夹列表还没加载时保留原设置');
});

test('the summary describes quiet hours and mute state', () => {
  assert.equal(P.accountNotifyHint(base(), 'a'), '收件箱有新邮件时提醒');
  assert.equal(P.accountNotifyHint(base(), 'b'), '此账号不弹出通知');
  const p = { ...base(), notifyFolders: { a: ['Work'] } };
  assert.equal(P.accountNotifyHint(p, 'a'), '收件箱和另外 1 个文件夹有新邮件时提醒');
  assert.equal(P.accountNotifyHint({ ...p, notify: false }, 'a'), '新邮件通知已关闭');
});

function app(invoke, sections = {}) {
  const overlay = { innerHTML: '', querySelectorAll: (sel) => (sel === '[data-notify-account]' ? Object.values(sections) : []), querySelector: () => null };
  const listeners = {};
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: P, MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async (name, fn) => { listeners[name] = fn; } } } },
    document: { querySelector: (s) => (s === '#overlay' ? overlay : null), addEventListener: () => {} },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = () => {};
    globalThis.api = { S, openSettings, loadFolders, savePrefs: typeof savePrefs === 'function' ? savePrefs : null, forgetAccountPrefs: typeof forgetAccountPrefs === 'function' ? forgetAccountPrefs : null };
  `, context);
  context.api.S.accounts = [{ id: 'a', email: 'a@x.com', name: 'A' }, { id: 'b', email: 'b@x.com', name: 'B' }];
  context.api.S.folders = { a: [{ path: 'INBOX', role: 'inbox' }, { path: 'Work', name: '工作', role: 'other' }], b: [{ path: 'INBOX', role: 'inbox' }] };
  context.api.S.prefs = base();
  return { ...context.api, overlay, listeners };
}

test('settings show general and per-account notification controls from saved preferences', () => {
  const t = app(async () => {});
  t.openSettings();
  const html = t.overlay.innerHTML;
  assert.match(html, /<form class="prefs-form"/);
  assert.match(html, /name="tray" checked/);
  assert.doesNotMatch(html, /name="autostart" checked/);
  assert.match(html, /<option value="full" selected>/);
  assert.match(html, /name="dndStart" value="22:00" disabled/, '免打扰关闭时时间不可编辑');
  assert.match(html, /data-notify-account="a"[\s\S]*name="accountNotify" aria-describedby="notify-hint-a" checked[\s\S]*value="Work"/);
  assert.match(html, /data-notify-account="b"[\s\S]*name="accountNotify"(?! checked)/);
  assert.match(html, /此账号不弹出通知/);
});

test('saving keeps the confirmed preferences and rolls back the form on failure', async () => {
  let reply = (prefs) => ({ ...prefs, dndStart: '23:00' });
  const sent = [];
  const t = app(async (cmd, input) => { if (cmd === 'save_prefs') { sent.push(plain(input.prefs)); return reply(input.prefs); } });
  assert.equal(typeof t.savePrefs, 'function');
  await t.savePrefs({ dnd: true, dndStart: '23:00' });
  assert.equal(sent[0].dnd, true);
  assert.equal(t.S.prefs.dndStart, '23:00');
  reply = () => { throw '免打扰时间格式应为 HH:MM'; };
  await t.savePrefs({ dndStart: '99:00' });
  assert.equal(t.S.prefs.dndStart, '23:00', '失败时保留上次成功保存的设置');
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('changes made while an earlier save is in flight build on it instead of overwriting it', async () => {
  const replies = [];
  const sent = [];
  const t = app((cmd, input) => {
    if (cmd !== 'save_prefs') return Promise.resolve();
    sent.push(plain(input.prefs));
    const d = deferred();
    replies.push(() => d.resolve(input.prefs));
    return d.promise;
  });
  const first = t.savePrefs({ accounts: { a: { notify: true, folders: ['Work'] } } });
  const second = t.savePrefs({ tray: false });
  await new Promise((r) => setImmediate(r));
  replies.shift()();
  await new Promise((r) => setImmediate(r));
  replies.shift()();
  await Promise.all([first, second]);
  assert.deepEqual(plain(sent.at(-1).notifyFolders.a), ['Work'], '第二次保存带上第一次的改动');
  assert.equal(sent.at(-1).tray, false);
  assert.deepEqual(plain(t.S.prefs.notifyFolders.a), ['Work']);
  assert.equal(t.S.prefs.tray, false);
});

test('a failed save while another is pending rolls back only to what the backend confirmed', async () => {
  const t = app(async (cmd, input) => {
    if (cmd !== 'save_prefs') return;
    if (input.prefs.dndStart === '99:99') throw '免打扰时间格式应为 HH:MM';
    return input.prefs;
  });
  await t.savePrefs({ dndStart: '99:99' });
  assert.equal(t.S.prefs.dndStart, '22:00');
  await t.savePrefs({ tray: false });
  assert.equal(t.S.prefs.dndStart, '22:00', '失败的改动不会被下一次保存带上');
  assert.equal(t.S.prefs.tray, false);
});

test('saving before preferences were read loads them first so stored choices survive', async () => {
  const stored = { ...base(), mutedAccounts: ['b'], notifyFolders: { a: ['Work'] } };
  const sent = [];
  const t = app(async (cmd, input) => {
    if (cmd === 'get_prefs') return stored;
    if (cmd === 'save_prefs') { sent.push(plain(input.prefs)); return input.prefs; }
  });
  t.S.prefs = null;
  await t.savePrefs({ tray: false });
  assert.deepEqual(plain(sent[0].mutedAccounts), ['b']);
  assert.deepEqual(plain(sent[0].notifyFolders), { a: ['Work'] });
});

test('settings fetch missing folder lists and refresh just that account section', async () => {
  const section = { outerHTML: '', dataset: { notifyAccount: 'c' } };
  const t = app(async (cmd, input) => {
    if (cmd === 'list_folders' && input.accountId === 'c') return [{ path: 'INBOX', role: 'inbox' }, { path: 'Proj', name: '项目', role: 'other' }];
    return [];
  }, { c: section });
  t.S.accounts.push({ id: 'c', email: 'c@x.com', name: 'C' });
  t.openSettings();
  assert.match(t.overlay.innerHTML, /data-notify-account="c"[\s\S]*正在读取文件夹/);
  await new Promise((r) => setImmediate(r));
  assert.match(section.outerHTML, /data-notify-account="c"[\s\S]*value="Proj"/);
  assert.match(section.outerHTML, /<fieldset class="notify-folders"[\s\S]*<legend/, '文件夹勾选是一组');
});

test('a folder list that fails to load offers a retry instead of loading forever', async () => {
  const section = { outerHTML: '', dataset: { notifyAccount: 'c' } };
  const t = app(async (cmd, input) => {
    if (cmd === 'list_folders') throw '连接失败';
    return [];
  }, { c: section });
  t.S.accounts.push({ id: 'c', email: 'c@x.com', name: 'C' });
  t.openSettings();
  await new Promise((r) => setImmediate(r));
  assert.match(section.outerHTML, /读取文件夹失败/);
  assert.match(section.outerHTML, /data-action="retry-notify-folders"/);
});

test('removing an account drops it from the preferences kept in the page', () => {
  const t = app(async () => {});
  assert.equal(typeof t.forgetAccountPrefs, 'function');
  t.forgetAccountPrefs('b');
  assert.deepEqual(plain(t.S.prefs.mutedAccounts), []);
  assert.equal(t.S.prefs.notifyFolders.b, undefined);
});

test('a newer folder load started elsewhere still fills the settings section', async () => {
  const section = { outerHTML: '', dataset: { notifyAccount: 'c' } };
  const pending = [];
  const t = app((cmd) => {
    if (cmd !== 'list_folders') return Promise.resolve([]);
    const d = deferred();
    pending.push(d);
    return d.promise;
  }, { c: section });
  t.S.accounts.push({ id: 'c', email: 'c@x.com', name: 'C' });
  t.openSettings();
  // 账号上线时又读了一次文件夹（更新的那次才会写入 S.folders）
  const again = t.loadFolders('c');
  pending[0].resolve([{ path: 'INBOX', role: 'inbox' }]);
  await new Promise((r) => setImmediate(r));
  pending[1].resolve([{ path: 'INBOX', role: 'inbox' }, { path: 'Proj', name: '项目', role: 'other' }]);
  await again;
  await new Promise((r) => setImmediate(r));
  assert.match(section.outerHTML, /value="Proj"/);
});

test('sections redrawn while a save is in flight show the pending choice', async () => {
  const d = deferred();
  const t = app((cmd, input) => (cmd === 'save_prefs' ? d.promise.then(() => input.prefs) : Promise.resolve([])));
  const saving = t.savePrefs({ accounts: { a: { notify: true, folders: ['Work'] } } });
  await new Promise((r) => setImmediate(r));
  t.openSettings();
  assert.match(t.overlay.innerHTML, /value="Work" checked/, '还在保存的勾选不能被重画掉');
  d.resolve();
  await saving;
});
