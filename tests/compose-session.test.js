const test = require('node:test');
const assert = require('node:assert/strict');
const C = require('../ui/compose-session');
const value = receipt => ({ draft: { accountId: 'a', bcc: 'hidden@example.com', htmlBody: '<b>正文</b><img src="data:image/png;base64,YQ==">', requestReceipt: receipt }, editor: { html: '<b>正文</b>', quote: { text: '原文' } } });
function deferred() { let resolve, reject; const promise = new Promise((a,b) => { resolve=a; reject=b; }); return {promise,resolve,reject}; }

test('receipt-only edits and reverted options update revision and durable dirty state', async () => {
  const writes=[];
  const s=C.create({id:'s1', initial:value(false), saved:true, save:async input => { writes.push(input); return input; }, remove:async()=>{}});
  s.update(value(true)); assert.equal(s.dirty,true); assert.equal(s.revision,1);
  await s.flush(); assert.equal(writes[0].draft.requestReceipt,true); assert.equal(writes[0].draft.bcc,'hidden@example.com'); assert.equal(s.dirty,false);
  s.update(value(false)); assert.equal(s.dirty,true); s.update(value(true)); assert.equal(s.dirty,false);
});
test('flush serializes saves and includes input made while the earlier write is pending', async () => {
  const first=deferred(), writes=[];
  const s=C.create({id:'s',initial:value(false),save:async x=>{writes.push(x);if(writes.length===1)await first.promise;return x;},remove:async()=>{}});
  const flushing=s.flush(); s.update(value(true)); first.resolve(); await flushing;
  assert.deepEqual(writes.map(x=>x.revision),[0,1]); assert.equal(s.dirty,false); assert.equal(writes[1].draft.requestReceipt,true);
});
test('failed persistence leaves unsaved content available and can be retried', async () => {
  let fail=true;
  const s=C.create({id:'s',initial:value(true),save:async x=>{if(fail)throw Error('disk full');return x;},remove:async()=>{}});
  await assert.rejects(s.flush(),/disk full/); assert.equal(s.state,'error'); assert.equal(s.value.draft.requestReceipt,true);
  fail=false;await s.flush();assert.equal(s.state,'saved');
});
test('ending waits for pending writes and blocks late changes from reviving the session', async () => {
  const pending=deferred(),events=[];
  const s=C.create({id:'s',initial:value(false),save:async x=>{await pending.promise;events.push('save');return x;},remove:async()=>events.push('remove')});
  const flushing=s.flush(),ending=s.end();assert.throws(()=>s.update(value(true)),/结束/);pending.resolve();await Promise.all([flushing,ending]);
  assert.deepEqual(events,['save','remove']);assert.equal(s.closed,true);
});
test('navigation failure preserves selection and competing navigation commits only the latest target', async () => {
  const gate=C.navigationGate(),slow=deferred(),targets=[];
  const b=gate(()=>slow.promise,()=>targets.push('B'));const c=gate(async()=>{},()=>targets.push('C'));await c;slow.resolve();await b;
  assert.deepEqual(targets,['C']);await assert.rejects(gate(async()=>{throw Error('disk');},()=>targets.push('D')),/disk/);assert.deepEqual(targets,['C']);
});
test('collapsed, unfocused, modal, busy and IME compose sessions cannot send',()=>{
  const ready={mode:'inline',focused:true,busy:false,blocked:false};assert.equal(C.canSend(ready),true);
  for(const other of [{mode:'collapsed'},{focused:false},{blocked:true},{busy:true},{composing:true}])assert.equal(C.canSend({...ready,...other}),false);
});
