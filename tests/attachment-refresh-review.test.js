const test = require('node:test');
const assert = require('node:assert/strict');
const { mount } = require('../ui/attachment-center.js');
const tick = () => new Promise(setImmediate);
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };
const item = uid => ({ source: { accountId: 'a', receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid }, partId: 0, rawHash: `hash-${uid}`, parserVersion: 1, filename: `file-${uid}.pdf`, sender: 'sender', mime: 'application/pdf', messageDate: uid, size: 1 });
const page = (items, extra = {}) => ({ items, coverage: [{ indexedMessages: 8, parsedMessages: 4, attachmentCount: items.length }], hasMore: false, nextCursor: null, catalogIncomplete: [], ...extra });

function setup(t, reply, options = {}) {
  const handlers = new Map(), elements = new Map(), calls = [], focused = [];
  const document = { activeElement: null };
  const container = {
    innerHTML: '', ownerDocument: document,
    addEventListener(type, fn, { signal }) { handlers.set(type, fn); signal.addEventListener('abort', () => handlers.delete(type), { once: true }); },
    querySelector(selector) {
      if (!elements.has(selector)) elements.set(selector, { value: selector === '[data-center-filter="kind"]' ? 'all' : '', textContent: '', innerHTML: '', hidden: false, scrollTop: 0,
        focus(options) { focused.push({ selector, options }); } });
      return elements.get(selector);
    },
    querySelectorAll() { return []; },
    contains(node) { return !!node?.inCenter; },
  };
  const center = mount({ container, targets: [{ accountId: 'a', receiveEpoch: 1, folder: 'INBOX' }], accounts: [{ id: 'a', email: 'a@example.test', receiveProtocol: 'imap' }],
    invoke: async (command, { input }) => { calls.push(structuredClone(input)); return reply(input, calls.length); },
    formatDate: String, formatSize: String, accountName: String, ...options });
  t.after(() => center.dispose());
  const button = (action, index) => ({ dataset: { centerAction: action }, isConnected: true, inCenter: true,
    closest(selector) { return selector === '[data-center-action]' ? this : selector === '[data-center-index]' && index != null ? { dataset: { centerIndex: String(index) } } : null; } });
  return { center, calls, container, document, focused,
    el: selector => container.querySelector(selector),
    click: (action, index) => handlers.get('click')({ target: button(action, index) }),
    focus: index => { document.activeElement = button('select', index); },
    change(name, value, type = 'change') { const node = container.querySelector(`[data-center-filter="${name}"]`); node.value = value; node.matches = () => true; handlers.get(type)({ target: node }); },
  };
}

test('AR01: clicking more during a filter debounce cannot cancel the new first-page query', async t => {
  const ui = setup(t, input => {
    if (input.cursor && input.query !== '') throw Error('附件索引或条件已改变，请刷新');
    return input.query ? page([item(99)]) : page([item(1)], {hasMore:true,nextCursor:'old-query-cursor'});
  });
  await tick();
  ui.change('query', 'new search', 'input');
  ui.click('more');
  await wait(250);
  assert.equal(ui.el('[data-center-filter="query"]').value, 'new search');
  assert.deepEqual(ui.center.items.map(row => row.source.uid), [99]);
  assert.equal(ui.el('.attachment-center-error').hidden, true);
});
