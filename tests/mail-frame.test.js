const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Exercise both real iframe mount functions with a document whose CSS makes
// its scroll height depend on its viewport, as vh/% email layouts can do.
function mount(kind, contentHeight) {
  const observers = [];
  const frame = { style: { height: '180px' }, isConnected: true };
  frame.contentDocument = {
    documentElement: { get scrollHeight() { return contentHeight(frame); } },
    addEventListener() {},
  };
  const sheet = { querySelector: () => frame, _quotePreview: { html: '<p>Original</p>' } };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { __TAURI__: { core: { invoke: async () => {} }, event: { listen: async () => {} } }, MailPulseQuote: { previewDocument: () => '<p>Original</p>' } },
    document: { querySelector: () => frame, addEventListener() {} },
    ResizeObserver: class { constructor(callback) { this.callback = callback; observers.push(this); } observe() {} disconnect() {} },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + '\nglobalThis.api = { mountFrame, mountComposeQuote };', context);
  if (kind === 'quote') context.api.mountComposeQuote(sheet);
  else context.api.mountFrame({ html: '<p>Original</p>', from: { addr: 'test@example.test' } }, { accountId: 'a', folder: 'INBOX', uid: 1 });
  frame.onload();
  return { frame, resize: () => observers.at(-1).callback() };
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
