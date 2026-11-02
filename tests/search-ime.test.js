const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// Model removal and focus loss, rather than replacing renderList with a spy.
// Input writes are tracked because assigning value or selection can itself
// interrupt the native input method even when the DOM node stays connected.
function page(invoke = async () => ({ items: [], coverage: [], hasMore: false })) {
  const listeners = {}, timers = new Set();
  const body = {}, overlay = { innerHTML: '' };
  const document = { activeElement: body, addEventListener(name, fn) { listeners[name] = fn; } };
  const decode = value => value.replaceAll('&quot;', '"').replaceAll('&#39;', "'").replaceAll('&lt;', '<').replaceAll('&gt;', '>').replaceAll('&amp;', '&');
  const list = {
    children: [],
    contains(node) { return this.children.some(child => child.contains(node)); },
    get innerHTML() { return this.children.map(child => child.html).join(''); },
    set innerHTML(html) {
      for (const child of [...this.children]) child.remove();
      const start = html.indexOf('<div class="list-head"');
      if (start >= 0) {
        const tags = /<div\b[^>]*>|<\/div>/g;
        tags.lastIndex = start;
        let depth = 0, end = 0, tag;
        while ((tag = tags.exec(html))) {
          depth += tag[0].startsWith('</') ? -1 : 1;
          if (depth === 0) { end = tags.lastIndex; break; }
        }
        this.children.push(section(html.slice(start, end), true));
        if (html.slice(end)) this.children.push(section(html.slice(end)));
      } else if (html) this.children.push(section(html));
    },
    insertAdjacentHTML(position, html) {
      assert.equal(position, 'beforeend');
      this.children.push(section(html));
    },
    querySelector(selector) {
      if (selector === '.list-head') return this.children.find(child => child.head) || null;
      for (const child of this.children) {
        const found = child.querySelector(selector);
        if (found) return found;
      }
      return null;
    },
    querySelectorAll() { return []; },
  };
  function section(html, head = false) {
    const element = {
      html, head,
      contains(node) { return node === this || node === this.q || node === this.rows; },
      get nextSibling() { return list.children[list.children.indexOf(this) + 1] || null; },
      get nextElementSibling() { return this.nextSibling; },
      remove() {
        if (this.contains(document.activeElement)) document.activeElement = body;
        const index = list.children.indexOf(this);
        if (index >= 0) list.children.splice(index, 1);
      },
      insertAdjacentHTML(position, content) {
        assert.equal(position, 'afterend');
        list.children.splice(list.children.indexOf(this) + 1, 0, section(content));
      },
      querySelector(selector) {
        if (selector === '#q') return this.q || null;
        if (selector === '.rows') return this.rows || null;
        if (selector === '.search-controls details') return this.advanced || null;
        return null;
      },
    };
    const match = /<input\b[^>]*id="q"[^>]*value="([^"]*)"/.exec(html);
    if (match) {
      let value = decode(match[1]);
      element.q = {
        id: 'q', dataset: {}, selectionStart: 0, selectionEnd: 0, selectionDirection: 'none',
        writes: { value: 0, focus: 0, selection: 0 },
        get isConnected() { return list.children.includes(element); },
        get value() { return value; },
        set value(next) { this.writes.value++; value = next; },
        nativeInput(next, start = next.length, end = start, direction = 'none') {
          value = next; this.selectionStart = start; this.selectionEnd = end; this.selectionDirection = direction;
        },
        focus() { this.writes.focus++; document.activeElement = this; },
        setSelectionRange(start, end, direction = 'none') {
          this.writes.selection++; this.selectionStart = start; this.selectionEnd = end; this.selectionDirection = direction;
        },
        closest(selector) {
          if (selector === '.list-head') return element;
          if (selector === '#list') return list;
          if (selector.split(/,\s*/).includes('input')) return this;
          return null;
        },
      };
    }
    if (/class="rows(?:\s[^"]*)?"/.test(html)) element.rows = {
      scrollTop: 0, clientHeight: 600,
      focus() { document.activeElement = this; },
      closest(selector) { return selector === '#list' ? list : null; },
      addEventListener() {},
    };
    if (html.includes('search-controls')) element.advanced = { open: false };
    return element;
  }
  document.querySelector = selector => selector === '#list' ? list : selector === '#overlay' ? overlay : list.querySelector(selector);
  document.querySelectorAll = () => [];
  const context = vm.createContext({
    document, console,
    setTimeout(fn, delay) { const timer = setTimeout(() => { timers.delete(timer); fn(); }, delay); timers.add(timer); return timer; },
    clearTimeout(timer) { timers.delete(timer); clearTimeout(timer); },
    window: {
      MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'),
      MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'),
      MailBeaconThreads: require('../ui/threads.js'), MailBeaconPrefs: require('../ui/prefs.js'),
      __TAURI__: { core: { invoke }, event: { listen: async () => {} } },
    },
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    renderSidebar = renderReader = updateBadge = () => {};
    globalThis.api = { S, renderList, render, onMailEvent, requestServerSearch, onKey, resetProductivityNavigation };
  `, context);
  const { S } = context.api;
  Object.assign(S, { acct: 'a', accounts: [{ id: 'a', email: 'demo@example.test', receiveEpoch: 1 }], searching: true });
  S.folders.a = [{ path: 'INBOX', name: '收件箱', role: 'inbox' }];
  S.lists.set('a|INBOX', []);
  context.api.renderList();
  const q = list.querySelector('#q');
  q.focus();
  q.writes = { value: 0, focus: 0, selection: 0 };
  return {
    ...context.api, document, list, q, listeners,
    key(key, extra = {}) {
      const event = { key, target: q, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; }, ...extra };
      context.api.onKey(event);
      return event;
    },
    close() { for (const timer of timers) clearTimeout(timer); },
  };
}

function preserved(t, q = t.q) {
  assert.equal(t.list.querySelector('#q') === q, true, 'the original search input stays connected');
  assert.equal(t.document.activeElement === q, true, 'native input focus is unchanged');
  assert.deepEqual(q.writes, { value: 0, focus: 0, selection: 0 }, 'rendering never writes to the focused input');
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

test('list refresh preserves a composing input, marked text and selection', t => {
  const p = page(); t.after(() => p.close());
  p.listeners.compositionstart({ target: p.q });
  p.q.nativeInput('jiangzheng', 0, 10, 'backward');
  p.renderList();
  preserved(p);
  assert.equal(p.q.value, 'jiangzheng');
  assert.deepEqual([p.q.selectionStart, p.q.selectionEnd, p.q.selectionDirection], [0, 10, 'backward']);
  assert.equal(p.S.query, '');
});

test('ordinary typing preserves the input and a non-collapsed selection', t => {
  const p = page(); t.after(() => p.close());
  p.q.nativeInput('合同报告', 1, 3, 'backward');
  p.listeners.input({ target: p.q, isComposing: false });
  preserved(p);
  assert.equal(p.S.query, '合同报告');
  assert.deepEqual([p.q.selectionStart, p.q.selectionEnd, p.q.selectionDirection], [1, 3, 'backward']);
});

test('background unread updates leave the composing search input untouched', t => {
  const p = page(); t.after(() => p.close());
  p.listeners.compositionstart({ target: p.q });
  p.q.nativeInput('jiangzheng');
  p.onMailEvent({ type: 'unread', accountId: 'a', count: 7 });
  preserved(p);
  assert.equal(p.S.live.a.unread, 7);
  assert.equal(p.q.value, 'jiangzheng');
});

test('a pending search result can render while the next word is composing', async t => {
  let finish;
  const pending = new Promise(resolve => { finish = resolve; });
  const p = page(async cmd => cmd === 'search_mail' ? pending : undefined); t.after(() => p.close());
  p.S.query = '合同';
  p.q.nativeInput('合同');
  p.requestServerSearch();
  await sleep(210);
  p.listeners.compositionstart({ target: p.q });
  p.q.nativeInput('合同jiangzheng');
  finish({ items: [], coverage: [], hasMore: false });
  await sleep(5);
  assert.equal(p.S.searchPage?.hasMore, false, 'the response was processed');
  preserved(p);
  assert.equal(p.q.value, '合同jiangzheng');
  assert.equal(p.S.query, '合同');
});

test('composition commit and final input submit the Chinese query once', t => {
  const p = page(); t.after(() => p.close());
  p.listeners.compositionstart({ target: p.q });
  p.q.nativeInput('jiangzheng');
  p.listeners.input({ target: p.q, isComposing: true });
  assert.equal(p.S.query, '');
  p.q.nativeInput('蒋政');
  p.listeners.compositionend({ target: p.q });
  const seq = p.S.searchSeq;
  p.listeners.input({ target: p.q, isComposing: false });
  preserved(p);
  assert.equal(p.S.query, '蒋政');
  assert.equal(p.S.searchComposing, false);
  assert.equal(p.S.searchSeq, seq, 'the final browser input does not restart the committed search');
});

for (const mode of ['composition flag', 'isComposing', 'WebKit keyCode 229']) {
  test(`Escape remains available to the input method with ${mode}`, t => {
    const p = page(); t.after(() => p.close());
    if (mode === 'composition flag') p.listeners.compositionstart({ target: p.q });
    const event = p.key('Escape', mode === 'isComposing' ? { isComposing: true } : mode === 'WebKit keyCode 229' ? { keyCode: 229 } : {});
    assert.equal(p.S.searching, true);
    assert.equal(event.defaultPrevented, false, 'the input method handles candidate cancellation');
    preserved(p);
  });
}

test('Escape closes search after the input method finishes', t => {
  const p = page(); t.after(() => p.close());
  p.listeners.compositionstart({ target: p.q });
  p.q.nativeInput('蒋政');
  p.listeners.compositionend({ target: p.q });
  p.key('Escape');
  assert.equal(p.S.searching, false);
  assert.equal(p.S.query, '');
  assert.equal(p.list.querySelector('#q'), null);
});

test('navigation clears the composition state before a new search opens', t => {
  const p = page(); t.after(() => p.close());
  p.listeners.compositionstart({ target: p.q });
  p.resetProductivityNavigation();
  p.renderList();
  p.S.searching = true;
  p.renderList();
  const q = p.list.querySelector('#q');
  q.nativeInput('报告');
  p.listeners.input({ target: q, isComposing: false });
  assert.equal(p.S.searchComposing, false);
  assert.equal(p.S.query, '报告');
});

test('an inactive retained input follows a newly selected saved query', t => {
  const p = page(); t.after(() => p.close());
  p.document.activeElement = {};
  p.S.query = '智能文件夹';
  p.renderList();
  assert.equal(p.list.querySelector('#q') === p.q, true);
  assert.equal(p.q.value, '智能文件夹');
  assert.equal(p.q.writes.value, 1);
  p.renderList();
  assert.equal(p.q.writes.value, 1, 'an unchanged inactive value is not rewritten');
});
