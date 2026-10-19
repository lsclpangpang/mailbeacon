const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const signatures = require('../ui/compose-signature.js');

// A small DOM fixture: tests execute the application's event handlers and HTML,
// without a browser dependency or a second implementation of its draft logic.
const decode = text => text.replace(/&(amp|lt|gt|quot|#39);/g,
  (_, entity) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" })[entity]);

function setup(invoke = async () => {}) {
  const listeners = {}, calls = [], notices = [], quoteCalls = [];
  let confirmCalls = 0;
  const document = { activeElement: null, addEventListener: (name, handler) => { listeners[name] = handler; } };

  class Element {
    constructor(tagName, attributes = {}) {
      this.tagName = tagName.toUpperCase();
      this.attributes = attributes;
      this.children = [];
      this.parentElement = null;
      this.dataset = {};
      for (const [key, value] of Object.entries(attributes)) {
        if (key.startsWith('data-')) this.dataset[key.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value;
      }
      this.id = attributes.id || '';
      this.value = attributes.value || '';
      this.hidden = 'hidden' in attributes;
      this.disabled = 'disabled' in attributes;
      this.selectionStart = this.selectionEnd = 0;
      const classes = new Set((attributes.class || '').split(/\s+/).filter(Boolean));
      this.classList = {
        contains: name => classes.has(name),
        add: name => classes.add(name),
        remove: name => classes.delete(name),
        toggle: (name, yes) => (yes ?? !classes.has(name)) ? classes.add(name) : classes.delete(name),
      };
    }
    get isConnected() { return this === overlay || !!this.parentElement?.isConnected; }
    get textContent() { return this._text ?? this.children.map(child => child.textContent).join(''); }
    set textContent(value) { this._text = String(value); }
    get innerHTML() { return this._html || ''; }
    set innerHTML(html) {
      this._html = html;
      this._text = undefined;
      for (const child of this.children) child.parentElement = null;
      this.children = [];
      const stack = [this];
      for (const match of html.matchAll(/<\/?([\w-]+)([^>]*)>|([^<]+)/g)) {
        if (match[3]) {
          const text = new Element('#text');
          text.textContent = decode(match[3]);
          text.parentElement = stack.at(-1);
          stack.at(-1).children.push(text);
        } else if (match[0].startsWith('</')) {
          const closing = stack.pop();
          if (closing.tagName === 'TEXTAREA') closing.value = closing.textContent.replace(/\r\n?/g, '\n').replace(/^\n/, '');
          if (closing.tagName === 'SELECT') {
            const options = closing.querySelectorAll('option');
            closing.value = (options.find(option => 'selected' in option.attributes) || options[0])?.value || '';
          }
        } else {
          const attrs = {};
          for (const attr of match[2].matchAll(/([\w-]+)(?:="([^"]*)"|'([^']*)')?/g)) attrs[attr[1]] = decode(attr[2] ?? attr[3] ?? '');
          const element = new Element(match[1], attrs);
          element.parentElement = stack.at(-1);
          stack.at(-1).children.push(element);
          if (!['input', 'img', 'br', 'hr', 'meta', 'link'].includes(match[1]) && !match[0].endsWith('/>')) stack.push(element);
        }
      }
    }
    matches(selector) {
      return selector.split(',').some(part => {
        part = part.trim();
        const attributes = [...part.matchAll(/\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]/g)];
        part = part.replace(/\[[^\]]+\]/g, '');
        const tag = part.match(/^[\w-]+/)?.[0];
        if (tag && this.tagName !== tag.toUpperCase()) return false;
        if ([...part.matchAll(/#([\w-]+)/g)].some(match => match[1] !== this.id)) return false;
        if ([...part.matchAll(/\.([\w-]+)/g)].some(match => !this.classList.contains(match[1]))) return false;
        return attributes.every(([, key, value]) => {
          const actual = key.startsWith('data-')
            ? this.dataset[key.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase())]
            : key === 'hidden' ? (this.hidden ? '' : undefined) : this.attributes[key];
          return actual !== undefined && (value === undefined || actual === value);
        });
      });
    }
    closest(selector) { return this.matches(selector) ? this : this.parentElement?.closest(selector) || null; }
    querySelectorAll(selector) {
      const alternatives = selector.split(',').map(part => part.trim().split(/\s+(?![^\[]*\])/));
      const matches = [];
      const visit = parent => {
        for (const child of parent.children) {
          if (alternatives.some(parts => child.matches(parts.at(-1)) && (parts.length === 1 || child.parentElement?.closest(parts.slice(0, -1).join(' '))))) matches.push(child);
          visit(child);
        }
      };
      visit(this);
      return matches;
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
    contains(element) { return element === this || this.children.some(child => child.contains(element)); }
    focus() { document.activeElement = this; }
    setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    removeAttribute(name) { delete this.attributes[name]; }
    remove() { this.parentElement.children = this.parentElement.children.filter(child => child !== this); this.parentElement = null; }
    click() {
      if (this.disabled) return;
      return listeners.click({ target: this, preventDefault() {} });
    }
  }

  const overlay = new Element('div', { id: 'overlay' });
  document.querySelector = selector => selector === '#overlay' ? overlay : overlay.querySelector(selector);
  document.querySelectorAll = selector => overlay.querySelectorAll(selector);
  // The quote helper is an integration boundary stub, not a sanitizer. Its own
  // tests cover MIME/HTML composition; these cases verify the real UI passes the
  // original quote through unchanged and sends the helper's complete result.
  const quoteHelper = {
    createQuote(input) {
      quoteCalls.push({ method: 'createQuote', input });
      return input ? { ...input, html: input.html || '', rich: !!input.html } : null;
    },
    composeBody(body, quote) {
      const result = quote ? {
        body: `${body}\n\n${quote.header}\n${quote.text}`,
        htmlBody: `<html data-fixture="composed"><body><div>${body}</div>${quote.html || `<pre>${quote.text}</pre>`}</body></html>`,
      } : { body, htmlBody: null };
      quoteCalls.push({ method: 'composeBody', body, quote, result });
      return result;
    },
    previewDocument(quote, options) {
      quoteCalls.push({ method: 'previewDocument', quote, options });
      return `<html><body>${quote.html || quote.text}</body></html>`;
    },
  };
  const context = vm.createContext({
    window: { MailPulseSignature: signatures, MailPulseQuote: quoteHelper, __TAURI__: {
      core: { invoke: (cmd, input) => { calls.push({ cmd, input }); return invoke(cmd, input); } },
      event: { listen: async () => {} },
    } },
    document, console, setTimeout, clearTimeout,
    confirm: () => { confirmCalls++; return false; },
    notice: value => notices.push(value),
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = notice;
    globalThis.api = { S, openCompose, sendCompose, draftFor };
  `, context);
  const { S, openCompose, draftFor } = context.api;
  S.accounts = [
    { id: 'a', name: 'Alice', email: 'a@example.com', signature: 'Alice 的签名' },
    { id: 'b', name: 'Bob', email: 'b@example.com', signature: 'Bob 的签名' },
  ];
  const $ = selector => document.querySelector(selector);
  const click = (action, index = 0) => {
    const button = document.querySelectorAll(`[data-action="${action}"]`)[index];
    assert.ok(button, `missing ${action} button ${index}`);
    return button.click();
  };
  const key = (value, extra = {}) => listeners.keydown({
    target: document.activeElement || overlay, key: value, preventDefault() {}, ...extra,
  });
  const values = () => Object.fromEntries(['from', 'to', 'cc', 'bcc', 'subject', 'body'].map(field => [field, $(`#c-${field}`)?.value]));
  const change = selector => listeners.change({ target: $(selector) });
  return { S, open: openCompose, draftFor, $, click, key, change, values, overlay, calls, notices, quoteCalls, document,
    get confirmCalls() { return confirmCalls; } };
}

const reply = { accountId: 'a', to: 'Sender <sender@example.com>', cc: 'cc@example.com', subject: 'Re: 会议安排', body: '\n\n原邮件引用\n> 时间：10:30', inReplyTo: '<original@example.com>', references: ['<thread@example.com>'] };
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('Cancel and X directly close an untouched reply with prefilled recipients, quotation and signature', async () => {
  for (const button of [0, 1]) {
    const app = setup();
    app.open(reply);
    assert.match(app.$('#c-body').value, /Alice 的签名/);
    assert.match(app.$('#c-to').value, /sender@example.com/);
    await app.click('compose-close', button);
    assert.equal(app.$('#c-body'), null, `close button ${button} should dismiss an untouched reply`);
    assert.equal(app.confirmCalls, 0, 'native window.confirm is unavailable in the installed WKWebView');
  }
});

test('a new message or untouched forward with an automatic signature also closes directly', async () => {
  for (const draft of [{}, { subject: 'Fwd: 会议安排', body: '\n\n---------- 转发的邮件 ----------\n引用正文' }]) {
    const app = setup();
    app.open(draft);
    await app.click('compose-close');
    assert.equal(app.$('#c-body'), null);
    assert.equal(app.confirmCalls, 0);
  }
});

test('editing any draft field requests discard confirmation while leaving the original draft intact', async () => {
  for (const field of ['from', 'to', 'cc', 'bcc', 'subject', 'body']) {
    const app = setup();
    app.open(reply);
    app.$(`#c-${field}`).value = field === 'from' ? 'b' : `changed ${field}`;
    const before = app.values();
    await app.click('compose-close');
    const confirmation = app.$('.compose-confirm');
    assert.ok(confirmation, `${field} change needs an in-app confirmation`);
    assert.equal(confirmation.hidden, false);
    assert.equal(confirmation.dataset.kind, 'discard');
    assert.deepEqual(app.values(), before);
    assert.equal(app.confirmCalls, 0);
  }
});

test('restoring a changed field to its initial value makes the reply clean again', async () => {
  const app = setup();
  app.open(reply);
  const body = app.$('#c-body'), original = body.value;
  body.value += '\n临时输入';
  body.value = original;
  await app.click('compose-close');
  assert.equal(app.$('#c-body'), null);
});

test('continue editing preserves all fields, while explicit discard closes the draft', async () => {
  const app = setup();
  app.open(reply);
  app.$('#c-body').value += '\n请确认';
  const before = app.values();
  await app.click('compose-close');
  await app.click('compose-continue');
  assert.equal(app.$('.compose-confirm').hidden, true);
  assert.deepEqual(app.values(), before);
  await app.click('compose-close', 1);
  await app.click('compose-discard');
  assert.equal(app.$('#c-body'), null);
  assert.equal(app.calls.length, 0, 'closing must never send a message');
});

test('Escape dismisses a visible confirmation, then follows the same close flow on the next press', () => {
  const app = setup();
  app.open(reply);
  app.$('#c-body').value += '\n未完成回复';
  const before = app.values();
  app.key('Escape');
  assert.equal(app.$('.compose-confirm')?.hidden, false);
  app.key('Escape');
  assert.equal(app.$('.compose-confirm').hidden, true);
  assert.deepEqual(app.values(), before);
  app.key('Escape');
  assert.equal(app.$('.compose-confirm').hidden, false);
  assert.equal(app.calls.length, 0);
});

test('opening compose again, including Command-N, never overwrites an existing draft', () => {
  const app = setup();
  app.open(reply);
  app.$('#c-body').value += '\n我的回复';
  const sheet = app.$('.sheet'), before = app.values();
  app.open({ to: 'other@example.com', subject: 'replacement' });
  assert.equal(app.$('.sheet'), sheet);
  assert.deepEqual(app.values(), before);
  app.key('n', { metaKey: true });
  assert.equal(app.$('.sheet'), sheet);
  assert.deepEqual(app.values(), before);
});

test('sending locks editing and closing, prevents duplicate sends, and closes after success', async () => {
  const request = deferred(), app = setup(() => request.promise);
  app.open(reply);
  const pending = app.click('send'), sheet = app.$('.sheet');
  assert.equal(sheet.dataset.busy, 'true');
  for (const field of ['from', 'to', 'cc', 'bcc', 'subject', 'body']) assert.equal(app.$(`#c-${field}`).disabled, true, `${field} must be locked`);
  await app.click('compose-close');
  await app.click('compose-close', 1);
  app.key('Escape');
  app.key('n', { metaKey: true });
  await app.click('send');
  assert.equal(app.$('.sheet'), sheet);
  assert.equal(app.calls.filter(call => call.cmd === 'send_mail').length, 1);
  request.resolve();
  await pending;
  assert.equal(app.$('#c-body'), null);
});

test('send failure restores editable controls and keeps the draft so it can be cancelled', async () => {
  const request = deferred(), app = setup(() => request.promise);
  app.open(reply);
  app.$('#c-body').value += '\n需要保留';
  const before = app.values(), pending = app.click('send');
  request.reject(new Error('SMTP unavailable'));
  await pending;
  assert.deepEqual(app.values(), before);
  assert.notEqual(app.$('.sheet').dataset.busy, 'true');
  for (const field of ['to', 'cc', 'bcc', 'subject', 'body']) assert.equal(app.$(`#c-${field}`).disabled, false);
  assert.equal(app.$('[data-action="send"]').disabled, false);
  assert.ok(app.notices.some(value => String(value).includes('SMTP unavailable')));
  await app.click('compose-close');
  await app.click('compose-discard');
  assert.equal(app.$('#c-body'), null);
});

test('a completed send cannot dismiss a different overlay opened after its sheet detached', async () => {
  const request = deferred(), app = setup(() => request.promise);
  app.open(reply);
  const pending = app.click('send');
  app.overlay.innerHTML = '<div class="sheet" id="later-overlay">Other settings</div>';
  request.resolve();
  await pending;
  assert.ok(app.$('#later-overlay'), 'old send completion must only close its own compose sheet');
});

test('an empty subject uses in-app confirmation and only explicit confirmation sends', async () => {
  const app = setup();
  app.open({ to: 'recipient@example.com', body: 'hello' });
  await app.click('send');
  assert.equal(app.calls.length, 0);
  assert.equal(app.confirmCalls, 0);
  assert.equal(app.$('.compose-confirm')?.hidden, false);
  assert.equal(app.$('.compose-confirm').dataset.kind, 'send-without-subject');
  await app.click('compose-continue');
  assert.equal(app.$('.compose-confirm').hidden, true);
  assert.equal(app.calls.length, 0);
  await app.click('send');
  await app.click('compose-send-confirm');
  assert.equal(app.calls.filter(call => call.cmd === 'send_mail').length, 1);
  assert.equal(app.$('#c-body'), null);
});

const originalMail = {
  subject: '原始报价表', date: 1792369800000, uidValidity: 20261019,
  from: { name: 'Supplier', addr: 'supplier@example.com' },
  to: [{ name: 'Alice', addr: 'a@example.com' }, { name: 'Colleague', addr: 'cc@example.com' }],
  cc: [{ name: 'Team', addr: 'team@example.com' }],
  text: '型号\t报价\n测试产品\t198 元\n请查收图示。',
  html: '<table class="quote-table"><tr><th>型号</th><th>报价</th></tr><tr><td>测试产品</td><td>198 元</td></tr></table><img src="data:image/png;base64,PREVIEW" alt="产品图">',
  replyHtml: '<table class="quote-table"><tr><th>型号</th><th>报价</th></tr><tr><td>测试产品</td><td>198 元</td></tr></table><img src="cid:product-image" alt="产品图">',
  messageId: '<quote@example.com>', references: ['<earlier@example.com>'], attachments: [],
};

function selectOriginal(app, message = originalMail) {
  const source = { accountId: 'a', folder: 'Projects|2026', uid: 17 };
  app.S.sel = { ...source };
  app.S.msg = { ...message };
  app.S.lists.set('a|Projects|2026', [{ ...source, subject: message.subject, answered: false }]);
  return source;
}

test('reply, reply-all and forward capture HTML and text as a separate quote instead of editable reply text', () => {
  for (const mode of ['reply', 'reply-all', 'forward']) {
    const app = setup(), source = selectOriginal(app);
    const draft = app.draftFor(mode);
    assert.equal(draft.body, '', `${mode} must leave the new reply area empty`);
    assert.equal(draft.quote.html, originalMail.replyHtml, 'sending must retain CID references instead of preview data URLs');
    assert.equal(draft.quote.text, originalMail.text);
    assert.match(draft.quote.header, /Supplier/);
    assert.equal(draft.quotePreviewHtml, originalMail.html);
    assert.deepEqual(JSON.parse(JSON.stringify(draft.quoteSource)), { ...source, uidValidity: originalMail.uidValidity });
    app.open(draft);
    assert.match(app.$('#c-body').value, /Alice 的签名/);
    assert.doesNotMatch(app.$('#c-body').value, /测试产品|quote-table|请查收|原始报价表/);
    const preview = app.$('iframe');
    assert.ok(preview, 'HTML quote needs an isolated preview iframe');
    const permissions = String(preview.attributes.sandbox ?? preview.sandbox).split(/\s+/).filter(Boolean);
    assert.deepEqual(permissions, ['allow-same-origin'], 'only same-origin DOM access is allowed for sizing and link interception');
    assert.equal(permissions.includes('allow-scripts'), false);
    assert.equal(permissions.includes('allow-popups'), false);
    const captured = app.quoteCalls.find(call => call.method === 'createQuote');
    assert.ok(captured, 'quote content must pass through the quote helper');
    assert.equal(captured.input.html, originalMail.replyHtml);
  }
});

test('reply sends the complete helper output and retains its original source after selection and sender change', async () => {
  const app = setup(), source = selectOriginal(app);
  app.open(app.draftFor('reply'));
  app.$('#c-body').value = `报价已收到，请按此办理。${app.$('#c-body').value}`;
  app.$('#c-from').value = 'b';
  app.change('#c-from');
  app.S.sel.accountId = 'b';
  app.S.sel.folder = 'INBOX';
  app.S.sel.uid = 999;
  app.S.msg = { ...originalMail, html: '<p>另一封邮件</p>', text: '另一封邮件' };
  const editor = app.$('#c-body').value;
  await app.click('send');
  const composition = app.quoteCalls.find(call => call.method === 'composeBody');
  assert.ok(composition, 'sending must compose editable text together with the preserved quote');
  assert.equal(composition.body, editor);
  assert.equal(composition.quote.html, originalMail.replyHtml);
  assert.equal(composition.quote.text, originalMail.text);
  const { draft } = app.calls.find(call => call.cmd === 'send_mail').input;
  assert.equal(draft.accountId, 'b');
  assert.equal(draft.body, composition.result.body);
  assert.equal(draft.htmlBody, composition.result.htmlBody);
  assert.match(draft.body, /报价已收到/);
  assert.match(draft.body, /型号\t报价/);
  assert.match(draft.htmlBody, /<table/);
  assert.match(draft.htmlBody, /cid:product-image/);
  assert.doesNotMatch(draft.body + draft.htmlBody, /另一封邮件/);
  assert.deepEqual(JSON.parse(JSON.stringify(draft.quoteSource)), { ...source, uidValidity: originalMail.uidValidity });
  assert.equal(draft.inReplyTo, originalMail.messageId);
  assert.deepEqual(JSON.parse(JSON.stringify(draft.references)), originalMail.references);
  assert.equal(app.S.lists.get('a|Projects|2026')[0].answered, true, 'answered state belongs to the original quoted message');
});

test('plain-text originals remain separate visible quotations and are still included in the outgoing message', async () => {
  const app = setup();
  selectOriginal(app, { ...originalMail, html: '', replyHtml: '' });
  const draft = app.draftFor('reply');
  assert.equal(draft.body, '');
  assert.equal(draft.quote.text, originalMail.text);
  app.open(draft);
  assert.doesNotMatch(app.$('#c-body').value, /测试产品|型号/);
  assert.equal(app.$('iframe'), null);
  assert.match(app.$('pre')?.textContent || '', /型号\t报价/);
  app.$('#c-body').value += '\n谢谢';
  await app.click('send');
  const outgoing = app.calls.find(call => call.cmd === 'send_mail').input.draft;
  assert.match(outgoing.body, /谢谢/);
  assert.match(outgoing.body, /型号\t报价/);
});

test('an untouched reply with an isolated HTML quote closes directly through both close buttons', async () => {
  for (const button of [0, 1]) {
    const app = setup();
    selectOriginal(app);
    app.open(app.draftFor('reply'));
    assert.ok(app.$('iframe'), 'rich quote should be visible before closing');
    await app.click('compose-close', button);
    assert.ok(!app.$('#c-body'));
    assert.equal(app.calls.length, 0);
    assert.equal(app.confirmCalls, 0);
  }
});
