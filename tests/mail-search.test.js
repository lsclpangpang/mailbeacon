const test = require('node:test');
const assert = require('node:assert/strict');
const search = require('../ui/mail-search.js');

test('search request keeps scope, explicit filters and cursor together', () => {
  const input = search.request({ acct:'a', folder:'INBOX', query:'合同', searchScope:'all', searchIncludeExcluded:true, searchFilters:{unreadOnly:true, flaggedOnly:false, attachmentOnly:true, sender:'amy', after:'2026-01-01', before:'2026-02-01'} }, 'next');
  assert.equal(input.scope, 'all'); assert.equal(input.cursor, 'next'); assert.equal(input.includeExcluded,true);
  assert.equal(input.attachmentOnly,true); assert.ok(input.before > input.after);
});
test('virtual folders preserve their exact view and retain account boundary', () => {
  const input=search.request({acct:'a',folder:'__unread',query:'报告',searchScope:'folder',searchFilters:{}});
  assert.equal(input.scope,'folder'); assert.equal(input.folder,'__unread'); assert.equal(input.accountId,'a'); assert.equal(input.unreadOnly,true);
});
test('message references require a known mailbox generation and keep receive endpoint identity', () => {
  assert.equal(search.source({accountId:'a',folder:'INBOX',uid:1,receiveEpoch:2}),null);
  assert.deepEqual(search.source({accountId:'a',folder:'INBOX',uid:1,receiveEpoch:2,uidValidity:7}),{accountId:'a',folder:'INBOX',uid:1,receiveEpoch:2,uidValidity:7});
});
test('partial body coverage does not claim a complete zero-result search', () => {
  const text=search.coverageText([{indexed:20,total:30,full:4,prefix:12,skipped:2,snapshotAt:100}],[]);
  assert.match(text,/摘要 20\/30/); assert.match(text,/完整正文 4/); assert.match(text,/未覆盖/);
  assert.match(search.coverageText([],['文件夹未同步']),/文件夹未同步/);
});

test('sent and draft views keep role scope while expanded searches drop view-only filters', () => {
  for (const folder of ['__sent','__drafts']) {
    const input=search.request({acct:'all',folder,query:'报告',searchScope:'folder'});
    assert.equal(input.scope,'folder'); assert.equal(input.folder,folder);
  }
  assert.equal(search.request({acct:'all',folder:'__unread',query:'报告',searchScope:'all'}).unreadOnly,false);
});
