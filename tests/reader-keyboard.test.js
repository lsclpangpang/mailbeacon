const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const plain = (v) => JSON.parse(JSON.stringify(v));
const message = (uid, seen = true, extra = {}) => ({ accountId: 'a', folder: 'INBOX', uid, date: uid, seen, ...extra });
const selection = (m) => ({ accountId: m.accountId, folder: m.folder, uid: m.uid });
const target = (matches = []) => ({ id: '', closest: (s) => matches.some((m) => s.split(/,\s*/).includes(m)) ? target(matches) : null });

function app({ reducedMotion = true } = {}) {
  const listeners = {}, calls = [], opened = [], toasts = [], actionEvents = [];
  const frames = new Map();
  let motionNow = 1000, frameId = 0;
  let keyEvent = null;
  const scroll = { scrollTop: 0, clientHeight: 600, scrollHeight: 600,
    scrollBy(opts) { calls.push({ target: 'outer', ...opts }); this.scrollTop += opts.top; },
    getBoundingClientRect: () => ({ top: 52, bottom: 652 }) };
  const actions = Object.fromEntries(['archive', 'reply', 'reply-all', 'forward', 'search'].map(action => [action, {
    disabled: false,
    click() {
      if (this.disabled) return;
      calls.push({ action });
      actionEvents.push({ action, defaultPrevented: !!keyEvent?.defaultPrevented });
    },
  }]));
  const overlay = { innerHTML: '' };
  let frame = null;
  const query = (s) => {
    if (s === '#reader') return reader;
    if (s === '#overlay') return overlay;
    if (s === '.read-scroll' || s === '#reader .read-scroll') return scroll;
    if (s === '#mail-frame') return frame;
    const action = s.match(/\[data-action=["']?([\w-]+)["']?\]/)?.[1];
    return action ? actions[action] || null : null;
  };
  const reader = { querySelector: query, dataset: {}, innerHTML: '' };
  const context = vm.createContext({
    window: { MailBeaconReaderNav: require('../ui/reader-nav.js'),
      MailBeaconReaderMotion: require('../ui/reader-motion.js'),
      matchMedia: () => ({ matches: reducedMotion }),
      requestAnimationFrame: fn => { frames.set(++frameId, fn); return frameId; },
      cancelAnimationFrame: id => frames.delete(id),
      MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconThreads: require('../ui/threads.js'),
      __TAURI__: { core: { invoke: async (cmd,args) => cmd==='execute_mail_operation' ? {items:args.request.items.map(source=>({source,status:'succeeded'}))} : null }, event: { listen: async () => {} } } },
    document: { querySelector: query, querySelectorAll: () => [], activeElement: null,
      addEventListener: (name, fn) => { listeners[name] = fn; } },
    console, setTimeout, clearTimeout, opened, toasts,
    Date: class extends Date { static now() { return motionNow; } },
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    render = renderList = renderReader = updateBadge = () => {};
    toast = (text) => toasts.push(text);
    loadList = async () => {};
    showOperationOutcome = () => {};
    openMessage = (sel) => { opened.push(sel); S.sel = sel; patchMsg(sel.accountId, sel.folder, sel.uid, { seen: true }); };
    globalThis.api = { S, onKey, listEntries };
  `, context);
  const { S } = context.api;
  S.accounts = [{ id: 'a', email: 'me@example.test' }];
  S.acct = 'a';
  S.lists.set('a|INBOX', [message(9), message(8), message(7, false), message(6, false)]);
  S.sel = selection(message(9));
  const key = (value = ' ', extra = {}) => {
    const event = { key: value, target: target(), defaultPrevented: false,
      preventDefault() { this.defaultPrevented = true; }, ...extra };
    keyEvent = event;
    try { listeners.keydown(event); } finally { keyEvent = null; }
    return event.defaultPrevented;
  };
  return { ...context.api, key, opened, calls, toasts, scroll, actions, actionEvents, overlay,
    advance(ms) { motionNow += ms; const pending = [...frames.values()]; frames.clear(); pending.forEach(fn => fn(motionNow)); },
    pointerEvent: name => listeners[name]?.({ target: target() }),
    setFrame(inner, bottom = 652) { frame = { contentDocument: { scrollingElement: inner }, getBoundingClientRect: () => ({ top: bottom - inner.clientHeight, bottom }) }; } };
}

test('Space at the bottom prefers the next unread and prevents native scrolling', () => {
  const t = app();
  assert.equal(t.key(), true);
  assert.deepEqual(plain(t.opened), [selection(message(7))]);
});

test('Space scrolls a screen with overlap without selecting or marking another mail read', () => {
  const t = app();
  t.scroll.scrollHeight = 2000;
  assert.equal(t.key(), true);
  assert.equal(t.opened.length, 0);
  assert.equal(t.calls[0].target, 'outer');
  assert.equal(t.calls[0].top, 540);
  assert.equal(t.S.lists.get('a|INBOX')[2].seen, false);
});

test('Space falls back to the strict next item and does not wrap at the end', () => {
  const t = app();
  t.S.lists.get('a|INBOX').forEach((m) => { m.seen = true; });
  t.key();
  assert.equal(t.opened[0].uid, 8);
  t.S.sel = selection(message(6));
  t.key();
  assert.equal(t.opened.length, 1);
  assert.equal(t.S.sel.uid, 6);
  assert.equal(t.toasts.at(-1), '没有下一封了');
});

test('Space reports a loaded-list boundary when older mail is available', () => {
  const t = app();
  t.S.sel = selection(message(6));
  t.S.morePages.set('a|INBOX', true);
  t.key();
  assert.equal(t.toasts.at(-1), '当前已加载列表已到末尾');
});

test('Space never restarts from the first item if a search update removed the selection', () => {
  const t = app();
  t.S.sel = selection(message(100));
  t.key();
  assert.equal(t.opened.length, 0);
  assert.equal(t.S.sel.uid, 100);
});

test('unread view computes the target before replacing the selected read row', () => {
  const t = app();
  t.S.folder = '__unread';
  t.S.lists.set('a|__unread', [message(9), message(8, false), message(7, false)]);
  t.key(); t.key(); t.key();
  assert.deepEqual(t.opened.map((m) => m.uid), [8, 7]);
  assert.equal(t.S.sel.uid, 7);
});

test('search navigation uses current results rather than the inbox list', () => {
  const t = app();
  t.S.query = 'result';
  t.S.results.set('a|INBOX', { query: 'result', items: [message(9), message(4), message(2, false)] });
  t.key();
  assert.equal(t.opened[0].uid, 2);
});

test('conversation navigation resolves an older selected member and aggregate unread state', () => {
  const t = app();
  t.S.threads = true;
  t.S.lists.set('a|INBOX', [message(9, true, { messageId: 'a', references: ['root'] }),
    message(8), message(7, true, { messageId: 'b', references: ['other'] }),
    message(6, false, { messageId: 'other' }), message(5, true, { messageId: 'root' })]);
  t.S.sel = selection(message(5));
  t.key();
  assert.equal(t.opened[0].uid, 7, 'the latest read member represents the next unread conversation');
});

test('Space can advance while a partial body is still loading', () => {
  const t = app();
  t.S.msg = { partial: true };
  t.key();
  assert.equal(t.opened[0].uid, 7);
});

for (const name of ['input', 'textarea', 'select', '[contenteditable]', 'button', '[role="button"]']) {
  test(`Space preserves native behavior for ${name}`, () => {
    const t = app();
    assert.equal(t.key(' ', { target: target([name]) }), false);
    assert.equal(t.opened.length, 0);
  });
}

test('a focused list-row button can still use Space to navigate', () => {
  const t = app();
  t.key(' ', { target: target(['button', '.row[data-open]']) });
  assert.equal(t.opened[0].uid, 7);
});

for (const extra of [{ metaKey: true }, { ctrlKey: true }, { altKey: true }, { shiftKey: true }, { isComposing: true }, { keyCode: 229 }]) {
  test(`Space ignores protected key state ${JSON.stringify(extra)}`, () => {
    const t = app();
    assert.equal(t.key(' ', extra), false);
    assert.equal(t.opened.length, 0);
  });
}

test('held Space is consumed without scrolling or rapidly marking messages read', () => {
  const t = app();
  t.key();
  assert.equal(t.key(' ', { repeat: true }), true);
  assert.equal(t.opened.length, 1);
});

test('Space ignores an overlay and an empty reader', () => {
  const t = app();
  t.overlay.innerHTML = '<div class="sheet">compose</div>';
  assert.equal(t.key(), false);
  t.overlay.innerHTML = '';
  t.S.sel = null;
  assert.equal(t.key(), false);
  assert.equal(t.opened.length, 0);
});

test('Space from a long-mail iframe scrolls the inner body before changing mail', () => {
  const t = app();
  const inner = { scrollTop: 0, clientHeight: 8000, scrollHeight: 15000,
    scrollBy(opts) { t.calls.push({ target: 'inner', ...opts }); } };
  t.setFrame(inner);
  t.key(' ', { target: target() });
  assert.equal(t.opened.length, 0);
  assert.equal(t.calls[0].target, 'inner');
  assert.equal(t.calls[0].top, 540);
});

test('J/K and arrows keep strict list order, while E/R/S keep existing actions', async () => {
  const t = app();
  t.key('j'); t.key('k'); t.key('ArrowDown'); t.key('ArrowUp');
  assert.deepEqual(t.opened.map((m) => m.uid), [8, 9, 8, 9]);
  t.key('e'); t.key('r'); t.key('s');
  for (let i=0;i<10;i++) await Promise.resolve();
  assert.deepEqual(t.calls.filter((c) => c.action).map((c) => c.action), ['archive', 'reply']);
  assert.equal(t.S.lists.get('a|INBOX')[0].flagged, true);
});

test('an inner scroll page is bounded by the actual visible part of the mail frame', () => {
  const t = app();
  const inner = { scrollTop: 0, clientHeight: 8000, scrollHeight: 15000,
    scrollBy(opts) { t.calls.push({ target: 'inner', ...opts }); } };
  t.setFrame(inner, 352); // viewport is 52..652: only 300px of the body is visible
  t.key();
  assert.equal(t.opened.length, 0);
  assert.equal(t.calls[0].top, 270);
});

for (const [key, action] of [['r', 'reply'], ['a', 'reply-all'], ['f', 'forward']]) {
  test(`${key.toUpperCase()} cancels native text entry before opening ${action}`, () => {
    const t = app();
    t.S.msg = { partial: false };
    const prevented = t.key(key);
    assert.deepEqual(t.actionEvents, [{ action, defaultPrevented: true }],
      'opening and focusing the editor must happen after cancelling the triggering keystroke');
    assert.equal(prevented, true);
  });
}

test('slash cancels native text entry before opening search', () => {
  const t = app();
  assert.equal(t.key('/'), true);
  assert.deepEqual(t.actionEvents, [{ action: 'search', defaultPrevented: true }]);
});

test('reply, forward and search letters remain ordinary text inside editable fields', () => {
  for (const name of ['input', 'textarea', '[contenteditable]']) {
    for (const key of ['r', 'a', 'f', 'c', '/']) {
      const t = app();
      t.S.msg = { partial: false };
      assert.equal(t.key(key, { target: target([name]) }), false, `${name}: ${key}`);
      assert.equal(t.actionEvents.length, 0, `${name}: ${key} must not open another editor/search`);
    }
  }
});

test('an existing overlay leaves reply, forward and search letters untouched', () => {
  for (const key of ['r', 'a', 'f', 'c', '/']) {
    const t = app();
    t.S.msg = { partial: false };
    t.overlay.innerHTML = '<div class="sheet compose-sheet">editor</div>';
    assert.equal(t.key(key), false, key);
    assert.equal(t.actionEvents.length, 0, `${key} must not invoke a reader shortcut behind the overlay`);
  }
});

test('reply and forward do not consume keys when no message can open an editor', () => {
  for (const key of ['r', 'a', 'f']) {
    for (const state of ['no-selection', 'partial']) {
      const t = app();
      if (state === 'no-selection') t.S.sel = null;
      else t.S.msg = { partial: true };
      assert.equal(t.key(key), false, `${key}: ${state}`);
      assert.equal(t.actionEvents.length, 0, `${key}: ${state}`);
    }
  }
});

test('Space scrolls through intermediate positions and a quick second press cannot advance mail', () => {
  const t = app({ reducedMotion: false });
  t.scroll.scrollHeight = 700;
  t.key();
  t.advance(90);
  assert.ok(t.scroll.scrollTop > 0 && t.scroll.scrollTop < 100);
  t.key();
  t.advance(90);
  assert.equal(t.scroll.scrollTop, 100);
  assert.equal(t.opened.length, 0, 'finishing a scroll never opens a message automatically');
  t.key();
  assert.equal(t.opened[0].uid, 7);
});

test('manual scrolling and Escape cancel the pending Space motion', () => {
  for (const event of ['wheel', 'touchstart', 'pointerdown']) {
    const t = app({ reducedMotion: false });
    t.scroll.scrollHeight = 2000;
    t.key(); t.advance(60);
    const top = t.scroll.scrollTop;
    t.pointerEvent(event); t.advance(200);
    assert.equal(t.scroll.scrollTop, top, event);
    t.key(); t.advance(60);
    const beforeEscape = t.scroll.scrollTop;
    t.key('Escape'); t.advance(200);
    assert.equal(t.scroll.scrollTop, beforeEscape);
    assert.equal(t.S.sel, null);
  }
});


test('Cmd+A is scoped to the list and preserves editor and search text selection', () => {
  const t=app();
  assert.equal(t.key('a',{metaKey:true,target:target(['#list'])}),true);
  assert.equal(t.S.batch.size,4);
  t.S.batch.clear();
  assert.equal(t.key('a',{metaKey:true,target:target(['#list','input'])}),false);
  assert.equal(t.S.batch.size,0);
  assert.equal(t.key('a',{metaKey:true,isComposing:true,target:target(['#list'])}),false);
  assert.equal(t.S.batch.size,0);
});
test('J/K clear batch selection, Escape clears it before closing the reader', () => {
  const t=app();
  t.key('a',{metaKey:true,target:target(['#list'])});
  t.key('Escape');
  assert.equal(t.S.batch.size,0);
  assert.ok(t.S.sel);
  t.key('a',{metaKey:true,target:target(['#list'])});
  t.key('j');
  assert.equal(t.S.batch.size,0);
  assert.equal(t.opened.at(-1).uid,8);
});
