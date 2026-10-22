const test=require('node:test');
const assert=require('node:assert/strict');
const search=require('../ui/mail-search.js');
const accounts=[{id:'a',receiveEpoch:2},{id:'b',receiveEpoch:7}];
const folders={a:[{path:'INBOX',role:'inbox'},{path:'Archive',role:'archive'},{path:'Trash',role:'trash'}],b:[{path:'INBOX',role:'inbox'}]};
function capture(state){assert.equal(typeof search.captureSpec,'function','saved scopes need an explicit snapshot');return search.captureSpec(state,accounts,folders);}
test('SV02: saving current folder freezes accounts and generations',()=>{
 const spec=capture({acct:'all',folder:'INBOX',query:'',searchScope:'folder',searchFilters:{attachmentOnly:true}});
 assert.equal(spec.scope,'fixed');assert.deepEqual(spec.targets,[{accountId:'a',receiveEpoch:2,folder:'INBOX'},{accountId:'b',receiveEpoch:7,folder:'INBOX'}]);
 assert.equal(spec.attachmentOnly,true);
 accounts[0].receiveEpoch=9;assert.equal(spec.targets[0].receiveEpoch,2);accounts[0].receiveEpoch=2;
});
test('SV03: only explicit all-account scope stays dynamic',()=>{
 const spec=capture({acct:'a',folder:'INBOX',query:'invoice',searchScope:'all',searchFilters:{}});
 assert.equal(spec.scope,'all');assert.deepEqual(spec.targets,[]);
});
test('SV04: unknown folder catalog prevents saving an accidental broad scope',()=>{
 assert.equal(typeof search.captureSpec,'function');
 assert.throws(()=>search.captureSpec({acct:'a',folder:'INBOX',query:'',searchScope:'account'},accounts,{}),/文件夹/);
});
test('SV06: editing a view clones conditions while preserving its fixed source',()=>{
 assert.equal(typeof search.editSpec,'function');
 const saved={specVersion:1,scope:'fixed',targets:[{accountId:'a',receiveEpoch:2,folder:'Archive'}],query:'one',sort:'dateDesc'};
 const next=search.editSpec(saved,{query:'two',searchFilters:{relativeDays:30,attachmentOnly:true},searchIncludeExcluded:false});
 assert.equal(saved.query,'one');assert.equal(next.query,'two');assert.equal(next.relativeDays,30);assert.deepEqual(next.targets,saved.targets);
 next.targets[0].folder='changed';assert.equal(saved.targets[0].folder,'Archive');
});
test('SV02: unread aggregate preserves role sources and unread condition',()=>{
 const spec=capture({acct:'a',folder:'__unread',query:'',searchScope:'folder',searchFilters:{}});
 assert.equal(spec.unreadOnly,true);assert.deepEqual(spec.targets.map(x=>x.folder),['INBOX']);
});
