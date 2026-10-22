const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const A = require('../ui/account-repair.js');

test('repair input cannot replace account identity and empty password means keep', () => {
  const account = { id: 'a', email: 'demo@example.test', receiveProtocol: 'imap', connectionRevision: 4 };
  assert.deepEqual(A.input(account, { email: 'other@example.test', password: '', imapHost: 'mail.example.test', imapPort: '993', smtpHost: 'smtp.example.test', smtpPort: '587', name: 'Demo' }), {
    expectedRevision: 4, name: 'Demo', password: '', imapHost: 'mail.example.test', imapPort: 993, smtpHost: 'smtp.example.test', smtpPort: 587,
  });
});

test('stale configuration events cannot replace repaired account status', () => {
  const a = { connectionRevision: 3, receiveEpoch: 2 };
  assert.equal(A.acceptEvent(a, { connectionRevision: 2 }), false);
  assert.equal(A.acceptEvent(a, { connectionRevision: 3, receiveEpoch: 1 }), false);
  assert.equal(A.acceptEvent(a, { connectionRevision: 3, receiveEpoch: 2 }), true);
  assert.equal(A.acceptEvent(undefined, { connectionRevision: 1 }), true);
});

test('failed new-account form retains custom servers, TLS and temporary password', () => {
  const values = { email: 'demo@example.test', password: 'new input only', name: 'Demo', pop3Host: 'pop.example.test', pop3Port: '110', pop3Tls: false, smtpHost: 'smtp.example.test', smtpPort: '587' };
  const controls = Object.entries(values).map(([name, value]) => ({ name, value: typeof value === 'boolean' ? 'on' : value, type: typeof value === 'boolean' ? 'checkbox' : 'text', checked: value }));
  const copy = A.capture({ elements: controls });
  const rebuilt = { elements: controls.map(c => ({ ...c, value: '', checked: true })) };
  A.restore(rebuilt, copy);
  assert.deepEqual(A.capture(rebuilt), copy);
});

test('connection test distinguishes receive and send without claiming both repaired', () => {
  assert.match(A.testSummary({ receive: { ok: true }, send: { ok: false, detail: 'TLS failed' } }), /收信：正常.*发信：TLS failed/);
});

test('real page loads repair helpers and exposes existing-account repair commands', () => {
  const page = fs.readFileSync('ui/index.html', 'utf8');
  assert.ok(page.indexOf('account-repair.js') >= 0 && page.indexOf('account-repair.js') < page.indexOf('app.js'));
  const app = fs.readFileSync('ui/app.js', 'utf8');
  for (const cmd of ['test_account_connection', 'update_account_connection', 'reconnect_account']) assert.ok(app.includes(cmd));
  assert.ok(app.includes('MailBeaconAccountRepair.restore(nf, keep)'));
});
