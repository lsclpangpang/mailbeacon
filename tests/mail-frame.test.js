const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Exercise both real iframe mount functions with a document whose CSS makes
// its scroll height depend on its viewport, as vh/% email layouts can do.
function mount(kind, contentHeight) {
  const observers = [];
  const listeners = {};
  const calls = [];
  const frame = { style: { height: '180px' }, isConnected: true };
  const links = [{ target: '_self' }, { target: '_top' }, { target: '' }];
  frame.contentDocument = {
    documentElement: { get scrollHeight() { return contentHeight(frame); } },
    addEventListener(name, handler) { listeners[name] = handler; },
    querySelectorAll: selector => (selector.includes('a[href]') ? links : []),
  };
  const sheet = { querySelector: () => frame, _quotePreview: { html: '<p>Original</p>' } };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); } }, event: { listen: async () => {} } }, MailPulseQuote: { previewDocument: () => '<p>Original</p>' } },
    document: { querySelector: () => frame, addEventListener() {} },
    ResizeObserver: class { constructor(callback) { this.callback = callback; observers.push(this); } observe() {} disconnect() {} },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + '\nglobalThis.api = { mountFrame, mountComposeQuote, linkify };', context);
  if (kind === 'quote') context.api.mountComposeQuote(sheet);
  else context.api.mountFrame({ html: '<p>Original</p>', from: { addr: 'test@example.test' } }, { accountId: 'a', folder: 'INBOX', uid: 1 });
  frame.onload();
  return { frame, resize: () => observers.at(-1).callback(), listeners, calls, links, linkify: context.api.linkify };
}

for (const kind of ['reader', 'quote']) {
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
