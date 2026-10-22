const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function clock() {
  let now = 10000, id = 0;
  const timers = new Map();
  return {
    Date: class extends Date { static now() { return now; } },
    setTimeout(fn, delay = 0) { const token = ++id; timers.set(token, { fn, at: now + delay }); return token; },
    clearTimeout(token) { timers.delete(token); },
    advance(ms) {
      const until = now + ms;
      for (;;) {
        const next = [...timers].filter(([, timer]) => timer.at <= until).sort((a, b) => a[1].at - b[1].at)[0];
        if (!next) break;
        now = next[1].at;
        timers.delete(next[0]);
        next[1].fn();
      }
      now = until;
    },
    callbacks: () => [...timers.values()].map(timer => timer.fn),
    now: () => now,
  };
}

const decode = text => text.replace(/&(amp|lt|gt|quot|#39);/g,
  (_, entity) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" })[entity]);

// Small DOM fixture for the real reader render/event paths. It models ownership,
// replacement and button disabled state, while leaving layout to browser checks.
function setup(invoke = async () => fullMessage(), { reducedMotion = false } = {}) {
  const time = clock(), listeners = {}, calls = [], notices = [];
  const document = { activeElement: null, addEventListener: (name, fn) => { listeners[name] = fn; } };
  class Element {
    constructor(tagName, attributes = {}) {
      this.tagName = tagName.toUpperCase();
      this.attributes = { ...attributes };
      this.dataset = {};
      this.children = [];
      this.parentElement = null;
      this.style = {};
      this.id = attributes.id || '';
      this.disabled = 'disabled' in attributes;
      this.hidden = 'hidden' in attributes;
      this.className = attributes.class || '';
      this.classList = {
        contains: name => this.className.split(/\s+/).includes(name),
        add: (...names) => { this.className = [...new Set([...this.className.split(/\s+/).filter(Boolean), ...names])].join(' '); },
        remove: (...names) => { this.className = this.className.split(/\s+/).filter(name => !names.includes(name)).join(' '); },
        toggle: (name, force) => { const add = force ?? !this.classList.contains(name); this.classList[add ? 'add' : 'remove'](name); return add; },
      };
      for (const [name, value] of Object.entries(attributes)) if (name.startsWith('data-')) this.dataset[this.dataName(name)] = value;
    }
    dataName(name) { return name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase()); }
    get isConnected() { return this === root || !!this.parentElement?.isConnected; }
    get textContent() { return this._text ?? this.children.map(child => child.textContent).join(''); }
    set textContent(value) { this._text = String(value); this.children = []; }
    get innerHTML() { return this._html || ''; }
    set innerHTML(html) {
      this._html = String(html);
      this._text = undefined;
      for (const child of this.children) child.parentElement = null;
      this.children = [];
      const stack = [this];
      for (const match of this._html.matchAll(/<\/?([\w-]+)([^>]*)>|([^<]+)/g)) {
        if (match[3]) {
          const text = new Element('#text');
          text.textContent = decode(match[3]);
          stack.at(-1).appendChild(text);
        } else if (match[0].startsWith('</')) {
          if (stack.length > 1) stack.pop();
        } else {
          const attrs = {};
          for (const attr of match[2].matchAll(/([\w-]+)(?:="([^"]*)"|'([^']*)')?/g)) attrs[attr[1]] = decode(attr[2] ?? attr[3] ?? '');
          const element = new Element(match[1], attrs);
          stack.at(-1).appendChild(element);
          if (!['input', 'img', 'br', 'hr', 'meta', 'link'].includes(match[1]) && !match[0].endsWith('/>')) stack.push(element);
        }
      }
    }
    matches(selector) {
      return selector.split(',').some(part => {
        part = part.trim();
        const attrs = [...part.matchAll(/\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g)];
        part = part.replace(/\[[^\]]+\]/g, '');
        const tag = part.match(/^[\w-]+/)?.[0];
        if (tag && this.tagName !== tag.toUpperCase()) return false;
        if ([...part.matchAll(/#([\w-]+)/g)].some(match => match[1] !== this.id)) return false;
        if ([...part.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
        return attrs.every(([, name, value]) => {
          const actual = name.startsWith('data-') ? this.dataset[this.dataName(name)] : this.attributes[name];
          return actual !== undefined && (value === undefined || actual === value);
        });
      });
    }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) {
      const alternatives = selector.split(',').map(part => part.trim().split(/\s+(?![^\[]*\])/).filter(part => part !== '>'));
      const found = [];
      const visit = parent => {
        for (const child of parent.children) {
          if (alternatives.some(parts => child.matches(parts.at(-1)) && (parts.length === 1 || child.parentElement?.closest(parts.slice(0, -1).join(' '))))) found.push(child);
          visit(child);
        }
      };
      visit(this);
      return found;
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    appendChild(child) { child.remove(); child.parentElement = this; this.children.push(child); return child; }
    append(...children) { children.forEach(child => this.appendChild(child)); }
    remove() { if (this.parentElement) this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; }
    contains(element) { return this === element || this.children.some(child => child.contains(element)); }
    setAttribute(name, value) { this.attributes[name] = String(value); if (name === 'class') this.className = String(value); if (name.startsWith('data-')) this.dataset[this.dataName(name)] = String(value); }
    getAttribute(name) { return this.attributes[name] ?? null; }
    focus() { document.activeElement = this; }
    click() { if (!this.disabled) return listeners.click({ target: this, preventDefault() {} }); }
  }
  const root = new Element('body');
  root.innerHTML = '<main id="reader" class="reader"></main><div id="overlay"></div><div id="toast"></div>';
  document.body = root;
  document.querySelector = selector => root.querySelector(selector);
  document.querySelectorAll = selector => root.querySelectorAll(selector);
  document.createElement = tag => new Element(tag);
  const context = vm.createContext({
    window: {
      MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'),
      MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'),
      MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'),
      MailBeaconReaderNav: require('../ui/reader-nav.js'),
      MailBeaconNotificationOpen: require('../ui/notification-open.js'),
      matchMedia: () => ({ matches: reducedMotion }),
      __TAURI__: { core: { invoke: async (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } }, event: { listen: async () => {} } },
    },
    document, console, Date: time.Date, setTimeout: time.setTimeout, clearTimeout: time.clearTimeout,
    notice: text => notices.push(text),
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(`${source.slice(0, source.indexOf('(async function boot()'))}
    renderSidebar = renderList = updateBadge = revealSelectedRow = function () {};
    loadList = async function () {};
    toast = notice;
    globalThis.api = { S, openMessage, openFromNotification, renderReader, renderReaderHint, showReaderHint, hideReaderHint, onKey, onMailEvent, go };
  `, context);
  const api = context.api;
  Object.assign(api.S, { accounts: [{ id: 'a', email: 'a@example.test' }], acct: 'a', folder: 'INBOX' });
  api.S.folders.a = [{ path: 'INBOX', role: 'inbox' }, { path: 'Archive', role: 'archive' }];
  api.S.lists.set('a|INBOX', [3, 2, 1].map(uid => ({ ...selection(uid), date: uid, subject: `Mail ${uid}`, fromAddr: 'sender@example.test', fromName: 'Sender', seen: true, flagged: false })));
  const $ = selector => document.querySelector(selector);
  return { ...api, $, document, time, calls, notices, hints: () => document.querySelectorAll('.reader-hint'),
    key: (key, extra = {}) => api.onKey({ key, target: $('#reader'), preventDefault() {}, ...extra }),
  };
}

const selection = (uid = 3) => ({ accountId: 'a', folder: 'INBOX', uid });
const fullMessage = () => ({ subject: 'Full message', from: { name: 'Sender', addr: 'sender@example.test' }, to: [], cc: [], attachments: [], text: 'Body', date: 1 });
function deferred() { let resolve; const promise = new Promise(yes => { resolve = yes; }); return { promise, resolve }; }
async function settle() { for (let i = 0; i < 10; i++) await Promise.resolve(); }

test('opening mail shows one hint outside the scroll area, even while downloading', () => {
  const mail = setup(async () => new Promise(() => {}));
  mail.openMessage(selection());
  assert.equal(mail.hints().length, 1);
  const hint = mail.hints()[0];
  assert.equal(hint.parentElement, mail.$('#reader'));
  assert.equal(hint.closest('.read-scroll'), null);
  assert.deepEqual(hint.querySelectorAll('kbd').map(key => key.textContent), ['Space', 'E', 'S', 'R']);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
});

test('hint remains for two seconds, then fades for 150 ms and is removed', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  mail.time.advance(1999);
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
  mail.time.advance(1);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), true);
  mail.time.advance(149);
  assert.equal(mail.hints().length, 1);
  mail.time.advance(1);
  assert.equal(mail.hints().length, 0);
  mail.renderReader();
  assert.equal(mail.hints().length, 0);
});

test('a full reader rebuild preserves the original hint deadline', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  const deadline = mail.S.readerHintUntil;
  mail.time.advance(1200);
  mail.S.lists.get('a|INBOX')[0].flagged = true;
  mail.renderReader();
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.S.readerHintUntil, deadline);
  mail.time.advance(800);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), true);
});

test('hiding a hint prevents partial, full and star redraws from resurrecting it', async () => {
  const body = deferred();
  const mail = setup(async cmd => cmd === 'get_message' ? body.promise : false);
  const opened = mail.openMessage(selection());
  mail.hideReaderHint();
  mail.time.advance(150);
  mail.onMailEvent({ type: 'messagePartial', ...selection(), view: { ...fullMessage(), partial: true } });
  assert.equal(mail.hints().length, 0);
  body.resolve(fullMessage());
  await opened;
  mail.S.lists.get('a|INBOX')[0].flagged = true;
  mail.renderReader();
  assert.equal(mail.hints().length, 0);
});

test('partial to complete mail retains the deadline and refreshes disabled reply', async () => {
  const body = deferred();
  const mail = setup(async cmd => cmd === 'get_message' ? body.promise : false);
  const opened = mail.openMessage(selection());
  const deadline = mail.S.readerHintUntil;
  mail.time.advance(700);
  mail.onMailEvent({ type: 'messagePartial', ...selection(), view: { ...fullMessage(), partial: true } });
  const replyHint = () => mail.hints()[0].querySelectorAll('kbd').find(key => key.textContent === 'R').parentElement;
  assert.equal(replyHint().classList.contains('is-disabled'), true);
  mail.time.advance(500);
  body.resolve(fullMessage());
  await opened;
  assert.equal(mail.S.readerHintUntil, deadline);
  assert.equal(replyHint().classList.contains('is-disabled'), false);
});

test('switching mail cancels old timers and a stale timer cannot hide the new hint', async () => {
  const mail = setup();
  await mail.openMessage(selection(3));
  const staleCallbacks = mail.time.callbacks();
  mail.time.advance(1000);
  await mail.openMessage(selection(2));
  const deadline = mail.S.readerHintUntil;
  for (const callback of staleCallbacks) callback();
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
  assert.equal(mail.S.readerHintUntil, deadline);
  mail.time.advance(1000);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
});

test('reopening the same mail resets the timer without stacking hints', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  mail.time.advance(1500);
  await mail.openMessage(selection());
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
  mail.time.advance(500);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
});

test('finishing an old download cannot revive a dismissed current hint', async () => {
  const old = deferred();
  const mail = setup(async (cmd, input) => cmd === 'get_message' && input.uid === 3 ? old.promise : fullMessage());
  const first = mail.openMessage(selection(3));
  await mail.openMessage(selection(2));
  mail.hideReaderHint({ immediate: true });
  old.resolve(fullMessage());
  await first;
  assert.equal(mail.S.sel.uid, 2);
  assert.equal(mail.hints().length, 0);
});

test('Escape and changing folders clear the hint immediately', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  mail.key('Escape');
  assert.equal(mail.S.sel, null);
  assert.equal(mail.hints().length, 0);
  await mail.openMessage(selection());
  mail.go('a', 'Archive');
  assert.equal(mail.hints().length, 0);
  mail.time.advance(3000);
  assert.equal(mail.hints().length, 0);
});

test('Escape closing a menu removes the hint without clearing selection', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  mail.$('#overlay').innerHTML = '<div class="menu"></div>';
  mail.key('Escape');
  assert.equal(mail.$('#overlay').innerHTML, '');
  assert.equal(mail.S.sel.uid, 3);
  assert.equal(mail.hints().length, 0);
  mail.renderReader();
  assert.equal(mail.hints().length, 0);
});

test('star shortcut dismisses this mail hint through its own reader redraw', async () => {
  const mail = setup(async cmd => cmd === 'get_message' ? fullMessage() : true);
  await mail.openMessage(selection());
  mail.key('s');
  await settle();
  assert.equal(mail.S.lists.get('a|INBOX')[0].flagged, true);
  mail.time.advance(150);
  assert.equal(mail.hints().length, 0);
  mail.renderReader();
  assert.equal(mail.hints().length, 0);
});

test('list navigation replaces the old hint and gives the next mail a fresh two seconds', async () => {
  const mail = setup();
  await mail.openMessage(selection(3));
  mail.time.advance(700);
  mail.key('j');
  await settle();
  assert.equal(mail.S.sel.uid, 2);
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
});

test('clearing selection or losing the selected message removes the hint', async () => {
  const mail = setup();
  await mail.openMessage(selection());
  const original = mail.S.lists.get('a|INBOX');
  mail.S.lists.set('a|INBOX', []);
  mail.renderReader();
  assert.equal(mail.hints().length, 0);
  assert.equal(mail.S.readerHintUntil, 0);
  mail.S.lists.set('a|INBOX', original);
  mail.renderReader();
  assert.equal(mail.hints().length, 0, 'returning list data must not revive an already cleared hint');
});

test('unavailable archive is omitted from the hint and its shortcut stays disabled', async () => {
  const mail = setup();
  mail.S.folders.a = [{ path: 'INBOX', role: 'inbox' }];
  await mail.openMessage(selection());
  const archive = mail.$('#reader [data-action="archive"]');
  assert.equal(archive.disabled, true);
  assert.deepEqual(mail.hints()[0].querySelectorAll('kbd').map(key => key.textContent), ['Space', 'S', 'R']);
  assert.doesNotMatch(mail.hints()[0].textContent, /不可归档/);
  mail.key('e');
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false, 'a disabled shortcut has no effect');
});

test('archive hint returns when the toolbar gains an archive destination without restarting its timer', async () => {
  const mail = setup();
  mail.S.folders.a = [{ path: 'INBOX', role: 'inbox' }];
  await mail.openMessage(selection());
  const until = mail.S.readerHintUntil;
  const body = mail.$('#reader .read-scroll');
  mail.time.advance(500);
  mail.S.folders.a.push({ path: 'Archive', role: 'archive' });
  mail.renderReader();
  assert.equal(mail.$('#reader [data-action="archive"]').disabled, false);
  assert.deepEqual(mail.hints()[0].querySelectorAll('kbd').map(key => key.textContent), ['Space', 'E', 'S', 'R']);
  assert.equal(mail.S.readerHintUntil, until);
  assert.equal(mail.$('#reader .read-scroll'), body, 'folder availability must not rebuild the body');
  mail.S.folders.a = [{ path: 'INBOX', role: 'inbox' }];
  mail.renderReader();
  assert.equal(mail.$('#reader [data-action="archive"]').disabled, true);
  assert.deepEqual(mail.hints()[0].querySelectorAll('kbd').map(key => key.textContent), ['Space', 'S', 'R']);
  assert.equal(mail.S.readerHintUntil, until);
});

test('an effective archive shortcut opens the next mail with its own new hint', async () => {
  const mail = setup(async cmd => cmd === 'get_message' ? fullMessage() : true);
  await mail.openMessage(selection(3));
  mail.time.advance(500);
  mail.key('e');
  await settle();
  assert.equal(mail.S.sel.uid, 2);
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), false);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
  mail.time.advance(150);
  assert.equal(mail.hints().length, 1);
});

test('notification opening uses the same one-hint lifecycle', async () => {
  const mail = setup();
  await mail.openFromNotification(selection(2));
  assert.equal(mail.S.sel.uid, 2);
  assert.equal(mail.hints().length, 1);
  mail.time.advance(2150);
  assert.equal(mail.hints().length, 0);
});

test('notification hint starts only after the selected message first reaches the reader', async () => {
  const mail = setup();
  const messages = mail.S.lists.get('a|INBOX');
  mail.S.lists.clear();
  await mail.openFromNotification(selection(2));
  assert.equal(mail.hints().length, 0);
  mail.time.advance(3000);
  mail.S.lists.set('a|INBOX', messages);
  mail.renderReader();
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
});

test('reader navigation helper is loaded before app.js in the real entry page', () => {
  const html = fs.readFileSync(require.resolve('../ui/index.html'), 'utf8');
  const helper = html.indexOf('src="reader-nav.js"');
  assert.ok(helper >= 0, 'reader helper must be loaded');
  assert.ok(helper < html.indexOf('src="app.js"'));
});

test('reader and hint enter once and resume elapsed motion across partial and full redraws', async () => {
  const body = deferred();
  const mail = setup(async cmd => cmd === 'get_message' ? body.promise : false);
  const opened = mail.openMessage(selection());
  assert.equal(mail.$('.read-scroll').classList.contains('is-entering'), true);
  assert.equal(mail.hints()[0].classList.contains('is-entering'), true);
  const deadline = mail.S.readerHintUntil;
  mail.time.advance(70);
  mail.onMailEvent({ type: 'messagePartial', ...selection(), view: { ...fullMessage(), partial: true } });
  assert.equal(mail.$('.read-scroll').style.animationDelay, '-70ms');
  assert.equal(mail.hints()[0].style.animationDelay, '-70ms');
  mail.time.advance(150);
  body.resolve(fullMessage());
  await opened;
  assert.equal(mail.$('.read-scroll').classList.contains('is-entering'), false);
  assert.equal(mail.hints()[0].classList.contains('is-entering'), false);
  assert.equal(mail.S.readerHintUntil, deadline);
  mail.S.lists.get('a|INBOX')[0].flagged = true;
  mail.renderReader();
  assert.equal(mail.$('.read-scroll').classList.contains('is-entering'), false);
});

test('rapidly switching mail starts a fresh entry and ignores older downloads', async () => {
  const old = deferred();
  const mail = setup(async (cmd, input) => cmd === 'get_message' && input.uid === 3 ? old.promise : fullMessage());
  const first = mail.openMessage(selection(3));
  mail.time.advance(80);
  await mail.openMessage(selection(2));
  mail.time.advance(80);
  await mail.openMessage(selection(1));
  assert.equal(mail.$('.read-scroll').style.animationDelay, '0ms');
  old.resolve(fullMessage());
  await first;
  assert.equal(mail.S.sel.uid, 1);
  assert.equal(mail.hints().length, 1);
  assert.equal(mail.$('.read-scroll').style.animationDelay, '0ms');
});

test('reduced motion presents the reader and hint immediately without entry effects', async () => {
  const mail = setup(undefined, { reducedMotion: true });
  await mail.openMessage(selection());
  assert.equal(mail.$('.read-scroll').classList.contains('is-entering'), false);
  assert.equal(mail.hints()[0].classList.contains('is-entering'), false);
  assert.equal(mail.S.readerHintUntil, mail.time.now() + 2000);
});

test('hiding during entry cancels its entry effect before the hint exits', () => {
  const mail = setup(async () => new Promise(() => {}));
  mail.openMessage(selection());
  mail.time.advance(40);
  mail.hideReaderHint();
  assert.equal(mail.hints()[0].classList.contains('is-entering'), false);
  assert.equal(mail.hints()[0].classList.contains('is-hiding'), true);
});
