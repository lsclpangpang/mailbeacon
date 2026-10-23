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

test('AC01: empty, successful and failed reads describe only known attachments', async t => {
  let fail = false;
  const ui = setup(t, async () => { if (fail) throw Error('connection lost'); return page([]); });
  await tick();
  assert.match(ui.el('.attachment-items').innerHTML, /当前条件下还没有发现附件/);
  assert.match(ui.el('.attachment-coverage').textContent, /当前显示 0 个附件/);
  assert.match(ui.el('.attachment-coverage').textContent, /其他邮件可能还有附件/);
  assert.doesNotMatch(ui.el('.attachment-items').innerHTML, /会自动补充/);
  fail = true; await ui.center.refresh();
  assert.match(ui.el('.attachment-center-error').textContent, /connection lost/);
  assert.match(ui.el('.attachment-coverage').textContent, /刷新未完成/);
  assert.doesNotMatch(ui.el('.attachment-coverage').textContent, /同步完成/);
});

test('AC02: refresh preserves filters, three loaded pages, selected identity, keyboard focus and scroll', async t => {
  let rows = Array.from({ length: 350 }, (_, i) => item(i + 1));
  const ui = setup(t, async input => {
    const offset = Number(input.cursor || 0), end = offset + input.pageSize;
    return page(rows.slice(offset, end), { hasMore: end < rows.length, nextCursor: end < rows.length ? String(end) : null });
  });
  await tick(); ui.change('query', 'report'); await tick();
  ui.change('kind', 'pdf'); await tick();
  ui.click('more'); await tick(); ui.click('more'); await tick();
  assert.equal(ui.center.items.length, 300);
  ui.click('select', 149); ui.focus(149); ui.el('.attachment-items').scrollTop = 700;
  rows = [item(999), ...rows]; const before = ui.calls.length;
  await ui.center.refresh();
  assert.equal(ui.center.items.length, 300);
  assert.equal(ui.calls.length - before, 3);
  assert.ok(ui.calls.slice(before).every(input => input.query === 'report' && input.kind === 'pdf'));
  assert.match(ui.el('.attachment-items').innerHTML, /attachment-item selected" data-center-index="150"/);
  assert.equal(ui.el('.attachment-items').scrollTop, 700);
  assert.match(ui.focused.at(-1)?.selector || '', /data-center-index="150"/);
  assert.equal(ui.focused.at(-1)?.options.preventScroll, true);
});

test('AC03: index events debounce and invalidate an older response before the next request starts', async t => {
  const first = deferred();
  const ui = setup(t, (_input, n) => n === 1 ? first.promise : page([item(2)]));
  assert.equal(typeof ui.center.scheduleRefresh, 'function');
  ui.center.scheduleRefresh(); ui.center.scheduleRefresh(); ui.center.scheduleRefresh();
  first.resolve(page([item(1)])); await tick();
  assert.equal(ui.center.items.length, 0, 'an invalidated request cannot paint during the debounce window');
  await wait(250);
  assert.equal(ui.calls.length, 2);
  assert.equal(ui.center.items[0].source.uid, 2);
});

test('AC04: a newer filter request owns results and errors even when a stale refresh finishes later', async t => {
  const stale = deferred();
  const ui = setup(t, (_input, n) => n === 1 ? page([item(1)]) : n === 2 ? stale.promise : page([item(3)]));
  await tick(); const old = ui.center.refresh();
  ui.change('query', 'new'); await tick();
  stale.reject(Error('obsolete failure')); await old;
  assert.equal(ui.center.items[0].source.uid, 3);
  assert.equal(ui.el('.attachment-center-error').hidden, true);
  assert.equal(ui.el('[data-center-filter="query"]').value, 'new');
});

test('AC05: a failed page during refresh keeps all previous rows and selection', async t => {
  let fail = false;
  const rows = Array.from({ length: 150 }, (_, i) => item(i + 1));
  const ui = setup(t, input => {
    if (fail && input.cursor) throw Error('second page unavailable');
    return page(input.cursor ? rows.slice(100) : rows.slice(0, 100), { hasMore: !input.cursor, nextCursor: input.cursor ? null : '100' });
  });
  await tick(); ui.click('more'); await tick(); ui.click('select', 120);
  fail = true; await ui.center.refresh();
  assert.equal(ui.center.items.length, 150);
  assert.match(ui.el('.attachment-center-error').textContent, /second page unavailable/);
  assert.match(ui.el('.attachment-coverage').textContent, /仍显示上次读取的 150 个附件/);
});

test('AC06: POP3-only scope is unsupported, never an empty IMAP mailbox or a widened request', async t => {
  const ui = setup(t, () => { throw Error('must not query'); }, { targets: [], accounts: [{ id: 'pop', email: 'pop@example.test', receiveProtocol: 'pop3' }] });
  await tick();
  assert.equal(ui.calls.length, 0);
  assert.match(ui.el('.attachment-items').innerHTML, /暂不支持 POP3/);
  assert.doesNotMatch(ui.container.innerHTML, /option value="pop"/);
  assert.doesNotMatch(ui.el('.attachment-coverage').textContent, /没有附件/);
});

test('AC07: disposal cancels queued refresh and prevents late DOM writes', async t => {
  const late = deferred(); const ui = setup(t, () => late.promise);
  assert.equal(typeof ui.center.scheduleRefresh, 'function');
  ui.center.scheduleRefresh(); ui.center.dispose();
  const before = ui.el('.attachment-items').innerHTML;
  late.resolve(page([item(1)])); await wait(250);
  assert.equal(ui.calls.length, 1);
  assert.equal(ui.el('.attachment-items').innerHTML, before);
});

test('AC08: a temporarily shorter result does not shrink the user loaded page capacity', async t => {
  let size = 250;
  const ui = setup(t, input => {
    const offset = Number(input.cursor || 0), end = Math.min(size, offset + 100);
    return page(Array.from({ length: Math.max(0, end - offset) }, (_, i) => item(offset + i + 1)), { hasMore: end < size, nextCursor: end < size ? String(end) : null });
  });
  await tick(); ui.click('more'); await tick(); ui.click('more'); await tick();
  size = 50; await ui.center.refresh(); assert.equal(ui.center.items.length, 50);
  size = 350; await ui.center.refresh();
  assert.equal(ui.center.items.length, 300);
});

test('AC09: changed filters immediately invalidate old rows and pagination, including when the new request fails', async t => {
  const ui = setup(t, input => {
    if (input.query) throw Error('new filter unavailable');
    return page([item(1)], { hasMore: true, nextCursor: 'old-filter' });
  });
  await tick(); ui.change('query', 'new filter', 'input');
  assert.equal(ui.center.items.length, 0, 'old results must not appear to match the new filter');
  assert.equal(ui.el('[data-center-action="more"]').hidden, true);
  ui.click('more'); await wait(250);
  assert.equal(ui.calls.length, 2);
  assert.equal(ui.calls[1].cursor, null);
  assert.equal(ui.center.items.length, 0);
  assert.match(ui.el('.attachment-center-error').textContent, /new filter unavailable/);
  assert.match(ui.el('.attachment-coverage').textContent, /尚未读取成功/);
  assert.doesNotMatch(ui.el('.attachment-coverage').textContent, /当前显示 1/);
});
