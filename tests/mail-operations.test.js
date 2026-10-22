const test = require('node:test');
const assert = require('node:assert/strict');
const O = require('../ui/mail-operations.js');
test('partial and uncertain results never appear as an all-success toast', () => {
  const result = O.summary({items:[{status:'succeeded'}, {status:'failed'}, {status:'unknown'}]});
  assert.equal(result.ok, false);
  assert.match(result.text, /成功 1/);
  assert.match(result.text, /失败 1/);
  assert.match(result.text, /待确认 1/);
  assert.match(O.summary({items:[]}).text, /未确认/);
  assert.equal(O.summary(true).ok, false);
});
test('permanent delete is recognized per actual folder and separated from normal trash', () => {
  const mails = [{accountId:'a',folder:'INBOX',uid:1},{accountId:'a',folder:'Trash',uid:2}];
  const role = (a,f) => f === 'Trash' ? 'trash' : 'inbox';
  assert.deepEqual(O.partitionTrash(mails,role).permanent.map(m=>m.uid), [2]);
  assert.deepEqual(O.partitionTrash(mails,role).normal.map(m=>m.uid), [1]);
});
test('retry offers only definite failures and never includes unknown results', () => {
  const result = {items:[{status:'succeeded',source:{uid:1}},{status:'failed',source:{uid:2}},{status:'unknown',source:{uid:3}}]};
  assert.deepEqual(O.retryItems(result).map(m=>m.uid),[2]);
});
test('a response missing requested items remains unknown for those messages', () => {
  const refs=[{accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:1},{accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:2}];
  const result=O.completeResult({items:[{source:refs[0],status:'succeeded'}]},refs);
  assert.deepEqual(result.items.map(m=>m.status),['succeeded','unknown']);
  assert.equal(O.summary(result).ok,false);
});
