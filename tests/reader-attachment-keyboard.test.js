const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const tick = () => new Promise(resolve => setImmediate(resolve));

function fixture(protocol, folder, uidValidity = null) {
  const calls = [], errors = [], previews = [], listeners = {};
  const source = { accountId: 'a', receiveEpoch: 1, folder, uid: 17, uidValidity, index: 2, localOnly: false };
  const overlay = { innerHTML: '' };
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search.js'),
      MailBeaconAttachments: { showPreview: (...args) => previews.push(args) },
      __TAURI__: { core: { invoke: async (command, input) => {
        calls.push({ command, input });
        if (command === 'index_message_attachments') return [{ partId: 2, filename: 'manual.pdf' }];
        return '/tmp/manual.pdf';
      } }, event: { listen: async () => {} } },
    },
    document: {
      querySelector: selector => selector === '#overlay' ? overlay : null,
      addEventListener: (name, listener) => { listeners[name] = listener; },
    },
    console, setTimeout, clearTimeout,
    recordError: value => errors.push(String(value)),
  });
  const app = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(app.slice(0, app.indexOf('(async function boot()')) + '\n toast = recordError; globalThis.state = S;', context);
  context.state.accounts = [{ id: 'a', receiveEpoch: 1, receiveProtocol: protocol }];
  context.state.sel = source;
  const button = {
    isConnected: true,
    classList: { contains: () => false },
    dataset: { att: '2', attachmentSource: JSON.stringify(source) },
    closest: selector => selector === '[data-att]' ? button : null,
  };
  return { calls, errors, previews, source, async activate() {
    await listeners.click({ target: button, detail: 0, preventDefault() {} });
    await tick();
  } };
}

for (const [protocol, folder] of [['pop3', 'INBOX'], ['imap', '__local_drafts'], ['imap', '__outbox']]) {
  test(`keyboard attachment activation works for ${protocol} ${folder} without an IMAP identity`, async () => {
    const t = fixture(protocol, folder);
    await t.activate();
    assert.deepEqual(t.errors, []);
    assert.deepEqual(t.calls.map(call => call.command), ['open_attachment']);
    assert.deepEqual(JSON.parse(JSON.stringify(t.calls[0].input)), t.source);
  });
}

test('keyboard activation retains quick preview for a verified IMAP attachment', async () => {
  const t = fixture('imap', 'INBOX', 7);
  await t.activate();
  assert.deepEqual(t.errors, []);
  assert.deepEqual(t.calls.map(call => call.command), ['index_message_attachments']);
  assert.equal(t.previews.length, 1);
  assert.equal(t.previews[0][1], 0);
});

test('an IMAP attachment missing its mailbox generation still fails closed', async () => {
  const t = fixture('imap', 'INBOX');
  await t.activate();
  assert.match(t.errors[0], /身份不完整/);
  assert.deepEqual(t.calls, []);
});
