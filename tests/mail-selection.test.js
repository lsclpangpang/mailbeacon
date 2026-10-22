const test = require('node:test');
const assert = require('node:assert/strict');
const M = require('../ui/mail-selection.js');
const mail = (uid, more = {}) => ({ accountId: 'a', receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid, ...more });
const group = (...items) => ({ latest: items[0], items });
test('mail selection identity isolates accounts, server epochs and mailbox generations', () => {
  const a = mail(1);
  assert.notEqual(M.key(a), M.key(mail(1, {receiveEpoch: 2})));
  assert.notEqual(M.key(a), M.key(mail(1, {uidValidity: 8})));
  assert.notEqual(M.key(a), M.key(mail(1, {accountId: 'b'})));
  assert.notEqual(M.key(mail(1, {folder: 'a|b'})), M.key(mail(1, {accountId: 'a|a', folder: 'b'})));
});
test('range selection follows rendered row order and includes only loaded thread members', () => {
  const groups = [group(mail(8), mail(6)), group(mail(5)), group(mail(2))];
  const chosen = M.toggle(new Set(), groups, 0);
  assert.equal(chosen.size, 2);
  const range = M.toggle(chosen, groups, 2, 0);
  assert.deepEqual(M.items(groups, range).map(m => m.uid), [8,6,5,2]);
  assert.deepEqual(M.items(groups, M.toggle(range, groups, 0)).map(m => m.uid), [5,2]);
});
test('refresh never expands selection, vanished entries are pruned and threads may be mixed', () => {
  const chosen = new Set([M.key(mail(1)), M.key(mail(2))]);
  const refreshed = [group(mail(1), mail(3))];
  const retained = M.retain(chosen, refreshed);
  assert.equal(retained.size, 1);
  assert.equal(M.state(refreshed[0], retained), 'mixed');
  assert.deepEqual(M.items(refreshed, retained).map(m => m.uid), [1]);
});
test('selection grouping preserves real folders and accounts without duplicate messages', () => {
  const groups = [group(mail(1), mail(1)), group(mail(2, {folder:'Sent'})), group(mail(1,{accountId:'b'}))];
  const picked = M.items(groups, M.all(groups));
  assert.equal(picked.length, 3);
  assert.equal(M.groupByMailbox(picked).length, 3);
});
