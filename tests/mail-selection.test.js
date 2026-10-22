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

test('drag selection expands and contracts from its original snapshot in either direction', () => {
  const groups = [group(mail(1)), group(mail(2)), group(mail(3)), group(mail(4)), group(mail(5))];
  const base = new Set([M.key(mail(5))]);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 1, 3)).map(m => m.uid), [2,3,4,5]);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 1, 1)).map(m => m.uid), [2,5]);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 3, 0)).map(m => m.uid), [1,2,3,4,5]);
  assert.deepEqual([...base], [M.key(mail(5))], 'drag calculations never mutate the snapshot');
});

test('drag deselection restores entries when the range shrinks and retains outside selections', () => {
  const groups = [group(mail(1)), group(mail(2)), group(mail(3)), group(mail(4))];
  const base = M.all(groups);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 1, 3, true)).map(m => m.uid), [1]);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 1, 1, true)).map(m => m.uid), [1,3,4]);
  assert.deepEqual(M.items(groups, M.dragRange(base, groups, 2, 0, true)).map(m => m.uid), [4]);
  assert.equal(base.size, 4);
});

test('drag ranges preserve complete mailbox identities and include only loaded thread members', () => {
  const groups = [group(mail(7), mail(8)), group(mail(7, {accountId:'b'})), group(mail(7, {uidValidity:8}))];
  const picked = M.dragRange(new Set(), groups, 0, 1);
  assert.equal(picked.size, 3);
  assert.equal(picked.has(M.key(mail(7, {uidValidity:8}))), false);
  for (const [anchor, index] of [[-1,1],[0,3],[0,1.5],[NaN,0]]) {
    const unchanged = M.dragRange(picked, groups, anchor, index);
    assert.notEqual(unchanged, picked);
    assert.deepEqual(unchanged, picked);
  }
});

test('pointer row lookup includes scroll offset and clamps to loaded virtual rows', () => {
  assert.equal(M.rowIndexAt(100,100,0,200), 0);
  assert.equal(M.rowIndexAt(165.9,100,0,200), 0);
  assert.equal(M.rowIndexAt(166,100,0,200), 1);
  assert.equal(M.rowIndexAt(166,100,6600,200), 101);
  assert.equal(M.rowIndexAt(-100,100,0,200), 0);
  assert.equal(M.rowIndexAt(100000,100,6600,200), 199);
  assert.equal(M.rowIndexAt(120,100,0,0), -1);
  assert.equal(M.rowIndexAt(NaN,100,0,200), -1);
  assert.equal(M.rowIndexAt(120,100,0,200,0), -1);
});

test('edge scrolling accelerates toward the edge and stays bounded outside the viewport', () => {
  assert.equal(M.edgeVelocity(200,100,500), 0);
  assert.equal(M.edgeVelocity(136,100,500), 0);
  assert.equal(M.edgeVelocity(464,100,500), 0);
  assert.equal(M.edgeVelocity(118,100,500), -9);
  assert.equal(M.edgeVelocity(482,100,500), 9);
  assert.equal(M.edgeVelocity(-200,100,500), -18);
  assert.equal(M.edgeVelocity(900,100,500), 18);
});

test('edge scrolling handles short or invalid viewports without overlapping direction zones', () => {
  assert.equal(M.edgeVelocity(110,100,120), 0);
  assert.equal(M.edgeVelocity(105,100,120), -9);
  assert.equal(M.edgeVelocity(115,100,120), 9);
  assert.equal(M.edgeVelocity(100,100,100), 0);
  assert.equal(M.edgeVelocity(100,100,90), 0);
  assert.equal(M.edgeVelocity(NaN,100,500), 0);
});
