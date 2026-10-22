const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const modulePath = path.join(__dirname, '../ui/platform.js');
const api = fs.existsSync(modulePath) ? require(modulePath) : {};

test('desktop platform detection supports WebView2, WKWebView and reduced user-agent data', () => {
  assert.equal(typeof api.detectPlatform, 'function');
  assert.equal(api.detectPlatform({ userAgentData: { platform: 'Windows' }, platform: 'Win32' }), 'windows');
  assert.equal(api.detectPlatform({ platform: 'Win32', userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' }), 'windows');
  assert.equal(api.detectPlatform({ platform: 'MacIntel', userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' }), 'macos');
  assert.equal(api.detectPlatform({ platform: 'Linux x86_64' }), 'linux');
  assert.equal(api.detectPlatform({}), 'unknown');
});

test('platform labels use Ctrl on Windows and retain native Mac symbols', () => {
  assert.equal(typeof api.forPlatform, 'function');
  const windows = api.forPlatform('windows'), mac = api.forPlatform('macos');
  assert.equal(windows.shortcut('N', { shift: true }), 'Ctrl+Shift+N');
  assert.equal(windows.shortcut('Enter'), 'Ctrl+Enter');
  assert.equal(mac.shortcut('N', { shift: true }), '⇧⌘N');
  assert.equal(mac.shortcut('Enter'), '⌘Enter');
  assert.equal(windows.revealLabel, '在文件资源管理器中显示');
  assert.equal(mac.revealLabel, '在 Finder 中显示');
});

test('send modifier is platform specific and rejects IME, repeat and extra modifiers', () => {
  assert.equal(typeof api.forPlatform, 'function');
  const windows = api.forPlatform('windows'), mac = api.forPlatform('macos');
  assert.equal(windows.isSendShortcut({ key: 'Enter', ctrlKey: true }), true);
  assert.equal(mac.isSendShortcut({ key: 'Enter', metaKey: true }), true);
  assert.equal(windows.isSendShortcut({ key: 'Enter', metaKey: true }), false);
  assert.equal(mac.isSendShortcut({ key: 'Enter', ctrlKey: true }), false);
  for (const blocked of [{ isComposing: true }, { keyCode: 229 }, { altKey: true }, { shiftKey: true }, { metaKey: true }, { repeat: true }]) {
    assert.equal(windows.isSendShortcut({ key: 'Enter', ctrlKey: true, ...blocked }), false);
  }
});

test('browser module sets document platform before app layout is painted', () => {
  assert.ok(fs.existsSync(modulePath));
  const document = { documentElement: { dataset: {} } };
  const context = vm.createContext({ window: { navigator: { platform: 'Win32' }, document } });
  vm.runInContext(fs.readFileSync(modulePath, 'utf8'), context);
  assert.equal(document.documentElement.dataset.platform, 'windows');
  assert.equal(context.window.MailBeaconPlatform.shortcut('Enter'), 'Ctrl+Enter');
});
