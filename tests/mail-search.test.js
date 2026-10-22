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

test('SV01: condition-only queries stay active without a keyword', () => {
  assert.equal(typeof search.active, 'function', 'SearchSpec needs an explicit active state');
  assert.equal(search.active({query:'',searching:true,searchFilters:{attachmentOnly:true}}),true);
  assert.equal(search.active({query:'',searching:true,searchFilters:{sender:'amy'}}),true);
  assert.equal(search.active({query:'',searching:true,searchFilters:{}}),false);
  assert.equal(search.active({query:'',searching:false,searchFilters:{attachmentOnly:true}}),false);
});
test('SV13: a saved all-mail view remains active after its last condition is cleared', () => {
  assert.equal(typeof search.active, 'function');
  assert.equal(search.active({query:'',activeSavedView:'view-a',searchFilters:{}}),true);
  assert.equal(search.active({query:'',activeSavedView:null,searchFilters:{}}),false);
});
test('SV05: a date upper bound is the next local midnight across daylight saving', () => {
  const {execFileSync}=require('node:child_process');
  const source=`const s=require('./ui/mail-search.js'); const x=s.request({acct:'a',folder:'INBOX',query:'',searchFilters:{after:'2026-03-08',before:'2026-03-08'}});process.stdout.write(JSON.stringify(x));`;
  const value=JSON.parse(execFileSync(process.execPath,['-e',source],{cwd:require('node:path').resolve(__dirname,'..'),env:{...process.env,TZ:'America/New_York'}}));
  assert.equal(value.before-value.after,23*3600000);
});
test('SV05: relative dates are resolved when queried, not frozen when saved', () => {
  const state={acct:'a',folder:'INBOX',query:'',searchFilters:{relativeDays:2}};
  const first=search.request(state,null,new Date(2026,9,22,12));
  const next=search.request(state,null,new Date(2026,9,23,12));
  assert.equal(first.after,new Date(2026,9,21).getTime());
  assert.equal(next.after,new Date(2026,9,22).getTime());
});
