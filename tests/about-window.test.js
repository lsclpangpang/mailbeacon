const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { createAboutController } = require('../ui/about.js');

function view() {
  const nodes = Object.fromEntries(['about-version', 'about-error'].map(id => [id, { textContent: '', hidden: true }]));
  return { nodes, document: { getElementById: id => nodes[id] } };
}

test('about renders the runtime version supplied by the app', async () => {
  const v = view();
  const calls = [];
  const controller = createAboutController({ document: v.document, invoke: async (command) => {
    calls.push(command);
    return { version: '0.15.7' };
  } });
  await controller.load();
  assert.deepEqual(calls, ['about_info']);
  assert.equal(v.nodes['about-version'].textContent, '版本 0.15.7');
});

test('about links use fixed backend destinations without navigating the window', async () => {
  const v = view();
  const calls = [];
  const controller = createAboutController({ document: v.document, invoke: async (...args) => calls.push(args) });
  await controller.openLink('project');
  await controller.openLink('issues');
  assert.deepEqual(calls, [['open_project_link', { kind: 'project' }], ['open_project_link', { kind: 'issues' }]]);
  await controller.openLink('https://example.com');
  assert.equal(calls.length, 2, 'arbitrary URLs must never be passed to the backend');
});

test('opening a project link reports failures and a retry clears the message', async () => {
  const v = view();
  let fail = true;
  const controller = createAboutController({ document: v.document, invoke: async () => {
    if (fail) throw new Error('browser unavailable');
  } });
  await controller.openLink('project');
  assert.equal(v.nodes['about-error'].hidden, false);
  assert.match(v.nodes['about-error'].textContent, /无法打开链接.*browser unavailable/);
  fail = false;
  await controller.openLink('project');
  assert.equal(v.nodes['about-error'].hidden, true);
});

test('version lookup failure remains visible rather than showing a guessed version', async () => {
  const v = view();
  const controller = createAboutController({ document: v.document, invoke: async () => { throw 'offline'; } });
  await controller.load();
  assert.equal(v.nodes['about-version'].textContent, '版本暂不可用');
  assert.equal(v.nodes['about-error'].hidden, false);
});

test('native help menu failures are visible in the about window and the listener can be removed', async () => {
  const v = view();
  let onError;
  let stopped = false;
  const controller = createAboutController({
    document: v.document,
    invoke: async () => {},
    listen: async (event, callback) => {
      assert.equal(event, 'native-menu-error');
      onError = callback;
      return () => { stopped = true; };
    },
  });
  const stop = await controller.connectMenuErrors();
  onError({ payload: '菜单操作失败：无法启动浏览器' });
  assert.equal(v.nodes['about-error'].hidden, false);
  assert.equal(v.nodes['about-error'].textContent, '菜单操作失败：无法启动浏览器');
  stop();
  assert.equal(stopped, true);
});

test('failure to subscribe to native menu errors is reported visibly', async () => {
  const v = view();
  const controller = createAboutController({
    document: v.document,
    invoke: async () => {},
    listen: async () => { throw new Error('permission denied'); },
  });
  const stop = await controller.connectMenuErrors();
  assert.equal(v.nodes['about-error'].hidden, false);
  assert.match(v.nodes['about-error'].textContent, /无法接收菜单提示.*permission denied/);
  assert.doesNotThrow(stop);
});

test('about is a standalone local page with restricted script and navigation sources', () => {
  const html = fs.readFileSync(path.join(__dirname, '../ui/about.html'), 'utf8');
  assert.match(html, /Content-Security-Policy/);
  assert.match(html, /script-src 'self'/);
  assert.match(html, /about\.js/);
  assert.doesNotMatch(html, /src=["'](?:app\.js|https?:)/);
  assert.match(html, /id="about-error"[^>]*role="alert"/);
  assert.match(html, /src="icon\.png"/);
});

test('about capability grants only close and menu feedback listening and is isolated from main', () => {
  const capability = JSON.parse(fs.readFileSync(path.join(__dirname, '../src-tauri/capabilities/about.json'), 'utf8'));
  assert.deepEqual(capability.windows, ['about']);
  assert.deepEqual(capability.permissions, ['core:window:allow-close', 'core:event:allow-listen', 'core:event:allow-unlisten']);
  assert.equal(capability.remote, undefined);
});
