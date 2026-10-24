const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Exercise both real iframe mount functions with a document whose CSS makes
// its scroll height depend on its viewport, as vh/% email layouts can do.
function mount(kind, contentHeight, images = [], { platform = 'macos', remote = false } = {}) {
  const observers = [];
  const mutations = [];
  const listeners = {};
  const listenerOptions = {};
  const calls = [];
  const frame = { style: { height: '180px' }, isConnected: true };
  const links = [{ target: '_self' }, { target: '_top' }, { target: '' }];
  frame.contentDocument = {
    documentElement: { get scrollHeight() { return contentHeight(frame); } },
    addEventListener(name, handler, options) { listeners[name] = handler; listenerOptions[name] = options; },
    querySelectorAll: selector => selector === 'img' ? images : (selector.includes('a[href]') ? links : []),
  };
  const sheet = { querySelector: () => frame, _quotePreview: { html: '<p>Original</p>' } };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); } }, event: { listen: async () => {} } }, MailPulseQuote: require('../ui/compose-quote.js') },
    document: { querySelector: () => frame, addEventListener() {} },
    ResizeObserver: class { constructor(callback) { this.callback = callback; observers.push(this); } observe() {} disconnect() {} },
    MutationObserver: class { constructor(callback) { this.callback = callback; mutations.push(this); } observe() {} },
    console, setTimeout, clearTimeout,
  });
  context.window.MailBeaconPlatform = require('../ui/platform.js').forPlatform(platform);
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + '\nglobalThis.api = { mountFrame, mountComposeQuote, linkify };', context);
  if (remote) vm.runInContext('S.showImages.add("from:test@example.test")', context);
  sheet._quoteRemoteImages = remote;
  if (kind === 'quote') context.api.mountComposeQuote(sheet);
  else context.api.mountFrame({ html: '<p>Original</p>', from: { addr: 'test@example.test' } }, { accountId: 'a', folder: 'INBOX', uid: 1 });
  frame.onload();
  return { frame, resize: () => observers.at(-1).callback(), mutate: records => mutations.at(-1).callback(records), listeners, listenerOptions, calls, links, linkify: context.api.linkify };
}

function mailImage(complete, naturalWidth) {
  const attrs = new Set();
  return {
    tagName: 'IMG', complete, naturalWidth,
    toggleAttribute(name, enabled) { if (enabled) attrs.add(name); else attrs.delete(name); },
    hasAttribute(name) { return attrs.has(name); },
  };
}

for (const kind of ['reader', 'quote']) {
  test(`${kind} upgrades allowed HTTP images on macOS without allowing blocked remote content`, () => {
    const allowed = mount(kind, () => 300, [], { platform: 'macos', remote: true }).frame.srcdoc;
    assert.match(allowed, /upgrade-insecure-requests/);
    assert.match(allowed, /img-src data: cid: https: http:/);
    const blocked = mount(kind, () => 300, [], { platform: 'macos', remote: false }).frame.srcdoc;
    assert.doesNotMatch(blocked, /upgrade-insecure-requests|img-src[^;\"]*https:/);
    const windows = mount(kind, () => 300, [], { platform: 'windows', remote: true }).frame.srcdoc;
    assert.doesNotMatch(windows, /upgrade-insecure-requests/);
    assert.match(windows, /img-src data: cid: https: http:/);
  });

  test(`${kind} scales decoded images without inflating blocked or pending image placeholders`, () => {
    const loaded = mailImage(true, 1600);
    const blocked = mailImage(true, 0);
    const pending = mailImage(false, 0);
    // A stale/sender-provided marker must not make an undecoded image responsive.
    blocked.toggleAttribute('data-mailbeacon-loaded', true);
    const { frame } = mount(kind, () => 450, [loaded, blocked, pending]);
    assert.equal(loaded.hasAttribute('data-mailbeacon-loaded'), true);
    assert.equal(blocked.hasAttribute('data-mailbeacon-loaded'), false);
    assert.equal(pending.hasAttribute('data-mailbeacon-loaded'), false);
    assert.doesNotMatch(frame.srcdoc, /img\s*\{[^}]*height\s*:\s*auto/);
    assert.match(frame.srcdoc, /img:where\(\[data-mailbeacon-loaded\]\)\s*\{height:auto\}/);
  });

  test(`${kind} updates image sizing and frame height when an image later loads or fails`, () => {
    const img = mailImage(false, 0);
    let height = 300;
    const { frame, listeners, listenerOptions } = mount(kind, () => height, [img]);
    assert.equal(listenerOptions.load, true, 'image load events do not bubble');
    assert.equal(listenerOptions.error, true, 'image error events do not bubble');
    img.complete = true;
    img.naturalWidth = 1600;
    height = 500;
    listeners.load({ target: img });
    assert.equal(img.hasAttribute('data-mailbeacon-loaded'), true);
    assert.equal(frame.style.height, '500px');
    img.naturalWidth = 0;
    height = 350;
    listeners.error({ target: img });
    assert.equal(img.hasAttribute('data-mailbeacon-loaded'), false);
    assert.equal(frame.style.height, '350px');
    // Fonts, stylesheets and other load targets must not be treated as images.
    listeners.load({ target: { tagName: 'LINK' } });
  });

  test(`${kind} observes decoding and source changes even when sandboxed image events are suppressed`, async () => {
    const img = mailImage(false, 0);
    let resolve, reject;
    img.decode = () => new Promise((yes, no) => { resolve = yes; reject = no; });
    const { mutate } = mount(kind, () => 300, [img]);
    img.complete = true;
    img.naturalWidth = 1600;
    resolve();
    await Promise.resolve();
    assert.equal(img.hasAttribute('data-mailbeacon-loaded'), true);
    img.complete = false;
    img.naturalWidth = 0;
    mutate([{ target: img }]);
    assert.equal(img.hasAttribute('data-mailbeacon-loaded'), false);
    img.complete = true;
    reject(new Error('Image could not be decoded'));
    await Promise.resolve();
    assert.equal(img.hasAttribute('data-mailbeacon-loaded'), false);
  });

  test(`${kind} viewport-dependent email height converges instead of growing forever`, () => {
    const { frame, resize } = mount(kind, frame => parseFloat(frame.style.height) + 40);
    for (let i = 0; i < 250; i++) resize();
    const height = frame.style.height;
    resize();
    assert.equal(frame.style.height, height);
    assert.ok(parseFloat(height) <= 8000);
  });

  test(`${kind} keeps normal mail height and bounds oversized layouts without truncating the source`, () => {
    let height = 450;
    const { frame, resize } = mount(kind, () => height);
    assert.equal(frame.style.height, '450px');
    const html = frame.srcdoc;
    height = 100000000;
    resize();
    assert.ok(parseFloat(frame.style.height) <= 8000);
    assert.equal(frame.srcdoc, html);
    assert.match(frame.srcdoc, /Original/);
  });
}

test('plain-text mail bodies linkify every opener-supported link scheme', () => {
  const { linkify } = mount('reader', () => 300);
  const linked = linkify('http://example.test mailto:person@example.test tel:+8613812345678');
  assert.match(linked, /<a href="http:\/\/example\.test" data-link="http:\/\/example\.test">/);
  assert.match(linked, /<a href="mailto:person@example\.test" data-link="mailto:person@example\.test">/);
  assert.match(linked, /<a href="tel:\+8613812345678" data-link="tel:\+8613812345678">/);
  assert.doesNotMatch(linkify('javascript:alert(1)'), /data-link/);
});

for (const kind of ['reader', 'quote']) {
  test(`${kind} hyperlinks are opened through the system opener`, async () => {
    for (const url of ['https://example.test/report', 'mailto:person@example.test', 'tel:+8613812345678']) {
      const mounted = mount(kind, () => 300);
      const link = { href: url };
      const target = { closest: selector => selector === 'a[href]' ? link : null };
      let prevented = 0;
      await mounted.listeners.click({ target, preventDefault() { prevented++; } });
      assert.equal(prevented, 1);
      assert.equal(mounted.calls.length, 1);
      assert.equal(mounted.calls[0].cmd, 'open_link');
      assert.equal(mounted.calls[0].input.url, url);
    }
  });

  test(`${kind} keeps internal fragment links inside the iframe`, async () => {
    const mounted = mount(kind, () => 300);
    let scrolled = 0;
    const destination = { scrollIntoView: () => { scrolled++; } };
    const link = {
      href: 'about:srcdoc#details',
      getAttribute: name => name === 'href' ? '#details' : null,
      ownerDocument: { getElementById: id => id === 'details' ? destination : null },
    };
    const target = { closest: selector => selector === 'a[href]' ? link : null };
    let prevented = 0;
    await mounted.listeners.click({ target, preventDefault() { prevented++; } });
    assert.equal(prevented, 1);
    assert.equal(scrolled, 1);
    assert.equal(mounted.calls.length, 0);
  });
}

// WebKit 不给沙箱 iframe 跑外层挂的点击监听，链接只能靠新窗口请求交给后端；
// 邮件自带的 _self/_top 会让 iframe 原地跳转或被拦下，所以要统一改成 _blank。
test('reader rewrites every mail link to request a new window', () => {
  const { links } = mount('reader', () => 300);
  assert.deepEqual(links.map(a => a.target), ['_blank', '_blank', '_blank']);
});

// 正文图片：双击写入系统临时目录并用默认程序打开（与附件双击同链路）。

for (const kind of ['reader', 'quote']) {
  test(`${kind} double-click on a data: image opens it through open_inline_image`, async () => {
    const mounted = mount(kind, () => 300);
    assert.equal(typeof mounted.listeners.dblclick, 'function', 'mail frame must listen for dblclick on images');
    const img = { tagName: 'IMG', currentSrc: '', src: 'data:image/png;base64,iVBORw0KGgo=' };
    const target = { closest: selector => selector === 'img' ? img : null };
    let prevented = 0;
    await mounted.listeners.dblclick({ target, preventDefault() { prevented++; } });
    assert.equal(prevented, 1);
    assert.equal(mounted.calls.length, 1);
    assert.equal(mounted.calls[0].cmd, 'open_inline_image');
    assert.equal(mounted.calls[0].input.src, img.src);
  });

  test(`${kind} double-click on a remote image opens it through open_inline_image`, async () => {
    const mounted = mount(kind, () => 300);
    assert.equal(typeof mounted.listeners.dblclick, 'function');
    const img = { tagName: 'IMG', currentSrc: 'https://example.test/a.png', src: 'https://example.test/a.png' };
    const target = { closest: selector => selector === 'img' ? img : null };
    await mounted.listeners.dblclick({ target, preventDefault() {} });
    assert.equal(mounted.calls.length, 1);
    assert.equal(mounted.calls[0].cmd, 'open_inline_image');
    assert.equal(mounted.calls[0].input.src, 'https://example.test/a.png');
  });

  test(`${kind} prefers currentSrc over src when opening an image`, async () => {
    const mounted = mount(kind, () => 300);
    const img = { tagName: 'IMG', currentSrc: 'https://cdn.example.test/full.png', src: 'https://example.test/thumb.png' };
    const target = { closest: selector => selector === 'img' ? img : null };
    await mounted.listeners.dblclick({ target, preventDefault() {} });
    assert.equal(mounted.calls[0].input.src, 'https://cdn.example.test/full.png');
  });

  test(`${kind} double-click outside images does not open anything`, async () => {
    const mounted = mount(kind, () => 300);
    assert.equal(typeof mounted.listeners.dblclick, 'function');
    const target = { closest: () => null };
    await mounted.listeners.dblclick({ target, preventDefault() {} });
    assert.equal(mounted.calls.length, 0);
  });

  test(`${kind} double-click on an image with empty src does not invoke`, async () => {
    const mounted = mount(kind, () => 300);
    const img = { tagName: 'IMG', currentSrc: '', src: '' };
    const target = { closest: selector => selector === 'img' ? img : null };
    await mounted.listeners.dblclick({ target, preventDefault() {} });
    assert.equal(mounted.calls.length, 0);
  });
}

test('open_inline_image backend writes under temp mailbeacon-images and opens like attachments', () => {
  const lib = fs.readFileSync(require.resolve('../src-tauri/src/lib.rs'), 'utf8');
  const start = lib.indexOf('async fn open_inline_image');
  assert.ok(start >= 0, 'must expose async open_inline_image command');
  const slice = lib.slice(start, start + 3500);
  assert.match(slice, /temp_dir\(\)/, 'must use std::env::temp_dir');
  assert.match(slice, /mailbeacon-images/, 'temp files live under …/mailbeacon-images/');
  assert.match(slice, /open_with_system/, 'macOS open path must use open_with_system like open_attachment');
  const handlers = lib.match(/generate_handler!\[([\s\S]*?)\]/);
  assert.ok(handlers, 'generate_handler! list missing');
  assert.match(handlers[1], /\bopen_inline_image\b/, 'open_inline_image must be registered in generate_handler!');
});
