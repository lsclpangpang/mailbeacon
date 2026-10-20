const test = require('node:test');
const assert = require('node:assert/strict');
const X = require('../ui/compose-extras.js');

const at = (s) => new Date(s).getTime();

test('quick send times skip the ones that are already past', () => {
  const morning = X.quickSendTimes(at('2026-10-20T09:30:00'));
  assert.deepEqual(morning.map((q) => q.label), ['今天 18:00', '明天 08:00', '明天 14:00']);
  assert.equal(morning[0].at, at('2026-10-20T18:00:00'));
  const evening = X.quickSendTimes(at('2026-10-20T17:58:00'));
  assert.deepEqual(evening.map((q) => q.label), ['明天 08:00', '明天 14:00'], '只剩几分钟的不算');
});

test('the default send time is the next full hour, at least ten minutes away', () => {
  assert.equal(X.defaultSendAt(at('2026-10-20T09:30:00')), at('2026-10-20T10:00:00'));
  assert.equal(X.defaultSendAt(at('2026-10-20T09:55:00')), at('2026-10-20T11:00:00'));
});

test('datetime-local values round-trip in local time', () => {
  const ms = at('2026-10-20T18:05:00');
  assert.equal(X.toLocalInput(ms), '2026-10-20T18:05');
  assert.equal(X.fromLocalInput('2026-10-20T18:05'), ms);
  assert.ok(Number.isNaN(X.fromLocalInput('')));
});

test('send times read naturally', () => {
  const now = at('2026-10-20T09:30:00');
  assert.equal(X.fmtSendAt(at('2026-10-20T18:00:00'), now), '今天 18:00');
  assert.equal(X.fmtSendAt(at('2026-10-21T08:05:00'), now), '明天 08:05');
  assert.equal(X.fmtSendAt(at('2026-11-03T09:00:00'), now), '11月3日 09:00');
  assert.equal(X.fmtSendAt(at('2027-01-03T09:00:00'), now), '2027年1月3日 09:00');
});

test('a receipt is only offered for other people\'s mail that asked and was not answered', () => {
  const v = { receiptTo: 'boss@x.com', receiptHandled: false, from: { addr: 'boss@x.com' } };
  assert.equal(X.offerReceipt(v, 'inbox', 'me@x.com', false), true);
  assert.equal(X.offerReceipt({ ...v, receiptHandled: true }, 'inbox', 'me@x.com', false), false, '服务器上已标记');
  assert.equal(X.offerReceipt(v, 'inbox', 'me@x.com', true), false, '本机已处理');
  assert.equal(X.offerReceipt({ ...v, receiptTo: 'ME@x.com' }, 'inbox', 'me@x.com', false), false, '自己发给自己的');
  for (const role of ['sent', 'drafts', 'local-drafts', 'outbox', 'trash', 'junk']) assert.equal(X.offerReceipt(v, role, 'me@x.com', false), false, role);
  assert.equal(X.offerReceipt({ ...v, receiptTo: null }, 'inbox', 'me@x.com', false), false);
});

test('a receipt to someone other than the sender is pointed out', () => {
  assert.equal(X.receiptNote({ receiptTo: 'boss@x.com', from: { addr: 'Boss@x.com' } }), '');
  assert.match(X.receiptNote({ receiptTo: 'track@spam.io', from: { addr: 'boss@x.com' } }), /不是发件人/);
});
