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
  const calls = [], listeners = {}, reader = { dataset: {}, innerHTML: '', querySelector: () => null };
  const overlay = { innerHTML: '' };
  const recommended = 'recommended' in options ? options.recommended : [PREVIEW];
  const installed = 'installed' in options ? options.installed : [TEXTEDIT];
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, args) => {
      calls.push({ cmd, args });
      if (cmd === 'attachment_open_with') {
        await options.beforeChoices?.(args);
        return { path: '/tmp/附件/手册.pdf', recommended, installed };
      }
      if (cmd === 'open_attachment') {
        await options.beforeOpen?.(args);
        if (options.defaultFails) throw 'Launcher "/usr/bin/open" failed';
      }
      return '/tmp/附件/手册.pdf';
    } }, event: { listen: async () => {} } } },
    document: { querySelector: selector => selector === '#reader' ? reader : selector === '#overlay' ? overlay : null, addEventListener: (name, callback) => { listeners[name] = callback; } },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    toast = function() {};
    render = function() {};
    showMenu = function(anchor, entries) { globalThis.menu = { anchor, entries }; };
    globalThis.api = { S, renderReader, openWithApps, appMenuEntries, showAttachmentMenu, openAttachment, closeOverlay };
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
    isConnected: true,
    dataset: { att: '2', attachmentSource: captured ? decode(captured[1]) : '' },
    classList: { contains: () => false },
    closest(selector) { return selector === '[data-att]' ? this : null; },
  };
  return { S, listeners, calls, context, button, overlay, hasSource: Boolean(captured) };
}

const labels = menu => Array.from(menu.entries.filter(entry => typeof entry !== 'string').map(entry => entry.label));
const appChoices = menu => menu.entries.find(entry => entry?.label === '打开方式')?.children;
const plain = value => JSON.parse(JSON.stringify(value));

test('single click does nothing; right click keeps apps inside the open-with submenu', async () => {
  const { listeners, calls, context, button, hasSource } = mount();
  assert.ok(hasSource, 'the rendered attachment must bind its original source');

  await listeners.click({ ...EVENT, target: button, detail: 1 });
  assert.equal(calls.length, 0, '鼠标单击不下载也不打开');
  assert.equal(context.menu, undefined, '鼠标单击不弹菜单');

  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  assert.ok(context.menu, '右键要弹附件菜单');
  assert.deepEqual(labels(context.menu), ['打开', '打开方式', '保存到「下载」', '在 Finder 中显示']);
  assert.deepEqual(labels({ entries: appChoices(context.menu) }), ['Preview']);
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
  assert.deepEqual(labels(context.menu), ['Preview']);
});

test('unknown file types fall back to installed editors', async () => {
  const { listeners, context, button } = mount({
    recommended: [],
    installed: [{ name: 'WPS Office', path: '/Applications/wpsoffice.app' }, TEXTEDIT],
  });
  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  assert.deepEqual(labels(context.menu), ['打开', '打开方式', '保存到「下载」', '在 Finder 中显示']);
  assert.deepEqual(labels({ entries: appChoices(context.menu) }), ['TextEdit', 'WPS Office']);
});

test('choosing an app keeps the original attachment source after the selected mail changes', async () => {
  const { S, listeners, calls, context, button } = mount();
  listeners.contextmenu({ ...EVENT, target: button });
  await tick();
  await tick();
  S.sel = { accountId: 'other', folder: 'INBOX', uid: 999 };
  S.folder = 'INBOX';
  appChoices(context.menu)[0].run();
  await tick();
  assert.deepEqual(calls.map(call => call.cmd), ['attachment_open_with', 'open_attachment_with_app']);
  assert.deepEqual(JSON.parse(JSON.stringify(calls[1].args)), {
    accountId: 'a', folder: 'Projects|2026', uid: 17, uidValidity: 1234, index: 2, app: '/System/Applications/Preview.app',
  });
});


test('recommended apps are deduplicated and the default app is first without truncating choices', () => {
  const { context } = mount();
  const apps = Array.from({ length: 14 }, (_, i) => ({ name: `App ${i}`, path: `/Applications/App-${i}.app` }));
  const defaultApp = { name: 'Preview', path: PREVIEW.path, isDefault: true };
  const result = plain(context.api.openWithApps({ recommended: [apps[0], ...apps, defaultApp], installed: [TEXTEDIT] }));
  assert.equal(result.length, 15, 'all distinct recommended apps stay accessible in the scrollable submenu');
  assert.equal(result[0].path, PREVIEW.path);
  assert.equal(result[0].isDefault, true);
  assert.equal(result.filter(app => app.path === apps[0].path).length, 1);
  assert.ok(!result.some(app => app.path === TEXTEDIT.path), 'installed fallback should not dilute real recommendations');
});

test('localized editors lead unknown-type fallback and system default still takes precedence', () => {
  const { context } = mount();
  const editor = { ...TEXTEDIT, name: '文本编辑' };
  const installed = [
    { name: 'Another App', path: '/Applications/Another.app' },
    editor,
    { name: 'Special App', path: '/Applications/Special.app', isDefault: true },
  ];
  assert.deepEqual(Array.from(context.api.openWithApps({ recommended: [], installed }), app => app.name), [
    'Special App', '文本编辑', 'Another App',
  ]);
});

test('app entries use concise names, default badges and only safe PNG icons', () => {
  const { context } = mount();
  const png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jP1EAAAAASUVORK5CYII=';
  const entries = context.api.appMenuEntries({}, [
    { ...PREVIEW, name: '预览', isDefault: true, icon: png },
    { name: 'Remote icon', path: '/Applications/Remote.app', icon: 'https://example.test/tracker.png' },
    { name: 'SVG icon', path: '/Applications/SVG.app', icon: 'data:image/svg+xml,<svg onload="alert(1)"></svg>' },
    { ...TEXTEDIT, icon: null },
  ]);
  assert.equal(entries[0].label, '预览');
  assert.equal(entries[0].sub, '默认');
  assert.equal(entries[0].image, png);
  for (const entry of entries.slice(1)) {
    assert.ok(!entry.image, 'external URLs and non-PNG image sources are not used');
    assert.equal(entry.icon, 'app', 'apps without valid artwork use an app glyph, not a document glyph');
    assert.notEqual(entry.sub, '默认');
  }
});

test('an empty app list keeps primary attachment actions without an empty submenu', async () => {
  const { context, button } = mount({ recommended: [], installed: [] });
  await context.api.showAttachmentMenu(button);
  assert.deepEqual(labels(context.menu), ['打开', '保存到「下载」', '在 Finder 中显示']);
  assert.equal(appChoices(context.menu), undefined);
});

test('save and reveal retain the source captured by the attachment menu', async () => {
  const { S, context, button, calls } = mount();
  await context.api.showAttachmentMenu(button);
  S.sel = { accountId: 'another', folder: 'Other', uid: 777 };
  const entries = context.menu.entries;
  entries.find(entry => entry?.label === '保存到「下载」').run();
  entries.find(entry => entry?.label === '在 Finder 中显示').run();
  await tick();
  for (const cmd of ['save_attachment', 'reveal_attachment']) {
    const call = calls.find(call => call.cmd === cmd);
    assert.ok(call, `${cmd} must be invoked`);
    assert.deepEqual(plain(call.args), { accountId: 'a', folder: 'Projects|2026', uid: 17, uidValidity: 1234, index: 2 });
  }
});

test('a pending app query cannot reopen a menu for a detached attachment', async () => {
  let resolveChoices;
  const pendingChoices = new Promise(resolve => { resolveChoices = resolve; });
  const { context, button } = mount({ beforeChoices: () => pendingChoices });
  const pending = context.api.showAttachmentMenu(button);
  button.isConnected = false;
  resolveChoices();
  await pending;
  assert.equal(context.menu, undefined);
});


function deferred() {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
}

test('a slower earlier attachment query cannot replace the latest requested menu', async () => {
  const earlier = deferred(), latest = deferred();
  const { context, button } = mount({ beforeChoices: args => args.index === 2 ? earlier.promise : latest.promise });
  const other = {
    ...button,
    dataset: { ...button.dataset, attachmentSource: JSON.stringify({ ...JSON.parse(button.dataset.attachmentSource), index: 3 }) },
  };
  const pendingEarlier = context.api.showAttachmentMenu(button);
  const pendingLatest = context.api.showAttachmentMenu(other);
  latest.resolve();
  await pendingLatest;
  assert.equal(context.menu.anchor, other, 'the latest request must be shown as soon as it resolves');
  earlier.resolve();
  await pendingEarlier;
  assert.equal(context.menu.anchor, other, 'an old response must not overwrite the current attachment menu');
});

test('closing the overlay cancels a pending attachment menu request', async () => {
  const choices = deferred();
  const { context, button } = mount({ beforeChoices: () => choices.promise });
  const pending = context.api.showAttachmentMenu(button);
  context.api.closeOverlay();
  choices.resolve();
  await pending;
  assert.equal(context.menu, undefined, 'closing should remain effective after the app query completes');
});

test('Escape cancels a pending attachment menu while dismissing an existing overlay', async () => {
  const choices = deferred();
  const { context, button, listeners, overlay } = mount({ beforeChoices: () => choices.promise });
  overlay.innerHTML = '<div class="menu">Existing menu</div>';
  const pending = context.api.showAttachmentMenu(button);
  listeners.keydown({ ...EVENT, key: 'Escape', target: { id: '', closest: () => null } });
  assert.equal(overlay.innerHTML, '', 'Escape must dismiss the visible overlay');
  choices.resolve();
  await pending;
  assert.equal(context.menu, undefined, 'the late response must not undo Escape');
});

test('default-open failure cannot display fallback choices for a detached attachment', async () => {
  const choices = deferred();
  const { context, button } = mount({ defaultFails: true, beforeChoices: () => choices.promise });
  const pending = context.api.openAttachment(JSON.parse(button.dataset.attachmentSource), button);
  await tick();
  button.isConnected = false;
  choices.resolve();
  await pending;
  assert.equal(context.menu, undefined);
});


for (const stage of ['default-open', 'fallback-choices']) {
  test(`a pending ${stage} result cannot replace a newer compose or settings sheet`, async () => {
    const response = deferred();
    const { context, button, overlay } = mount({
      defaultFails: true,
      beforeOpen: stage === 'default-open' ? () => response.promise : undefined,
      beforeChoices: stage === 'fallback-choices' ? () => response.promise : undefined,
    });
    const pending = context.api.openAttachment(JSON.parse(button.dataset.attachmentSource), button);
    await tick();
    const newerSheet = { className: 'sheet compose-sheet' };
    overlay.firstChild = newerSheet;
    overlay.innerHTML = '<div class="sheet compose-sheet">Unsaved draft</div>';
    response.resolve();
    await pending;
    assert.equal(context.menu, undefined, 'a new sheet must not be overwritten by late open-with choices');
    assert.equal(overlay.firstChild, newerSheet);
  });
}
