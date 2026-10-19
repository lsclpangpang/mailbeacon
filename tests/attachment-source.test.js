const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const decode = value => value.replace(/&(amp|quot|lt|gt|#39);/g, (_, entity) => ({ amp: '&', quot: '"', lt: '<', gt: '>', '#39': "'" })[entity]);

test('attachment buttons retain the rendered message UIDVALIDITY after selection changes', async () => {
  const calls = [], listeners = {}, reader = { dataset: {}, innerHTML: '' };
  const context = vm.createContext({
    window: { __TAURI__: { core: { invoke: async (cmd, args) => { calls.push({ cmd, args }); return '/tmp/attachment.pdf'; } }, event: { listen: async () => {} } } },
    document: { querySelector: selector => selector === '#reader' ? reader : null, addEventListener: (name, callback) => { listeners[name] = callback; } },
    console, setTimeout, clearTimeout,
  });
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + '\ntoast = function() {}; globalThis.api = { S, renderReader };', context);
  const { S, renderReader } = context.api;
  S.accounts = [{ id: 'a', name: 'A', email: 'a@example.test' }];
  S.acct = 'a';
  S.sel = { accountId: 'a', folder: 'Projects|2026', uid: 17 };
  S.folder = 'Projects|2026';
  S.lists.set('a|Projects|2026', [{ ...S.sel, subject: 'Attachment', date: 1, fromAddr: 'sender@example.test' }]);
  S.msg = { subject: 'Attachment', uidValidity: 1234, from: { name: 'Sender', addr: 'sender@example.test' }, to: [], cc: [], date: 1, text: 'Mail', attachments: [{ index: 2, name: '报价.pdf', size: 1024 }] };
  renderReader();
  const captured = reader.innerHTML.match(/data-attachment-source="([^"]+)"/);
  assert.ok(captured, 'the rendered attachment must bind its original source');
  const button = { dataset: { att: '2', attachmentSource: decode(captured[1]) }, classList: { contains: () => false }, closest(selector) { return selector === '[data-att]' ? this : null; } };
  S.sel = { accountId: 'b', folder: 'INBOX', uid: 900 };
  S.msg = { uidValidity: 5678 };
  await listeners.click({ target: button, preventDefault() {} });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, 'save_attachment');
  assert.deepEqual(JSON.parse(JSON.stringify(calls[0].args)), { accountId: 'a', folder: 'Projects|2026', uid: 17, uidValidity: 1234, index: 2 });
});
