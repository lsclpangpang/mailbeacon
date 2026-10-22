const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const draftHelper = require('../ui/compose-draft.js');
const editorHelper = require('../ui/compose-editor.js');
const recipientsHelper = require('../ui/recipients.js');

// A small DOM fixture: tests execute the application's event handlers and HTML,
// without a browser dependency or a second implementation of its draft logic.
const decode = text => text.replace(/&(amp|lt|gt|quot|#39);/g,
  (_, entity) => ({ amp: '&', lt: '<', gt: '>', quot: '"', '#39': "'" })[entity]);

const bufferedRecord = (extra = {}) => ({ id: 'buffer-1', accountId: 'a', status: 'buffered', version: 1, createdAt: 1000, updatedAt: 1000, deadline: 11000, acceptedAt: null, copyStatus: 'pending', subject: 'Re: 会议安排', recipients: ['sender@example.com'], ...extra });

function setup(invoke = async (cmd) => cmd === 'enqueue_send' ? bufferedRecord() : undefined, options = {}) {
  const listeners = {}, calls = [], notices = [], quoteCalls = [];
  let confirmCalls = 0;
  const commands = [];
  const document = {
    activeElement: null,
    addEventListener: (name, handler) => { listeners[name] = handler; },
    // 记录编辑器命令；真正的排版由浏览器完成，这里只验证界面发出的命令
    execCommand: (command, _ui, value) => { commands.push([command, value ?? null]); return true; },
  };

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
      this.checked = 'checked' in attributes;
      this.selectionStart = this.selectionEnd = 0;
      const classes = new Set((attributes.class || '').split(/\s+/).filter(Boolean));
      this.classList = {
        contains: name => classes.has(name),
        add: name => classes.add(name),
        remove: name => classes.delete(name),
        toggle: (name, yes) => (yes ?? !classes.has(name)) ? classes.add(name) : classes.delete(name),
      };
    }
    get isConnected() { return this === overlay || this === host || this === dock || !!this.parentElement?.isConnected; }
    get childElementCount() { return this.children.length; }
    appendChild(child) {
      if (child.parentElement) child.parentElement.children = child.parentElement.children.filter(item => item !== child);
      child.parentElement = this; this.children.push(child); return child;
    }
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
  const host = options.sessions ? new Element('div', { id: 'compose-host', hidden: '' }) : null;
  const dock = options.sessions ? new Element('div', { id: 'compose-dock', hidden: '' }) : null;
  const roots = [overlay,host,dock].filter(Boolean);
  document.querySelector = selector => roots.find(el => el.matches(selector)) || roots.map(el => el.querySelector(selector)).find(Boolean) || null;
  document.querySelectorAll = selector => roots.flatMap(el => el.querySelectorAll(selector));
  // The quote helper is an integration boundary stub, not a sanitizer. Its own
  // tests cover MIME/HTML composition; these cases verify the real UI passes the
  // original quote through unchanged and sends the helper's complete result.
  const quoteHelper = {
    createQuote(input) {
      quoteCalls.push({ method: 'createQuote', input });
      return input ? { ...input, html: input.html || '', rich: !!input.html } : null;
    },
    composeBody(body, quote, replyHtml) {
      if (!quote) {
        const result = { body, htmlBody: replyHtml != null ? `<html data-fixture="composed"><body>${replyHtml}</body></html>` : null };
        quoteCalls.push({ method: 'composeBody', body, quote, result });
        return result;
      }
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
    window: { MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), MailPulseQuote: quoteHelper, MailPulseDraft: draftHelper, MailPulseEditor: editorHelper, MailBeaconRecipients: recipientsHelper, __TAURI__: {
      core: { invoke: async (cmd, input) => {
        calls.push({ cmd, input }); const result = await invoke(cmd, input);
        if (result !== undefined) return result;
        if (options.sessions && cmd === 'compose_recovery_save') return input.input;
        if (options.sessions && cmd === 'compose_recovery_list') return [];
        return result;
      } },
      event: { listen: async () => {} },
    } },
    document, console, setTimeout, clearTimeout,
    confirm: () => { confirmCalls++; return false; },
    notice: value => notices.push(value),
  });
  if (options.sessions) context.window.MailBeaconComposeSession = require('../ui/compose-session');
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = notice;
    globalThis.api = { S, openCompose, sendCompose, draftFor, onAttachmentsDropped, openContacts, guardComposeNavigation, setComposeMode, canSendCompose, restoreComposeRecovery };
  `, context);
  const { S, openCompose, draftFor, onAttachmentsDropped, openContacts } = context.api;
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
  // 正文是富文本编辑器：读成纯文本，写入时转成 HTML
  const body = () => editorHelper.htmlToText($('#c-body')?.innerHTML || '');
  const setBody = (text) => { $('#c-body').innerHTML = `<div>${editorHelper.textToHtml(text)}</div>`; };
  const type = (text) => { $('#c-body').innerHTML += `<div>${editorHelper.textToHtml(text.replace(/^\n/, ''))}</div>`; };
  const values = () => Object.fromEntries(['from', 'to', 'cc', 'bcc', 'subject', 'body'].map(field => [field, field === 'body' ? body() : $(`#c-${field}`)?.value]));
  const change = selector => listeners.change({ target: $(selector) });
  return { S, open: openCompose, draftFor, dropped: onAttachmentsDropped, openContacts, $, click, key, change, values, overlay, calls, notices, quoteCalls, document,
    body, setBody, type, commands, listeners, host, dock, api: context.api,
    get confirmCalls() { return confirmCalls; } };
}

const reply = { accountId: 'a', to: 'Sender <sender@example.com>', cc: 'cc@example.com', subject: 'Re: 会议安排', body: '\n\n原邮件引用\n> 时间：10:30', inReplyTo: '<original@example.com>', references: ['<thread@example.com>'] };
test('changing only the receipt option protects the modified draft on close', async () => {
  const app = setup();
  app.open(reply);
  app.$('#c-receipt').checked = true;
  app.change('#c-receipt');
  await app.click('compose-close');
  assert.equal(app.$('.compose-confirm')?.hidden, false, 'receipt-only changes must be considered dirty');
  await app.click('compose-discard');
});

test('collapse persists full immediate input and preserves the editor and attachment tokens', async () => {
  const app = setup(undefined, {sessions:true}); app.open({...reply,attachments:[{kind:'file',token:'one',name:'a.pdf',size:1}]});
  const body = app.$('#c-body'), sheet = app.$('.compose-sheet');
  app.type('最后输入'); app.$('#c-receipt').checked=true;app.$('#c-followup').checked=true;
  await app.click('compose-collapse');
  const write=app.calls.find(x=>x.cmd==='compose_recovery_save').input.input;
  assert.match(write.editor.html,/最后输入/);assert.equal(write.draft.requestReceipt,true);assert.equal(write.draft.followupSeconds,172800);
  assert.equal(sheet.dataset.mode,'collapsed');assert.equal(app.$('#c-body'),body);
  assert.equal(app.calls.some(x=>x.cmd==='release_attachments'),false);
  await app.click('compose-restore');assert.equal(sheet.dataset.mode,'expanded');assert.equal(app.$('#c-body'),body);
  await app.click('compose-close');await app.click('compose-discard');
});

test('failed local persistence keeps compose visible and never switches reader selection', async () => {
  const app=setup(async cmd=>{if(cmd==='compose_recovery_save')throw Error('disk full');},{sessions:true});
  app.open({...reply,mode:'inline',quoteSource:{accountId:'a',folder:'INBOX',uid:1}}); app.type('保留');
  app.S.sel={accountId:'a',folder:'INBOX',uid:1};
  await app.api.guardComposeNavigation(()=>{app.S.sel={uid:2};},{accountId:'a',folder:'INBOX',uid:2});
  assert.equal(app.S.sel.uid,1);assert.equal(app.$('.compose-sheet').dataset.mode,'inline');assert.match(app.body(),/保留/);
  await app.click('compose-close');await app.click('compose-discard');
});

test('collapsed editor cannot send from an unrelated modal and Escape leaves its session intact', async () => {
  const app=setup(undefined,{sessions:true});app.open(reply);await app.click('compose-collapse');
  app.overlay.innerHTML='<div class="scrim"><div class="sheet"><input id="search-modal"></div></div>';
  app.$('#search-modal').focus();await app.key('Enter',{metaKey:true});
  assert.equal(app.calls.some(x=>x.cmd==='enqueue_send'),false);
  await app.key('Escape');assert.ok(app.$('.compose-sheet'));assert.equal(app.$('.compose-sheet').dataset.mode,'collapsed');
  assert.equal(app.calls.some(x=>x.cmd==='release_attachments'),false);
  await app.click('compose-restore');await app.click('compose-close');
});

test('inline reply navigation saves first then collapses the original session before selecting B', async () => {
  const pending=deferred();
  const app=setup(async(cmd,input)=>{if(cmd==='compose_recovery_save'){await pending.promise;return input.input;}},{sessions:true});
  app.open({...reply,mode:'inline',quoteSource:{accountId:'a',folder:'INBOX',uid:1}});app.type('最新内容');app.S.sel={uid:1};
  const changing=app.api.guardComposeNavigation(()=>{app.S.sel={uid:2};},{accountId:'a',folder:'INBOX',uid:2});
  assert.equal(app.S.sel.uid,1);pending.resolve();await changing;
  assert.equal(app.S.sel.uid,2);assert.equal(app.$('.compose-sheet').dataset.mode,'collapsed');assert.match(app.dock.innerHTML,/回复/);
  await app.click('compose-restore');await app.click('compose-close');await app.click('compose-discard');
});
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('Cancel and X directly close an untouched reply with prefilled recipients, quotation and signature', async () => {
  for (const button of [0, 1]) {
    const app = setup();
    app.open(reply);
    assert.match(app.body(), /Alice 的签名/);
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
    if (field === 'body') app.setBody('changed body');
    else app.$(`#c-${field}`).value = field === 'from' ? 'b' : `changed ${field}`;
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
  const body = app.$('#c-body'), original = body.innerHTML;
  body.innerHTML += '<div>临时输入</div>';
  body.innerHTML = original;
  await app.click('compose-close');
  assert.equal(app.$('#c-body'), null);
});

test('continue editing preserves all fields, while explicit discard closes the draft', async () => {
  const app = setup();
  app.open(reply);
  app.type('\n请确认');
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
  app.type('\n未完成回复');
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
  app.type('\n我的回复');
  const sheet = app.$('.sheet'), before = app.values();
  app.open({ to: 'other@example.com', subject: 'replacement' });
  assert.equal(app.$('.sheet'), sheet);
  assert.deepEqual(app.values(), before);
  app.key('n', { metaKey: true });
  assert.equal(app.$('.sheet'), sheet);
  assert.deepEqual(app.values(), before);
});

test('sending locks editing and closing, prevents duplicate sends, and closes after success', { timeout: 1000 }, async () => {
  const request = deferred(), entered = deferred(), app = setup(cmd => { if (cmd === 'enqueue_send') { entered.resolve(); return request.promise; } });
  app.open(reply);
  const pending = app.click('send'), sheet = app.$('.sheet');
  assert.equal(sheet.dataset.busy, 'true');
  for (const field of ['from', 'to', 'cc', 'bcc', 'subject']) assert.equal(app.$(`#c-${field}`).disabled, true, `${field} must be locked`);
  assert.equal(app.$('#c-body').attributes.contenteditable, 'false', 'body must be locked');
  await app.click('compose-close');
  await app.click('compose-close', 1);
  app.key('Escape');
  app.key('n', { metaKey: true });
  await app.click('send');
  assert.equal(app.$('.sheet'), sheet);
  await entered.promise;
  assert.equal(app.calls.filter(call => call.cmd === 'enqueue_send').length, 1);
  request.resolve(bufferedRecord());
  await pending;
  assert.equal(app.$('#c-body'), null);
});

test('send failure restores editable controls and keeps the draft so it can be cancelled', async () => {
  const request = deferred(), app = setup(() => request.promise);
  app.open(reply);
  app.type('\n需要保留');
  const before = app.values(), pending = app.click('send');
  request.reject(new Error('SMTP unavailable'));
  await pending;
  assert.deepEqual(app.values(), before);
  assert.notEqual(app.$('.sheet').dataset.busy, 'true');
  for (const field of ['to', 'cc', 'bcc', 'subject']) assert.equal(app.$(`#c-${field}`).disabled, false);
  assert.equal(app.$('#c-body').attributes.contenteditable, 'true');
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
  request.resolve(bufferedRecord());
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
  assert.equal(app.calls.filter(call => call.cmd === 'enqueue_send').length, 1);
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
    assert.deepEqual(JSON.parse(JSON.stringify(draft.quoteSource)), { ...source, receiveEpoch: 1, uidValidity: originalMail.uidValidity });
    app.open(draft);
    assert.match(app.body(), /Alice 的签名/);
    assert.doesNotMatch(app.body(), /测试产品|quote-table|请查收|原始报价表/);
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
  app.$('#c-body').innerHTML = '<div>报价已收到，请按此办理。</div>' + app.$('#c-body').innerHTML;
  app.$('#c-from').value = 'b';
  app.change('#c-from');
  app.S.sel.accountId = 'b';
  app.S.sel.folder = 'INBOX';
  app.S.sel.uid = 999;
  app.S.msg = { ...originalMail, html: '<p>另一封邮件</p>', text: '另一封邮件' };
  const editor = app.body();
  await app.click('send');
  const composition = app.quoteCalls.find(call => call.method === 'composeBody');
  assert.ok(composition, 'sending must compose editable text together with the preserved quote');
  assert.equal(composition.body, editor);
  assert.equal(composition.quote.html, originalMail.replyHtml);
  assert.equal(composition.quote.text, originalMail.text);
  const { draft } = app.calls.find(call => call.cmd === 'enqueue_send').input;
  assert.equal(draft.accountId, 'b');
  assert.equal(draft.body, composition.result.body);
  assert.equal(draft.htmlBody, composition.result.htmlBody);
  assert.match(draft.body, /报价已收到/);
  assert.match(draft.body, /型号\t报价/);
  assert.match(draft.htmlBody, /<table/);
  assert.match(draft.htmlBody, /cid:product-image/);
  assert.doesNotMatch(draft.body + draft.htmlBody, /另一封邮件/);
  assert.deepEqual(JSON.parse(JSON.stringify(draft.quoteSource)), { ...source, receiveEpoch: 1, uidValidity: originalMail.uidValidity });
  assert.equal(draft.inReplyTo, originalMail.messageId);
  assert.deepEqual(JSON.parse(JSON.stringify(draft.references)), originalMail.references);
  assert.equal(app.S.lists.get('a|Projects|2026')[0].answered, false, 'entering the send buffer must not mark the original as answered before SMTP acceptance');
  assert.equal(app.calls.some(call => call.cmd === 'send_mail' || call.cmd === 'mark_answered'), false, 'compose must enqueue through the single buffered send entry');
});

test('plain-text originals remain separate visible quotations and are still included in the outgoing message', async () => {
  const app = setup();
  selectOriginal(app, { ...originalMail, html: '', replyHtml: '' });
  const draft = app.draftFor('reply');
  assert.equal(draft.body, '');
  assert.equal(draft.quote.text, originalMail.text);
  app.open(draft);
  assert.doesNotMatch(app.body(), /测试产品|型号/);
  assert.equal(app.$('iframe'), null);
  assert.match(app.$('pre')?.textContent || '', /型号\t报价/);
  app.type('\n谢谢');
  await app.click('send');
  const outgoing = app.calls.find(call => call.cmd === 'enqueue_send').input.draft;
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

const plain = (value) => JSON.parse(JSON.stringify(value));

test('closing a changed message offers to save it as a draft', async () => {
  const app = setup(async (cmd) => cmd === 'save_draft' ? { accountId: 'a', folder: 'Drafts', uid: 5, uidValidity: 1, messageId: '<d@x>' } : undefined);
  app.open({ to: 'x@example.com', subject: '计划' });
  app.setBody('写到一半');
  await app.click('compose-close');
  assert.equal(app.$('.compose-confirm').hidden, false);
  await app.click('compose-save-draft');
  const save = app.calls.find((c) => c.cmd === 'save_draft');
  assert.ok(save, '应保存草稿');
  assert.equal(save.input.draft.subject, '计划');
  assert.match(save.input.draft.body, /写到一半/);
  assert.equal(save.input.previous, null);
  assert.equal(app.$('#c-body'), null, '保存后关闭写信窗口');
  assert.ok(app.notices.some((n) => String(n).includes('草稿')));
});

test('a failed draft save keeps the compose sheet open', async () => {
  const app = setup(async (cmd) => { if (cmd === 'save_draft') throw new Error('offline'); });
  app.open({ to: 'x@example.com', subject: '计划' });
  app.setBody('重要内容');
  await app.click('compose-close');
  await app.click('compose-save-draft');
  assert.ok(app.$('#c-body'), '保存失败不能丢掉正在写的内容');
  assert.ok(app.notices.some((n) => String(n).includes('offline')));
});

test('discarding deletes a draft autosaved during this session', async () => {
  const ref = { accountId: 'a', folder: 'Drafts', uid: 8, uidValidity: 1, messageId: '<auto@x>' };
  const app = setup(async (cmd) => cmd === 'save_draft' ? ref : undefined);
  app.open({ to: 'x@example.com', subject: '临时' });
  app.setBody('自动保存过');
  app.$('.sheet')._autosave.change({ changed: true });
  await app.$('.sheet')._autosave.flush();
  await app.click('compose-close');
  await app.click('compose-discard');
  assert.equal(app.$('#c-body'), null);
  const del = app.calls.find((c) => c.cmd === 'delete_draft');
  assert.deepEqual(plain(del.input.draft), ref);
});

test('a reopened draft closes untouched without prompting, and discarding edits keeps it', async () => {
  const draftRef = { accountId: 'a', folder: 'Drafts', uid: 3, uidValidity: 2, messageId: '<old@x>' };
  for (const edit of [false, true]) {
    const app = setup();
    app.open({ to: 'x@example.com', subject: '旧草稿', body: '旧正文', draftRef });
    if (edit) app.type('改动');
    await app.click('compose-close');
    if (edit) await app.click('compose-discard');
    assert.equal(app.$('#c-body'), null);
    assert.equal(app.calls.filter((c) => c.cmd === 'delete_draft').length, 0, '不能删掉用户原来的草稿');
  }
});

test('a reopened draft keeps its original until the user saves or sends', async () => {
  const original = { accountId: 'a', folder: 'Drafts', uid: 3, uidValidity: 2, messageId: '<old@x>' };
  const copy = { accountId: 'a', folder: 'Drafts', uid: 9, uidValidity: 2, messageId: '<copy@x>' };
  // 自动保存后选「不保存」：删掉自动保存的副本，原稿保留
  let app = setup(async (cmd) => cmd === 'save_draft' ? copy : cmd === 'enqueue_send' ? bufferedRecord() : undefined);
  app.open({ to: 'x@example.com', subject: '旧草稿', body: '旧正文', draftRef: original });
  app.type('试改');
  app.$('.sheet')._autosave.change({ edited: 1 });
  await app.$('.sheet')._autosave.flush();
  const firstSave = app.calls.find((c) => c.cmd === 'save_draft');
  assert.equal(firstSave.input.previous, null, '自动保存不能替换掉原稿');
  await app.click('compose-close');
  await app.click('compose-discard');
  assert.deepEqual(app.calls.filter((c) => c.cmd === 'delete_draft').map((c) => plain(c.input.draft)), [copy]);
  // 选「保存草稿」：新版本存好后删掉原稿
  app = setup(async (cmd) => cmd === 'save_draft' ? copy : cmd === 'enqueue_send' ? bufferedRecord() : undefined);
  app.open({ to: 'x@example.com', subject: '旧草稿', body: '旧正文', draftRef: original });
  app.type('定稿');
  await app.click('compose-close');
  await app.click('compose-save-draft');
  assert.deepEqual(app.calls.filter((c) => c.cmd === 'delete_draft').map((c) => plain(c.input.draft)), [original]);
  // 发送：原稿和自动保存的副本都删除，并立即从列表移除
  app = setup(async (cmd) => cmd === 'save_draft' ? copy : cmd === 'enqueue_send' ? bufferedRecord() : undefined);
  app.S.lists.set('a|Drafts', [{ accountId: 'a', folder: 'Drafts', uid: 3, subject: '旧草稿' }, { accountId: 'a', folder: 'Drafts', uid: 9, subject: '副本' }]);
  app.open({ to: 'x@example.com', subject: '旧草稿', body: '旧正文', draftRef: original });
  app.$('.sheet')._autosave.change({ edited: 2 });
  await app.$('.sheet')._autosave.flush();
  await app.click('send');
  const send = app.calls.find((c) => c.cmd === 'enqueue_send');
  assert.deepEqual(plain(send.input.draftRefs), [copy, original]);
  assert.deepEqual(app.S.lists.get('a|Drafts'), []);
});

test('a reopened draft restores Bcc, keeps its signature once, and stays in the thread', async () => {
  const app = setup();
  app.open({
    to: 'x@example.com', bcc: 'boss@example.com', subject: 'Re: 报价', body: '正文\n\nAlice 的签名',
    draftRef: { accountId: 'a', folder: 'Drafts', uid: 3, uidValidity: 2, messageId: '<old@x>' },
    inReplyTo: '<orig@x>', references: ['<root@x>', '<orig@x>'],
  });
  assert.equal(app.$('#c-bcc').value, 'boss@example.com');
  assert.equal(app.$('.cc-row').hidden, false, '有密送时要显示抄送/密送栏');
  assert.equal(app.body().match(/Alice 的签名/g).length, 1, '不能再插一次签名');
  await app.click('send');
  const { draft } = app.calls.find((c) => c.cmd === 'enqueue_send').input;
  assert.equal(draft.bcc, 'boss@example.com');
  assert.equal(draft.inReplyTo, '<orig@x>');
  assert.deepEqual(plain(draft.references), ['<root@x>', '<orig@x>']);
});

test('sending an old endpoint draft does not hide a new endpoint message with the same UID', async () => {
  const app = setup();
  app.S.accounts.find(a => a.id === 'a').receiveEpoch = 2;
  const current = { accountId: 'a', receiveEpoch: 2, folder: 'Drafts', uid: 3, subject: '新服务器草稿' };
  app.S.lists.set('a|Drafts', [current]);
  const original = { accountId: 'a', receiveEpoch: 1, folder: 'Drafts', uid: 3, uidValidity: 7, messageId: '<old@x>' };
  app.open({ to: 'x@example.com', subject: '旧服务器草稿', body: '旧正文', draftRef: original });
  await app.click('send');
  assert.deepEqual(plain(app.calls.find(c => c.cmd === 'enqueue_send').input.draftRefs), [original]);
  assert.deepEqual(plain(app.S.lists.get('a|Drafts')), [current]);
});

test('draft cleanup preserves a reused UID after the same endpoint rebuilds its mailbox', async () => {
  for (const [local, currentValidity] of [[false, 8], [false, 7], [true, 8]]) {
    const app = setup();
    const folder = local ? '__local_drafts' : 'Drafts';
    const current = { accountId: 'a', receiveEpoch: 1, folder, uid: 3, uidValidity: currentValidity, subject: '当前草稿' };
    const original = { accountId: 'a', receiveEpoch: 1, folder, uid: 3, uidValidity: 7, messageId: '<old@x>' };
    app.S.lists.set(`a|${folder}`, [current]);
    app.S.results.set('search', { items: [current] });
    app.S.thread = { items: [current] };
    app.open({ to: 'x@example.com', subject: '原草稿', body: '正文', draftRef: original });
    await app.click('send');
    const expected = local || currentValidity === 7 ? [] : [current];
    assert.deepEqual(plain(app.S.lists.get(`a|${folder}`)), expected);
    assert.deepEqual(plain(app.S.results.get('search').items), expected);
    assert.deepEqual(plain(app.S.thread.items), expected);
  }
});

test('draft cleanup does not merge references across receive identities', async () => {
  const original = { accountId: 'a', receiveEpoch: 1, folder: 'Drafts', uid: 3, uidValidity: 7, messageId: '<same@x>' };
  const copy = { ...original, receiveEpoch: 2 };
  const app = setup(async cmd => cmd === 'save_draft' ? copy : undefined);
  app.S.accounts.find(a => a.id === 'a').receiveEpoch = 2;
  app.open({ to: 'x@example.com', subject: '草稿', body: '正文', draftRef: original });
  app.$('.sheet')._autosave.change({ edited: 2 });
  await app.$('.sheet')._autosave.flush();
  await app.click('send');
  assert.deepEqual(plain(app.calls.find(c => c.cmd === 'enqueue_send').input.draftRefs), [copy, original]);
});

test('continue editing loads the draft attachments as tokens independent of the old version', async () => {
  const picked = [{ token: 'mem1', name: '合同.pdf', size: 4 }];
  const app = setup(async (cmd) => cmd === 'draft_attachments' ? picked : undefined);
  const source = { accountId: 'a', receiveEpoch: 3, folder: 'Drafts', uid: 11 };
  app.S.accounts.find(a => a.id === 'a').receiveEpoch = 3;
  app.S.sel = { ...source };
  // 第 0 个部件是回复引用里的内嵌图片，邮件视图不把它列为附件
  app.S.msg = { subject: '草稿', text: '正文', to: [{ name: '', addr: 'x@example.com' }], cc: [], bcc: [], attachments: [{ index: 1, name: '合同.pdf', size: 4 }], messageId: 'd@x', uidValidity: 5, references: [] };
  app.S.lists.set('a|Drafts', [{ ...source, subject: '草稿' }]);
  app.S.folders.a = [{ path: 'Drafts', name: '草稿箱', role: 'drafts' }];
  // 阅读区在测试里不渲染；用同样的 data-action 按钮走真实的点击处理。
  app.overlay.innerHTML = '<button data-action="edit-draft">继续编辑</button>';
  await app.click('edit-draft');
  const request = app.calls.find((c) => c.cmd === 'draft_attachments');
  assert.deepEqual(plain(request.input), { accountId: 'a', receiveEpoch: 3, folder: 'Drafts', uid: 11, uidValidity: 5, indices: [1] }, '只取出视图里列出的附件，不包括内嵌图片');
  await app.click('send');
  const send = app.calls.find((c) => c.cmd === 'enqueue_send');
  assert.deepEqual(plain(send.input.draft.attachments), [{ kind: 'file', token: 'mem1' }]);
  assert.equal(send.input.draftRefs[0].receiveEpoch, 3);
});

test('attachments can be removed and dropped files are added to the open message', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '拖拽' });
  app.dropped([{ token: 'a1', name: 'a.png', size: 10 }, { token: 'a2', name: 'b.zip', size: 20 }]);
  assert.match(app.$('.compose-atts').textContent, /a\.png/);
  await app.click('remove-attachment', 0);
  assert.doesNotMatch(app.$('.compose-atts').textContent, /a\.png/);
  await app.click('send');
  const send = app.calls.find((c) => c.cmd === 'enqueue_send');
  assert.deepEqual(plain(send.input.draft.attachments), [{ kind: 'file', token: 'a2' }]);
});

test('adding an attachment counts as an unsaved change', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: 's' });
  app.dropped([{ token: 'a1', name: 'a.png', size: 10 }]);
  await app.click('compose-close');
  assert.equal(app.$('.compose-confirm').hidden, false);
});

test('attachments over the size limit block sending', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '大文件' });
  app.dropped([{ token: 'big', name: 'big.mov', size: 60 * 1024 * 1024 }]);
  await app.click('send');
  assert.equal(app.calls.filter((c) => c.cmd === 'enqueue_send').length, 0);
  assert.ok(app.notices.some((n) => String(n).includes('超过')));
});

test('forwarding keeps the original attachments instead of a warning', async () => {
  const app = setup();
  const source = selectOriginal(app, { ...originalMail, attachments: [{ index: 0, name: '合同.pdf', size: 99 }] });
  const draft = app.draftFor('forward');
  assert.ok(!draft.note, '不再提示「转发暂不带原附件」');
  app.open(draft);
  assert.match(app.$('.compose-atts').textContent, /合同\.pdf/);
  app.$('#c-to').value = 'boss@example.com';
  await app.click('send');
  const send = app.calls.find((c) => c.cmd === 'enqueue_send');
  assert.deepEqual(plain(send.input.draft.attachments), [{ kind: 'message', ...source, receiveEpoch: 1, uidValidity: originalMail.uidValidity, index: 0 }]);
});

test('closing a compose sheet or removing a chip releases attachment tokens', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: 's', attachments: [{ kind: 'file', token: 'm1', name: 'a.pdf', size: 1 }, { kind: 'file', token: 'm2', name: 'b.pdf', size: 1 }],
    draftRef: { accountId: 'a', folder: 'Drafts', uid: 3, uidValidity: 2, messageId: '<old@x>' } });
  await app.click('remove-attachment', 0);
  await app.click('compose-close');
  await app.click('compose-discard');
  const released = app.calls.filter((c) => c.cmd === 'release_attachments').flatMap((c) => plain(c.input.tokens));
  assert.deepEqual(released.sort(), ['m1', 'm2']);
  const untouched = setup();
  untouched.open({ to: 'x@example.com', subject: 's', attachments: [{ kind: 'file', token: 'm3', name: 'c.pdf', size: 1 }],
    draftRef: { accountId: 'a', folder: 'Drafts', uid: 3, uidValidity: 2, messageId: '<old@x>' } });
  await untouched.click('compose-close');
  assert.equal(untouched.$('#c-body'), null);
  assert.deepEqual(untouched.calls.filter((c) => c.cmd === 'release_attachments').flatMap((c) => plain(c.input.tokens)), ['m3'], '没改动直接关闭也要释放');
});

const sentDraft = (app) => app.calls.find((c) => c.cmd === 'enqueue_send').input.draft;
const fmt = (app, cmd) => {
  app.overlay.querySelector(`[data-action="fmt"][data-cmd="${cmd}"]`).click();
};

test('the body is a rich editor and sending carries both HTML and plain text', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '格式' });
  const editor = app.$('#c-body');
  assert.equal(editor.attributes.contenteditable, 'true');
  assert.match(editor.innerHTML, /mp-signature[^>]*>Alice 的签名/);
  editor.innerHTML = '<div><b>重点</b>：请看<a href="https://e.com">链接</a></div>' + editor.innerHTML;
  await app.click('send');
  const draft = sentDraft(app);
  assert.match(draft.htmlBody, /<b>重点<\/b>/);
  assert.match(draft.body, /重点：请看链接 <https:\/\/e.com>/);
  assert.match(draft.body, /Alice 的签名/);
  assert.equal(draft.html, undefined, '不把编辑器内部字段发给后端');
});

test('toolbar buttons and pickers issue editing commands', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '格式' });
  fmt(app, 'bold');
  fmt(app, 'ul');
  fmt(app, 'align-center');
  const size = app.$('[data-fmt="size"]');
  size.value = 'large';
  app.listeners.change({ target: size });
  const color = app.$('[data-fmt="color"]');
  color.value = '#c00000';
  app.listeners.change({ target: color });
  const used = app.commands.filter(([c]) => c !== 'styleWithCSS');
  assert.deepEqual(used, [['bold', null], ['insertUnorderedList', null], ['justifyCenter', null], ['fontSize', '5'], ['foreColor', '#c00000']]);
  await app.click('compose-close');
  assert.equal(app.$('#c-body'), null, '只发命令、内容没变时不算修改，直接关闭');
});

test('links are inserted through an in-app field and unsafe links are refused', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '链接' });
  fmt(app, 'link');
  const input = app.$('#c-link');
  assert.ok(input, '点链接按钮后出现输入框');
  input.value = 'javascript:alert(1)';
  await app.click('popover-ok');
  assert.equal(app.commands.some(([c]) => c === 'createLink'), false);
  assert.ok(app.notices.some((n) => String(n).includes('链接')));
  input.value = 'example.com/report';
  await app.click('popover-ok');
  assert.deepEqual(app.commands.find(([c]) => c === 'createLink'), ['createLink', 'https://example.com/report']);
  assert.equal(app.$('#c-link'), null, '插入后输入框关闭');
});

test('clicking a hyperlink in the compose body opens it externally', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '链接' });
  for (const url of ['https://example.com/report', 'mailto:person@example.com', 'tel:+8613812345678']) {
    app.$('#c-body').innerHTML = `<div><a href="${url}">链接</a></div>`;
    const link = app.$('#c-body a[href]');
    link.href = url;
    const target = link.children[0];
    let prevented = 0;
    await app.listeners.click({ target, preventDefault() { prevented++; } });
    assert.equal(prevented, 1);
    assert.deepEqual(plain(app.calls.at(-1)?.input), { url });
  }
});

test('tables and images are inserted as HTML', async () => {
  const app = setup(async (cmd) => cmd === 'pick_inline_image' ? 'data:image/png;base64,iVBORw==' : undefined);
  app.open({ to: 'x@example.com', subject: '表格' });
  fmt(app, 'table');
  app.$('#c-rows').value = '2';
  app.$('#c-cols').value = '3';
  await app.click('popover-ok');
  const table = app.commands.find(([c, v]) => c === 'insertHTML' && v.includes('<table'));
  assert.ok(table);
  assert.equal((table[1].match(/<tr>/g) || []).length, 2);
  await fmt(app, 'image');
  await new Promise((r) => setImmediate(r));
  assert.ok(app.calls.some((c) => c.cmd === 'pick_inline_image'));
  assert.ok(app.commands.some(([c, v]) => c === 'insertHTML' && v === '<img src="data:image/png;base64,iVBORw==">'));
});

test('pasted HTML is cleaned by the backend, plain text is inserted as text', async () => {
  const app = setup(async (cmd, input) => cmd === 'clean_html' ? '<b>干净</b>' : undefined);
  app.open({ to: 'x@example.com', subject: '粘贴' });
  const editor = app.$('#c-body');
  let prevented = 0;
  const clip = (data) => ({ types: Object.keys(data), files: [], items: [], getData: (t) => data[t] || '' });
  await app.listeners.paste({ target: editor, clipboardData: clip({ 'text/html': '<b onclick="x()">脏</b>', 'text/plain': '脏' }), preventDefault() { prevented++; } });
  assert.equal(prevented, 1);
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'clean_html').input), { html: '<b onclick="x()">脏</b>' });
  assert.ok(app.commands.some(([c, v]) => c === 'insertHTML' && v === '<b>干净</b>'));
  await app.listeners.paste({ target: editor, clipboardData: clip({ 'text/plain': '只是文字' }), preventDefault() { prevented++; } });
  assert.ok(app.commands.some(([c, v]) => c === 'insertText' && v === '只是文字'));
});

test('switching sender replaces an untouched signature block but keeps an edited one', () => {
  let app = setup();
  app.open({ to: 'x@example.com', subject: '签名' });
  app.$('#c-from').value = 'b';
  app.change('#c-from');
  assert.match(app.$('#c-body').innerHTML, /Bob 的签名/);
  assert.doesNotMatch(app.$('#c-body').innerHTML, /Alice 的签名/);
  app = setup();
  app.open({ to: 'x@example.com', subject: '签名' });
  app.$('#c-body').innerHTML = app.$('#c-body').innerHTML.replace('Alice 的签名', 'Alice 的签名（手改）');
  app.$('#c-from').value = 'b';
  app.change('#c-from');
  assert.match(app.$('#c-body').innerHTML, /Alice 的签名（手改）/);
  assert.doesNotMatch(app.$('#c-body').innerHTML, /Bob 的签名/);
});

test('continue editing a rich draft cleans its HTML before putting it in the editor', async () => {
  const app = setup(async (cmd, input) => cmd === 'clean_html' ? '<div><b>草稿正文</b></div>' : cmd === 'draft_attachments' ? [] : undefined);
  const source = { accountId: 'a', folder: 'Drafts', uid: 12 };
  app.S.sel = { ...source };
  app.S.msg = { subject: '草稿', text: '草稿正文', html: '<div><b>草稿正文</b><script>x()</script></div>', to: [], cc: [], bcc: [], attachments: [], messageId: 'd@x', uidValidity: 5, references: [] };
  app.S.lists.set('a|Drafts', [{ ...source, subject: '草稿' }]);
  app.overlay.innerHTML = '<button data-action="edit-draft">继续编辑</button>';
  await app.click('edit-draft');
  assert.ok(app.calls.some((c) => c.cmd === 'clean_html' && c.input.html.includes('<script>') && c.input.blockRemote === true), '重开草稿不加载远程图片');
  assert.equal(app.$('#c-body').innerHTML, '<b>草稿正文</b>', '去掉发信时加的外层包装，避免每次重开多套一层');
});

test('Escape in the link field closes only the field and Enter inserts the link', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '链接' });
  fmt(app, 'link');
  const input = app.$('#c-link');
  input.value = 'example.com';
  app.document.activeElement = input;
  app.key('Escape');
  assert.ok(app.$('#c-body'), 'Esc 不能关掉整个写信窗口');
  assert.equal(app.$('.compose-popover').hidden, true);
  assert.equal(app.$('.compose-confirm').hidden, true);
  fmt(app, 'link');
  const again = app.$('#c-link');
  again.value = 'example.com';
  app.document.activeElement = again;
  app.key('Enter');
  assert.deepEqual(app.commands.find(([c]) => c === 'createLink'), ['createLink', 'https://example.com']);
});

test('HTML dropped into the editor is cleaned like a paste', async () => {
  const app = setup(async (cmd) => cmd === 'clean_html' ? '<p>干净</p>' : undefined);
  app.open({ to: 'x@example.com', subject: '拖拽' });
  let prevented = 0;
  await app.listeners.drop({ target: app.$('#c-body'), dataTransfer: { types: ['text/html'], files: [], getData: (t) => t === 'text/html' ? '<p class="x" style="position:fixed">脏</p>' : '' }, preventDefault() { prevented++; } });
  assert.equal(prevented, 1);
  assert.ok(app.calls.some((c) => c.cmd === 'clean_html'));
  assert.ok(app.commands.some(([c, v]) => c === 'insertHTML' && v === '<p>干净</p>'));
});

test('a clipboard with both an image and HTML keeps the HTML (e.g. a copied table)', async () => {
  const app = setup(async (cmd) => cmd === 'clean_html' ? '<table><tr><td>1</td></tr></table>' : undefined);
  app.open({ to: 'x@example.com', subject: '表格' });
  const png = { type: 'image/png', size: 10 };
  await app.listeners.paste({ target: app.$('#c-body'), clipboardData: { types: ['text/html', 'Files'], files: [png], getData: (t) => t === 'text/html' ? '<table><tr><td>1</td></tr></table>' : '' }, preventDefault() {} });
  assert.ok(app.commands.some(([c, v]) => c === 'insertHTML' && v.includes('<table>')));
});

test('typing is debounced before building autosave snapshots', async () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '防抖' });
  const sheet = app.$('.sheet');
  let changes = 0;
  const real = sheet._autosave.change;
  sheet._autosave.change = (snapshot) => { changes++; return real(snapshot); };
  for (let i = 0; i < 5; i++) { app.type('字'); app.listeners.input({ target: app.$('#c-body') }); }
  assert.equal(changes, 0, '连续输入时不立刻序列化整篇内容');
  await new Promise((r) => setTimeout(r, 450));
  assert.equal(changes, 1);
});

test('choosing a size from the dropdown keeps focus on it for arrow-key navigation', () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '字号' });
  const size = app.$('[data-fmt="size"]');
  size.value = 'small';
  app.document.activeElement = size;
  app.listeners.change({ target: size });
  assert.deepEqual(app.commands.filter(([c]) => c === 'fontSize'), [['fontSize', '2']]);
  assert.equal(size.value, 'small', '不重置，方向键可以继续选下一个');
  assert.equal(app.document.activeElement, size, '焦点留在下拉框');
});

test('the size picker returns to its placeholder when it loses focus', () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '字号' });
  const size = app.$('[data-fmt="size"]');
  size.value = 'large';
  app.listeners.change({ target: size });
  app.listeners.focusout({ target: size });
  assert.equal(size.value, '', '下次再选同一项也会触发');
});

test('Enter while composing text in the link field does not insert yet', () => {
  const app = setup();
  app.open({ to: 'x@example.com', subject: '链接' });
  fmt(app, 'link');
  const input = app.$('#c-link');
  input.value = 'example.com';
  app.document.activeElement = input;
  app.key('Enter', { isComposing: true });
  assert.equal(app.commands.some(([c]) => c === 'createLink'), false, '输入法选词的回车不能确认');
  assert.ok(app.$('#c-link'));
});

const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const typeInto = (app, selector, value) => {
  const input = app.$(selector);
  input.value = value;
  input.selectionStart = input.selectionEnd = value.length;
  app.document.activeElement = input;
  app.listeners.input({ target: input });
  return input;
};

test('recipient fields suggest contacts and insert the chosen one with the keyboard', async () => {
  const suggestions = [
    { kind: 'recent', name: '赵敏', email: 'zhao@example.com', members: [] },
    { kind: 'contact', name: '张三', email: 'zhang@example.com', members: [] },
  ];
  const app = setup(async (cmd, input) => cmd === 'contacts_suggest' ? suggestions : undefined);
  app.open({ subject: '联想' });
  const to = typeInto(app, '#c-to', 'a@example.com, zh');
  await wait(200);
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'contacts_suggest').input), { query: 'zh' });
  const list = app.$('.recipient-suggest');
  assert.ok(list && !list.hidden, '出现联想列表');
  assert.match(list.textContent, /赵敏[\s\S]*张三/);
  app.key('ArrowDown');
  app.key('Enter');
  assert.equal(to.value, 'a@example.com, 张三 <zhang@example.com>, ');
  assert.ok(!app.$('.recipient-suggest') || app.$('.recipient-suggest').hidden);
  assert.ok(app.$('#c-body'), '回车选择联系人不会发送或关闭');
});

test('Escape closes the suggestion list without closing the compose sheet', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? [{ kind: 'contact', name: 'A', email: 'a@x.com', members: [] }] : undefined);
  app.open({ subject: 's' });
  typeInto(app, '#c-to', 'a');
  await wait(200);
  app.key('Escape');
  assert.ok(app.$('#c-body'));
  assert.ok(!app.$('.recipient-suggest') || app.$('.recipient-suggest').hidden);
  assert.equal(app.$('.compose-confirm').hidden, true);
});

test('choosing a group inserts all of its members', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_suggest'
    ? [{ kind: 'group', name: '项目组', email: '', groupId: 1, members: ['张三 <z@x.com>', 'l@x.com'] }] : undefined);
  app.open({ subject: '分组' });
  const cc = (app.$('.cc-row').hidden = false, typeInto(app, '#c-cc', '项目'));
  await wait(200);
  assert.match(app.$('.recipient-suggest').textContent, /项目组/);
  app.$('.recipient-suggest [data-action="suggest-pick"]').click();
  assert.equal(cc.value, '张三 <z@x.com>, l@x.com, ');
});

test('the contact picker adds checked contacts to the chosen field without duplicates', async () => {
  const contacts = [
    { id: 1, name: '张三', emails: ['z@x.com'], org: '', note: '' },
    { id: 2, name: 'Li, Si', emails: ['l@x.com', 'l2@x.com'], org: '', note: '' },
  ];
  const app = setup(async (cmd) => cmd === 'contacts_list' ? contacts : cmd === 'groups_list' ? [] : undefined);
  app.open({ to: 'z@x.com', subject: '选择' });
  await app.click('pick-contacts');
  await wait(0);
  const boxes = app.overlay.querySelectorAll('[data-pick-address]');
  assert.equal(boxes.length, 2);
  for (const box of boxes) box.checked = true;
  app.overlay.querySelector('[data-action="picker-add"][data-target="cc"]').click();
  assert.equal(app.$('#c-cc').value, '张三 <z@x.com>, "Li, Si" <l@x.com>');
  assert.equal(app.$('.cc-row').hidden, false, '加到抄送时展开抄送栏');
  assert.equal(app.$('#c-to').value, 'z@x.com', '收件人不变');
});

test('the contacts manager creates, edits, deletes, groups and imports', async () => {
  let contacts = [{ id: 1, name: '张三', emails: ['z@x.com', 'z2@x.com'], org: 'ACME', note: '' }];
  let groups = [];
  const app = setup(async (cmd, input) => {
    if (cmd === 'contacts_list') return contacts;
    if (cmd === 'groups_list') return groups;
    if (cmd === 'contact_save') { contacts = contacts.concat({ ...input.contact, id: 2 }); return 2; }
    if (cmd === 'contact_delete') { contacts = contacts.filter((c) => c.id !== input.id); return null; }
    if (cmd === 'group_save') { groups = [{ id: 9, name: input.name, members: [] }]; return 9; }
    if (cmd === 'group_members_set') { groups = [{ ...groups[0], members: input.members }]; return null; }
    if (cmd === 'contacts_import') return { added: 3, updated: 1, skipped: 2 };
    return undefined;
  });
  await app.openContacts();
  assert.match(app.overlay.textContent, /张三[\s\S]*z@x.com/);
  await app.click('contact-new');
  app.$('#contact-name').value = '李四';
  app.$('#contact-emails').value = 'l@x.com\nl2@x.com, l3@x.com';
  await app.click('contact-save');
  const saved = app.calls.find((c) => c.cmd === 'contact_save').input.contact;
  assert.deepEqual(plain(saved), { id: null, name: '李四', emails: ['l@x.com', 'l2@x.com', 'l3@x.com'], org: '', note: '' });
  await wait(0);
  assert.match(app.overlay.textContent, /李四/);

  app.overlay.querySelector('[data-contact-id="1"]').click();
  await wait(0);
  assert.equal(app.$('#contact-name').value, '张三');
  assert.equal(app.$('#contact-emails').value, 'z@x.com\nz2@x.com', '多个邮箱每行一个');
  await app.click('contact-delete');
  await app.click('contact-delete-confirm');
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'contact_delete').input), { id: 1 });

  await app.click('group-new');
  app.$('#group-name').value = '项目组';
  await app.click('group-save');
  await wait(0);
  const member = app.overlay.querySelector('[data-member-id="2"]');
  member.checked = true;
  app.listeners.change({ target: member });
  await wait(0);
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'group_members_set').input), { id: 9, members: [2] });

  await app.click('contacts-import');
  await wait(0);
  assert.ok(app.notices.some((n) => /新增 3[\s\S]*合并 1[\s\S]*跳过 2/.test(String(n))));
});

test('a late suggestion response after Escape or a field change is ignored', async () => {
  let release;
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? new Promise((r) => { release = () => r([{ kind: 'contact', name: '王芳', email: 'w@x.com', members: [] }]); }) : undefined);
  app.open({ subject: '慢' });
  const to = typeInto(app, '#c-to', '王');
  await wait(200);
  app.key('Escape');
  app.$('.cc-row').hidden = false;
  const cc = app.$('#c-cc');
  app.document.activeElement = cc;
  release();
  await wait(0);
  assert.ok(!app.$('.recipient-suggest') || app.$('.recipient-suggest').hidden, '已关闭的列表不能被晚到的结果重新打开');
  app.key('Enter');
  assert.equal(to.value, '王', '在抄送框回车不能改收件人');
});

test('IME commits and Shift+Tab do not pick a suggestion', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? [{ kind: 'contact', name: '王芳', email: 'w@x.com', members: [] }] : undefined);
  app.open({ subject: '输入法' });
  const to = typeInto(app, '#c-to', '王');
  await wait(200);
  app.key('Enter', { keyCode: 229 });
  app.key('Tab', { shiftKey: true });
  assert.equal(to.value, '王');
});

test('suggestion list is announced as a combobox with an active option', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? [{ kind: 'contact', name: 'A', email: 'a@x.com', members: [] }, { kind: 'contact', name: 'B', email: 'b@x.com', members: [] }] : undefined);
  app.open({ subject: 'a11y' });
  const to = typeInto(app, '#c-to', 'a');
  await wait(200);
  assert.equal(to.attributes.role, 'combobox');
  assert.equal(to.attributes['aria-expanded'], 'true');
  app.key('ArrowDown');
  const active = app.overlay.querySelector('.suggest-item.active');
  assert.equal(to.attributes['aria-activedescendant'], active.id);
});

test('group suggestions skip addresses already in the field and recent entries can be forgotten', async () => {
  let items = [
    { kind: 'group', name: '项目组', email: '', groupId: 1, members: ['张三 <z@x.com>', 'l@x.com'] },
    { kind: 'recent', name: '噪音', email: 'noise@x.com', members: [] },
  ];
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? items : undefined);
  app.open({ subject: '去重' });
  const to = typeInto(app, '#c-to', 'z@x.com, 项目');
  await wait(200);
  app.overlay.querySelector('[data-action="suggest-forget"]').click();
  await wait(0);
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'contacts_forget').input), { email: 'noise@x.com' });
  assert.doesNotMatch(app.$('.recipient-suggest').textContent, /噪音/);
  app.overlay.querySelector('[data-action="suggest-pick"]').click();
  assert.equal(to.value, 'z@x.com, l@x.com, ');
});

test('Escape in the contact picker search closes only the picker', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_list' ? [{ id: 1, name: 'A', emails: ['a@x.com'], org: '', note: '' }] : cmd === 'groups_list' ? [] : undefined);
  app.open({ subject: '选择器' });
  await app.click('pick-contacts');
  await wait(0);
  app.document.activeElement = app.$('#c-picker-q');
  app.key('Escape');
  assert.ok(app.$('#c-body'));
  assert.equal(app.$('.contact-picker').hidden, true);
  assert.equal(app.$('.compose-confirm').hidden, true);
});

test('the contacts manager search never hides or overwrites the contact being edited, nor drops group members', async () => {
  const all = [
    { id: 1, name: '张磊', emails: ['zl@x.com'], org: '', note: '' },
    { id: 2, name: '王芳', emails: ['wf@x.com'], org: '', note: '' },
  ];
  let groups = [{ id: 9, name: '项目组', members: [1, 2] }];
  const app = setup(async (cmd, input) => {
    if (cmd === 'contacts_list') return input.query ? all.filter((c) => c.name.includes(input.query)) : all;
    if (cmd === 'groups_list') return groups;
    if (cmd === 'contact_save') return input.contact.id;
    if (cmd === 'group_members_set') { groups = [{ ...groups[0], members: input.members }]; return null; }
    return undefined;
  });
  await app.openContacts();
  app.overlay.querySelector('[data-contact-id="1"]').click();
  app.$('#contact-note').value = '未保存的备注';
  const q = app.$('#contacts-q');
  q.value = '王芳';
  app.listeners.input({ target: q });
  await wait(250);
  assert.equal(app.$('#contact-name').value, '张磊', '搜索不影响正在编辑的联系人');
  assert.equal(app.$('#contact-note').value, '未保存的备注', '搜索不丢掉未保存的修改');
  await app.click('contact-save');
  const saved = app.calls.find((c) => c.cmd === 'contact_save').input.contact;
  assert.equal(saved.id, 1);
  assert.equal(saved.name, '张磊');

  app.overlay.querySelector('[data-group-id="9"]').click();
  const member = app.overlay.querySelector('[data-member-id="2"]');
  member.checked = false;
  app.listeners.change({ target: member });
  await wait(0);
  assert.deepEqual(plain(app.calls.filter((c) => c.cmd === 'group_members_set').at(-1).input.members), [1], '被搜索过滤掉的成员不能被移出分组');
});

test('rapid group member toggles never lose an update', async () => {
  const all = [{ id: 1, name: 'A', emails: ['a@x.com'], org: '', note: '' }, { id: 2, name: 'B', emails: ['b@x.com'], org: '', note: '' }];
  const releases = [];
  const app = setup(async (cmd) => {
    if (cmd === 'contacts_list') return all;
    if (cmd === 'groups_list') return [{ id: 9, name: 'G', members: [1, 2] }];
    if (cmd === 'group_members_set') return new Promise((r) => releases.push(r));
    return undefined;
  });
  await app.openContacts();
  app.overlay.querySelector('[data-group-id="9"]').click();
  for (const id of [1, 2]) {
    const box = app.overlay.querySelector(`[data-member-id="${id}"]`);
    box.checked = false;
    app.listeners.change({ target: box });
  }
  releases.forEach((r) => r(null));
  await wait(0);
  const sets = app.calls.filter((c) => c.cmd === 'group_members_set').map((c) => plain(c.input.members));
  assert.deepEqual(sets.at(-1), [], '第二次勾选基于第一次的结果');
});

test('Shift+Delete forgets the highlighted recent suggestion from the keyboard', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_suggest' ? [{ kind: 'recent', name: '噪音', email: 'noise@x.com', members: [] }, { kind: 'contact', name: 'A', email: 'a@x.com', members: [] }] : undefined);
  app.open({ subject: '删除' });
  typeInto(app, '#c-to', 'x');
  await wait(200);
  app.key('Delete', { shiftKey: true });
  await wait(0);
  assert.deepEqual(plain(app.calls.find((c) => c.cmd === 'contacts_forget').input), { email: 'noise@x.com' });
  assert.equal(app.overlay.querySelector('.suggest-row').attributes.role, 'none');
});

test('closing the picker returns focus to its button and skips contacts without email', async () => {
  const app = setup(async (cmd) => cmd === 'contacts_list'
    ? [{ id: 1, name: 'A', emails: ['a@x.com'], org: '', note: '' }, { id: 2, name: '无邮箱', emails: [], org: '', note: '' }]
    : cmd === 'groups_list' ? [] : undefined);
  app.open({ subject: '焦点' });
  await app.click('pick-contacts');
  await wait(0);
  assert.equal(app.overlay.querySelectorAll('[data-pick-address]').length, 1);
  assert.doesNotMatch(app.$('.contact-picker').innerHTML, /undefined/);
  app.document.activeElement = app.$('#c-picker-q');
  app.key('Escape');
  assert.equal(app.document.activeElement, app.$('[data-action="pick-contacts"]'));
});


test('an empty send result never reports success or discards the editable draft', async () => {
  for (const result of [null, undefined, {}]) {
    const app = setup(async (cmd) => cmd === 'enqueue_send' ? result : undefined);
    app.open(reply);
    app.type('需要保留的内容');
    const before = app.values();
    await app.click('send');
    assert.ok(app.$('#c-body'), 'unconfirmed submission must keep the composer');
    assert.deepEqual(app.values(), before);
    assert.equal(app.$('[data-action="send"]').disabled, false);
    assert.ok(!app.notices.includes('已发送'));
    assert.ok(app.notices.some(value => String(value).includes('发送结果未确认')));
    await app.click('compose-close');
    await app.click('compose-discard');
  }
});


test('failed recovery sanitization releases newly registered attachment tokens without replacing a draft', async () => {
  const app = setup(async (cmd) => {
    if (cmd === 'compose_recovery_restore') return { sessionId: 'recover-1', revision: 1, draft: { accountId: 'a', subject: 'recover' }, editor: { html: '<b>body</b>' }, attachments: [{ token: 'recovered-token', name: 'a.pdf' }] };
    if (cmd === 'clean_html') throw Error('invalid stored HTML');
  }, { sessions: true });
  await app.api.restoreComposeRecovery('recover-1');
  assert.deepEqual(plain(app.calls.find(c => c.cmd === 'release_attachments')?.input || null), { tokens: ['recovered-token'] });
  assert.equal(app.$('.compose-sheet'), null);
});


test('compose child dialogs own the sending shortcut while they are open', async () => {
  const app = setup(undefined, { sessions: true }); app.open(reply);
  const body = app.$('#c-body'); body.focus();
  for (const selector of ['.contact-picker', '.compose-popover', '.compose-confirm']) {
    const dialog = app.$(selector); dialog.hidden = false;
    assert.equal(app.api.canSendCompose(body), false, selector + ' must block send');
    dialog.hidden = true;
  }
  app.$('.compose-sheet')._suggest = { input: app.$('#c-to'), items: [{}], index: 0 };
  assert.equal(app.api.canSendCompose(body), false, 'recipient suggestions must block send');
  app.$('.compose-sheet')._suggest = null;
  await app.click('compose-close');
});


test('buffered send carries the confirmed compose revision, undo period, full draft and followup option', async () => {
  const app = setup(undefined, { sessions: true });
  app.open({ ...reply, bcc: 'private@example.com' });
  app.type('最后输入');
  app.$('#c-receipt').checked = true;
  app.$('#c-followup').checked = true;
  const sessionId = app.$('.compose-sheet')._session.id;
  await app.click('send');
  const saved = app.calls.find(c => c.cmd === 'compose_recovery_save').input.input;
  const queued = app.calls.find(c => c.cmd === 'enqueue_send').input;
  assert.equal(queued.composeSessionId, sessionId);
  assert.equal(queued.composeRevision, saved.revision);
  assert.equal(queued.undoSeconds, 10);
  assert.equal(queued.followupSeconds, 172800);
  assert.equal(queued.draft.requestReceipt, true);
  assert.equal(queued.draft.bcc, 'private@example.com');
  assert.match(queued.draft.htmlBody, /最后输入/);
  assert.ok(queued.requestId);
  assert.equal(app.calls.some(c => c.cmd === 'send_mail'), false);
  assert.equal(app.$('.compose-sheet'), null);
});

test('save and close freezes every editable field until the save settles and unlocks on failure', async () => {
  const saving = deferred();
  const app = setup(cmd => cmd === 'save_draft' ? saving.promise : undefined);
  app.open(reply); app.type('保存前最后输入');
  await app.click('compose-close');
  const pending = app.click('compose-save-draft');
  const body = app.$('#c-body');
  assert.equal(body.attributes.contenteditable, 'false', 'pending save must prevent new body input from being lost');
  for (const field of ['to', 'cc', 'bcc', 'subject', 'receipt', 'followup']) assert.equal(app.$('#c-' + field).disabled, true, field + ' must be locked');
  saving.reject(Error('disk unavailable')); await pending;
  assert.equal(app.$('#c-body'), body);
  assert.equal(body.attributes.contenteditable, 'true');
  assert.match(app.body(), /保存前最后输入/);
  for (const field of ['to', 'cc', 'bcc', 'subject', 'receipt', 'followup']) assert.equal(app.$('#c-' + field).disabled, false, field + ' must unlock after failure');
  await app.click('compose-close'); await app.click('compose-discard');
});
