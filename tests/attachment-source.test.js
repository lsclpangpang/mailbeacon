const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const decode = value => value.replace(/&(amp|quot|lt|gt|#39);/g, (_, entity) => ({ amp: '&', quot: '"', lt: '<', gt: '>', '#39': "'" })[entity]);
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const EVENT = { preventDefault() {}, clientX: 10, clientY: 10 };
const PREVIEW = { name: 'Preview', path: '/System/Applications/Preview.app' };
const TEXTEDIT = { name: 'TextEdit', path: '/System/Applications/TextEdit.app' };

function mount(options = {}) {
  const calls = [], listeners = {}, reader = { dataset: {}, innerHTML: '' };
  const recommended = 'recommended' in options ? options.recommended : [PREVIEW];
  const installed = 'installed' in options ? options.installed : [TEXTEDIT];
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === 'attachment_open_with') return { path: '/tmp/附件/手册.pdf', recommended, installed };
      if (cmd === 'open_attachment' && options.defaultFails) throw 'Launcher "/usr/bin/open" failed';
      return '/tmp/附件/手册.pdf';
    } }, event: { listen: async () => {} } } },
    document: { querySelector: selector => selector === '#reader' ? reader : null, addEventListener: (name, callback) => { listeners[name] = callback; } },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    toast = function() {};
    showMenu = function(anchor, entries) { globalThis.menu = { anchor, entries }; };
    globalThis.api = { S, renderReader };
  `, context);
  const { S, renderReader } = context.api;
  S.accounts = [{ id: 'a', name: 'A', email: 'a@example.test' }];
  S.acct = 'a';
  S.sel = { accountId: 'a', folder: 'Projects|2026', uid: 17 };
  S.folder = 'Projects|2026';
  S.lists.set('a|Projects|2026', [{ ...S.sel, subject: 'Attachment', date: 1, fromAddr: 'sender@example.test' }]);
  S.msg = { subject: 'Attachment', uidValidity: 1234, from: { name: 'Sender', addr: 'sender@example.test' }, to: [], cc: [], date: 1, text: 'Mail', attachments: [{ index: 2, name: '手册.pdf', size: 1024 }] };
  renderReader();
  const captured = reader.innerHTML.match(/data-attachment-source="([^"]+)"/);
  const button = {
    dataset: { att: '2', attachmentSource: captured ? decode(captured[1]) : '' },
    classList: { contains: () => false },
    closest(selector) { return selector === '[data-att]' ? this : null; },
  };
  return { S, listeners, calls, context, button, hasSource: Boolean(captured) };
}

const labels = menu => Array.from(menu.entries.filter(entry => typeof entry !== 'string').map(entry => entry.label));

test('single click does nothing; right click lists the recommended apps', async () => {
  const { listeners, calls, context, button, hasSource } = mount();
  assert.ok(hasSource, 'the rendered attachment must bind its original source');

  await listeners.click({ ...EVENT, target: button, detail: 1 });
  assert.equal(calls.length, 0, '鼠标单击不下载也不打开');
  assert.equal(context.menu, undefined, '鼠标单击不弹菜单');

  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  assert.ok(context.menu, '右键要弹附件菜单');
  assert.deepEqual(labels(context.menu), ['打开', '用 Preview 打开', '保存到「下载」', '在 Finder 中显示']);
});

test('double-clicking opens with the default app directly', async () => {
  const { listeners, calls, button } = mount();
  listeners.dblclick({ ...EVENT, target: button });
  await tick();
  assert.deepEqual(calls.map(call => call.cmd), ['open_attachment']);
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args)), { accountId: 'a', folder: 'Projects|2026', uid: 17, uidValidity: 1234, index: 2 });
});

test('no default app falls back to the open-with list instead of an error', async () => {
  const { listeners, calls, context, button } = mount({ defaultFails: true });
  listeners.dblclick({ ...EVENT, target: button });
  await tick();
  await tick();
  assert.deepEqual(calls.map(call => call.cmd), ['open_attachment', 'attachment_open_with']);
  assert.ok(context.menu, '没有默认程序时要弹打开方式列表');
  assert.deepEqual(labels(context.menu), ['用 Preview 打开']);
});

test('unknown file types fall back to installed editors', async () => {
  const { listeners, context, button } = mount({
    recommended: [],
    installed: [{ name: 'WPS Office', path: '/Applications/wpsoffice.app' }, TEXTEDIT],
  });
  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  assert.deepEqual(labels(context.menu), ['打开', '用 TextEdit 打开', '用 WPS Office 打开', '保存到「下载」', '在 Finder 中显示']);
});

test('choosing an app opens the attachment with that app', async () => {
  const { listeners, calls, context, button } = mount();
  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  context.menu.entries[2].run();
  await tick();
  assert.deepEqual(calls.map(call => call.cmd), ['attachment_open_with', 'open_attachment_with_app']);
  assert.deepEqual(JSON.parse(JSON.stringify(calls[1].args)), {
    accountId: 'a', folder: 'Projects|2026', uid: 17, uidValidity: 1234, index: 2, app: '/System/Applications/Preview.app',
  });
});
