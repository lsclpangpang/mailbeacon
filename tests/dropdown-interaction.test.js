const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
const appSource = source.slice(0, source.indexOf('(async function boot()'));

function context(document, extra = {}) {
  const sandbox = vm.createContext({
    window: {
      MailBeaconRules: require('../ui/rules.js'),
      __TAURI__: { core: { invoke: async () => null }, event: { listen: async () => {} } },
    },
    document, console, setTimeout, clearTimeout, ...extra,
  });
  vm.runInContext(appSource, sandbox);
  return sandbox;
}

// Model form replacement and native focus loss without recreating a browser DOM.
// The generated controls still come from the real accountForm() markup.
function protocolForm() {
  const listeners = new Map();
  const body = { tagName: 'BODY' };
  const document = {
    activeElement: body,
    addEventListener(name, listener) {
      if (!listeners.has(name)) listeners.set(name, []);
      listeners.get(name).push(listener);
    },
    querySelector: selector => selector === '#acct-form-wrap' ? wrap : wrap.querySelector(selector),
  };
  let form;
  const attributes = html => Object.fromEntries([...html.matchAll(/([\w-]+)(?:="([^"]*)")?/g)].map(([, key, value]) => [key, value ?? '']));
  const wrap = {
    querySelector: selector => selector === '#acct-form' ? form : form?.querySelector(selector),
    set innerHTML(html) {
      if (document.activeElement.form === form) document.activeElement = body;
      const controls = [];
      form = {
        elements: controls,
        querySelector: selector => controls.find(control => selector === `#${control.id}` ||
          selector.match(/\[name=["']?([^\]"']+)/)?.[1] === control.name) || null,
        querySelectorAll: () => controls,
      };
      controls.namedItem = name => controls.find(control => control.name === name || control.id === name) || null;
      for (const match of html.matchAll(/<input\b([^>]*)>|<select\b([^>]*)>([\s\S]*?)<\/select>/g)) {
        const attrs = attributes(match[1] ?? match[2]);
        const options = [...(match[3] || '').matchAll(/<option\b([^>]*)>/g)].map(option => attributes(option[1]));
        const control = {
          tagName: match[1] !== undefined ? 'INPUT' : 'SELECT',
          id: attrs.id || '', name: attrs.name || '', type: attrs.type || 'text',
          value: options.length ? (options.find(option => 'selected' in option) || options[0]).value : attrs.value || '',
          checked: 'checked' in attrs, disabled: 'disabled' in attrs, form, dataset: {},
          matches: selector => selector === 'input' ? control.tagName === 'INPUT' : selector === 'select' && control.tagName === 'SELECT',
          closest: selector => selector === '#acct-form-wrap' ? wrap : null,
          focus() { if (control.form === form && !control.disabled) document.activeElement = control; },
        };
        controls.push(control);
        if (control.name) form[control.name] = control;
      }
    },
  };
  class FormDataFixture {
    constructor(current) { this.values = current.elements.filter(control => control.name && !control.disabled &&
      (control.type !== 'checkbox' || control.checked)).map(control => [control.name, control.value]); }
    entries() { return this.values[Symbol.iterator](); }
    [Symbol.iterator]() { return this.entries(); }
    get(name) { return this.values.find(([key]) => key === name)?.[1] ?? null; }
  }
  const sandbox = context(document, { FormData: FormDataFixture });
  vm.runInContext('formKind = "custom"; formReceiveProtocol = "imap"; globalThis.accountMarkup = accountForm();', sandbox);
  wrap.innerHTML = sandbox.accountMarkup;
  const protocol = () => form.elements.namedItem('receiveProtocol');
  return {
    document, field: name => form.elements.namedItem(name), protocol,
    change(value) {
      const target = protocol();
      target.focus(); target.value = value;
      for (const listener of listeners.get('change') || []) listener({ target });
    },
  };
}

test('switching receive protocol preserves shared account form values', () => {
  const page = protocolForm();
  const values = {
    email: 'account@example.invalid', name: 'Example User', password: 'placeholder-authorization-code',
    smtpHost: 'smtp.example.invalid', smtpPort: '587',
  };
  for (const [name, value] of Object.entries(values)) page.field(name).value = value;
  page.change('pop3');
  assert.equal(page.protocol().value, 'pop3');
  // Compare booleans, so even failed assertions never print authorization text.
  for (const [name, value] of Object.entries(values)) {
    assert.equal(page.field(name)?.value === value, true, `${name} survives switching to POP3`);
  }
  page.change('imap');
  for (const [name, value] of Object.entries(values)) {
    assert.equal(page.field(name)?.value === value, true, `${name} survives switching back to IMAP`);
  }
});

test('switching receive protocol restores focus to the replacement selector', () => {
  const page = protocolForm();
  const original = page.protocol();
  page.change('pop3');
  assert.notEqual(page.protocol(), original, 'protocol-dependent fields were rebuilt');
  assert.equal(page.document.activeElement === page.protocol(), true, 'keyboard focus stays on the new protocol selector');
});

test('changing a later rule condition restores focus to that same condition', () => {
  const body = { tagName: 'BODY' };
  let fields = [];
  const document = {
    activeElement: body, addEventListener() {},
    querySelector(selector) {
      const index = Number(selector.match(/data-cond-index=["']?(\d+)/)?.[1] ?? 0);
      if (!selector.includes('field')) return fields[index]?.closest('.rule-cond') || null;
      return fields[index] || null;
    },
  };
  const render = () => {
    for (const field of fields) field.isConnected = false;
    document.activeElement = body;
    fields = [0, 1].map(index => {
      const row = { dataset: { condIndex: String(index) }, querySelector: () => fields[index] };
      return {
        name: 'field', isConnected: true, closest: selector => selector === '.rule-cond' ? row : null,
        focus() { if (this.isConnected) document.activeElement = this; },
      };
    });
  };
  const rule = {
    accountId: 'a', conditions: [{ field: 'from', op: 'contains', value: 'sender' },
      { field: 'attachment', op: 'contains', value: '' }], actions: { moveTo: null },
  };
  const sandbox = context(document, { readFixture: () => rule, renderFixture: render });
  vm.runInContext(`
    readRuleForm = readFixture; renderRules = renderFixture;
    S.rulesUi = { editing: { rule: readFixture() } };
    globalThis.changeRule = ruleFormChanged;
  `, sandbox);
  render();
  const second = fields[1]; second.focus();
  sandbox.changeRule({}, second);
  assert.equal(document.activeElement === fields[1], true, 'focus must not jump to the first condition');
});
