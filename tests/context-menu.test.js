const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function setup() {
  const listeners = {}, menus = [], calls = [];
  const overlay = { innerHTML: '' };
  const context = vm.createContext({
    window: { __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return '/Downloads/test.eml'; } }, event: { listen: async () => {} } } },
    document: {
      addEventListener: (name, handler) => { listeners[name] = handler; },
      querySelector: selector => selector === '#overlay' ? overlay : null,
    },
    console, setTimeout, clearTimeout,
    captureMenu: (anchor, entries, point) => { menus.push({ anchor, entries, point }); },
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = function () {};
    loadList = async function () {};
    showMenu = captureMenu;
    globalThis.state = S;
  `, context);
  const S = context.state;
  S.accounts = [{ id: 'a' }, { id: 'b' }];
  S.acct = 'all';
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 1 };
  S.lists.set('a|INBOX', [{ ...S.sel, seen: true }]);
  S.lists.set('b|Projects|2026', [{ accountId: 'b', folder: 'Projects|2026', uid: 1, seen: false }]);
  S.folders.b = [{ path: 'Projects|2026', name: '当前文件夹', role: 'other' }, { path: 'Work', name: '工作', role: 'other' }];
  S.folders.a = [{ path: 'Private', name: '另一账号文件夹', role: 'other' }];
  const row = { dataset: { open: 'b|1|Projects|2026' }, getBoundingClientRect: () => ({ left: 10, top: 20, bottom: 60 }) };
  row.closest = selector => selector.includes('[data-open]') ? row : null;
  function rightClick(target = row) {
    let prevented = false;
    assert.equal(typeof listeners.contextmenu, 'function', 'mail rows need an app context menu');
    listeners.contextmenu({ target, clientX: 300, clientY: 400, preventDefault() { prevented = true; } });
    return prevented;
  }
  return { S, row, menus, calls, listeners, rightClick };
}

test('right clicking an unread row exposes download and move without opening or marking it read', () => {
  const { S, rightClick, menus, calls } = setup();
  assert.equal(rightClick(), true);
  assert.ok(menus[0].entries.some(e => e.label === '下载邮件（.eml）'));
  assert.ok(menus[0].entries.some(e => e.label === '移动到…'));
  assert.equal(S.sel.accountId, 'a');
  assert.equal(S.lists.get('b|Projects|2026')[0].seen, false);
  assert.equal(calls.length, 0);
});

test('context download targets the clicked account, UID and complete folder after selection changes', async () => {
  const { S, rightClick, menus, calls } = setup();
  rightClick();
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 99 };
  await menus[0].entries.find(e => e.label === '下载邮件（.eml）').run();
  assert.equal(calls[0].cmd, 'save_message');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].input)), { accountId: 'b', folder: 'Projects|2026', uid: 1 });
});

test('context move lists only the clicked account folders and keeps that source through the submenu', async () => {
  const { S, rightClick, menus, calls } = setup();
  rightClick();
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 99 };
  menus[0].entries.find(e => e.label === '移动到…').run();
  const entries = menus[1].entries;
  assert.ok(entries.some(e => e.label === '新建文件夹并移动…'));
  assert.equal(entries.some(e => /当前文件夹|另一账号/.test(e.label)), false);
  await entries.find(e => e.label === '工作').run();
  assert.equal(calls[0].cmd, 'move_messages');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].input)), { accountId: 'b', folder: 'Projects|2026', uids: [1], role: 'other', target: 'Work' });
  assert.equal(S.sel.accountId, 'a');
});

test('busy actions cannot be repeated through the context menu', () => {
  const { S, rightClick, menus } = setup();
  S.downloading.add('b|Projects|2026|1');
  S.moving.add('b|Projects|2026|1');
  rightClick();
  assert.equal(menus[0].entries.find(e => e.icon === 'download').disabled, true);
  assert.equal(menus[0].entries.find(e => e.label === '移动到…').disabled, true);
});

test('native context menu remains available outside message rows', () => {
  const { rightClick, menus } = setup();
  assert.equal(rightClick({ closest: () => null }), false);
  assert.equal(menus.length, 0);
});

test('Shift+F10 opens the same actions for a keyboard-focused row', () => {
  const { listeners, row, menus } = setup();
  let prevented = false;
  listeners.keydown({ target: row, key: 'F10', shiftKey: true, preventDefault() { prevented = true; } });
  assert.equal(prevented, true);
  assert.ok(menus[0].entries.some(e => e.label === '移动到…'));
});
