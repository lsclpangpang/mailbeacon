const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const deferred=()=>{let resolve,reject;const promise=new Promise((y,n)=>{resolve=y;reject=n});return{promise,resolve,reject}};
const tick=()=>new Promise(r=>setImmediate(r));
const source={accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:42};
const body={subject:'Saved',from:{name:'Sender',addr:'sender@example.test'},to:[],cc:[],text:'body',attachments:[],uidValidity:7};
function setup(handler){
 const calls=[];const context=vm.createContext({window:{MailBeaconSearch:require('../ui/mail-search.js'),MailBeaconViews:require('../ui/views.js'),MailBeaconThreads:require('../ui/threads.js'),__TAURI__:{core:{invoke:async(cmd,input)=>{calls.push({cmd,input});return handler(cmd,input)}},event:{listen:async()=>{}}}},document:{activeElement:null,querySelector:()=>null,querySelectorAll:()=>[],addEventListener:()=>{}},console,setTimeout,clearTimeout});
 const sourceCode=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
 vm.runInContext(sourceCode.slice(0,sourceCode.indexOf('(async function boot()'))+`\nrender=renderList=renderReader=renderSidebar=updateBadge=revealSelectedRow=redrawNotifySection=function(){};loadPreviews=async function(){};toast=function(){};globalThis.api={S,openMessage,loadList,loadFolders,moreSavedMessages,localReadOnly,progressText,onMailEvent};`,context);
 const api=context.api;Object.assign(api.S,{accounts:[{id:'a',email:'a@example.test',receiveEpoch:1,receiveProtocol:'imap'}],acct:'a',folder:'INBOX',live:{a:{unread:1}}});return{...api,calls};
}
test('cold start shows saved partial headers while the server is unavailable',async()=>{
 const network=deferred();const t=setup(async cmd=>cmd==='list_saved_messages'?{items:[{...source,seen:false}],coverage:[],hasMore:true,nextCursor:'next'}:cmd==='list_messages'?network.promise:null);
 const loaded=t.loadList('a','INBOX');await tick();assert.equal(t.S.lists.get('a|INBOX')[0].localOnly,true);network.reject('offline');await loaded;assert.equal(t.S.lists.get('a|INBOX')[0].uid,42);assert.equal(t.S.localSnapshots.get('a|INBOX').hasMore,true);
});
test('offline open uses only local reference command and never optimistically marks read',async()=>{
 const t=setup(async(cmd,input)=>{assert.equal(cmd,'get_message_ref');assert.equal(input.localOnly,true);return{source,message:body,localOnly:true,readMarked:false,snapshotAt:1}});
 t.S.lists.set('a|INBOX',[{...source,seen:false,localOnly:true}]);await t.openMessage(source);
 assert.equal(t.S.msg.localOnly,true);assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);assert.equal(t.S.live.a.unread,1);assert.deepEqual(t.calls.map(c=>c.cmd),['get_message_ref']);
});
test('an unconfirmed online seen result leaves unread state intact',async()=>{
 const t=setup(async cmd=>cmd==='get_message_ref'?{source,message:body,localOnly:false,readMarked:false}:null);t.S.lists.set('a|INBOX',[{...source,seen:false}]);await t.openMessage(source);assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);assert.equal(t.S.live.a.unread,1);
});
test('late reader and folder results cannot repopulate a different receiving endpoint',async()=>{
 const pending=deferred();const t=setup(async cmd=>cmd==='get_message_ref'?pending.promise:null);t.S.lists.set('a|INBOX',[{...source,seen:false}]);const opened=t.openMessage(source);await tick();t.S.accounts[0].receiveEpoch=2;pending.resolve({source,message:body,localOnly:false,readMarked:true});await opened;assert.equal(t.S.msg,null);assert.equal(t.S.live.a.unread,1);
 const folders=deferred();const f=setup(async cmd=>cmd==='list_saved_folders'?[]:folders.promise);const loading=f.loadFolders('a');await tick();f.S.accounts[0].receiveEpoch=2;folders.resolve([{path:'OldServer',role:'other'}]);await loading;assert.equal(f.S.folders.a,undefined);
});
test('unknown mailbox generation is refused before any network or write request',async()=>{
 const t=setup(async()=>{throw Error('must not invoke')});const unknown={accountId:'a',folder:'INBOX',uid:42};t.S.lists.set('a|INBOX',[{...unknown,seen:false}]);await t.openMessage(unknown);assert.match(t.S.msgError,/身份尚未确认/);assert.equal(t.calls.length,0);
});

test('late saved pagination is discarded after receiving endpoint changes and duplicate clicks are ignored',async()=>{
 const pending=deferred();const t=setup(async()=>pending.promise);t.S.lists.set('a|INBOX',[{...source}]);t.S.localSnapshots.set('a|INBOX',{hasMore:true,nextCursor:'page2'});
 const more=t.moreSavedMessages();const duplicate=t.moreSavedMessages();await tick();assert.equal(t.calls.length,1);
 t.S.accounts[0].receiveEpoch=2;t.S.lists.clear();t.S.localSnapshots.clear();pending.resolve({items:[{...source,uid:41}],hasMore:false});await Promise.all([more,duplicate]);assert.equal(t.S.lists.has('a|INBOX'),false);
});

const snapshot=(ref=source, message=body)=>({source:ref,message,localOnly:true,readMarked:false,snapshotAt:123});
const online=(ref=source, message=body, readMarked=true)=>({source:ref,message,localOnly:false,readMarked});

test('online open presents a cached body before the network responds, keeping it read-only and unread',async()=>{
 const network=deferred();
 const t=setup(async(cmd,input)=>{
  assert.equal(cmd,'get_message_ref');
  return input.localOnly ? snapshot() : network.promise;
 });
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 try {
  await tick();
  assert.equal(t.S.msg?.text,'body','local body must be visible while online validation is pending');
  assert.equal(t.S.msg.localOnly,true);
  assert.equal(t.S.msg.syncing,true);
  assert.equal(t.localReadOnly(t.S.lists.get('a|INBOX')[0]),true,'cached display must not enable server mutations');
  assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
  assert.equal(t.S.live.a.unread,1);
  assert.deepEqual(t.calls.map(c=>c.input.localOnly),[true,false]);
  t.onMailEvent({type:'messagePartial',accountId:'a',folder:'INBOX',uid:42,view:{...body,text:'partial text',partial:true}});
  assert.equal(t.S.msg.text,'body','a delayed partial event must not replace the complete cached body');
 } finally {
  network.resolve(online());
  await opened;
 }
 assert.equal(t.S.msg.localOnly,false);
 assert.equal(t.S.msg.syncing,false);
 assert.equal(t.localReadOnly(t.S.lists.get('a|INBOX')[0]),false);
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,true);
 assert.equal(t.S.live.a.unread,0);
});

test('a local cache miss continues to the normal online read without surfacing the local miss',async()=>{
 const t=setup(async(cmd,input)=>{
  assert.equal(cmd,'get_message_ref');
  if(input.localOnly)throw Error('这封邮件尚未缓存完整正文');
  return online();
 });
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 await t.openMessage(source);
 assert.deepEqual(t.calls.map(c=>c.input.localOnly),[true,false]);
 assert.equal(t.S.msg.text,'body');
 assert.equal(t.S.msg.localOnly,false);
 assert.equal(t.S.msgError,'');
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,true);
});

test('a failed online refresh keeps the already visible cached body readable and does not mark it read',async()=>{
 const network=deferred();
 const t=setup(async(cmd,input)=>input.localOnly ? snapshot() : network.promise);
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 network.reject(Error('连接超时'));
 await opened;
 assert.equal(t.S.msg?.text,'body');
 assert.equal(t.S.msg.localOnly,true);
 assert.equal(t.S.msg.syncing,false);
 assert.match(t.S.msg.syncError,/连接超时/);
 assert.equal(t.localReadOnly(t.S.lists.get('a|INBOX')[0]),true);
 assert.equal(t.S.msgError,'','the reader error card must not hide an available local copy');
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
 assert.equal(t.S.live.a.unread,1);
});

test('a late online result cannot replace another message with the same UID in a different folder',async()=>{
 const network=deferred(), other={...source,folder:'Archive',uidValidity:8};
 const otherBody={...body,text:'other folder body',uidValidity:8};
 const t=setup(async(cmd,input)=>{
  if(cmd!=='get_message_ref')return null;
  if(input.source.folder==='Archive')return input.localOnly ? snapshot(other,otherBody) : online(other,otherBody);
  return input.localOnly ? snapshot() : network.promise;
 });
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 t.S.lists.set('a|Archive',[{...other,seen:false}]);
 const first=t.openMessage(source);
 await tick();
 await t.openMessage(other);
 network.resolve(online());
 await first;
 assert.equal(t.S.sel.folder,'Archive');
 assert.equal(t.S.msg.text,'other folder body');
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
 assert.equal(t.S.live.a.unread,1);
});

test('a late online result cannot revive the same UID after its mailbox generation changes',async()=>{
 const network=deferred(), newer={...source,uidValidity:8};
 const t=setup(async(cmd,input)=>input.localOnly ? snapshot() : network.promise);
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 // A list refresh has replaced the source, independently of the pending command.
 t.S.sel=newer;
 t.S.msg=null;
 t.S.lists.set('a|INBOX',[{...newer,seen:false}]);
 network.resolve(online());
 await opened;
 assert.equal(t.S.msg,null,'old UIDVALIDITY must not populate the new mailbox generation');
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
 assert.equal(t.S.live.a.unread,1);
});

test('a late local-cache response after an account change cannot revive the reader or start more work',async()=>{
 const local=deferred();
 const t=setup(async()=>local.promise);
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 t.S.accounts[0].receiveEpoch=2;
 local.resolve(snapshot());
 await opened;
 assert.equal(t.S.msg,null);
 assert.deepEqual(t.calls.map(c=>c.input.localOnly),[true,false]);
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
 assert.equal(t.S.live.a.unread,1);
});

test('a late online result cannot revive a local copy after receiving endpoint invalidation',async()=>{
 const network=deferred(), newer={...source,receiveEpoch:2};
 const t=setup(async(cmd,input)=>input.localOnly ? snapshot() : network.promise);
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 t.S.accounts[0].receiveEpoch=2;
 t.S.sel=newer;
 t.S.msg=null;
 t.S.lists.set('a|INBOX',[{...newer,seen:false}]);
 network.resolve(online());
 await opened;
 assert.equal(t.S.msg,null);
 assert.equal(t.S.lists.get('a|INBOX')[0].seen,false);
 assert.equal(t.S.live.a.unread,1);
});

test('an online response with any mismatched source field cannot upgrade the local copy or mark it read',async t=>{
 for(const [field,value] of Object.entries({accountId:'other-account',receiveEpoch:2,folder:'Archive',uidValidity:8,uid:43})){
  await t.test(field,async()=>{
   const ref={...source,[field]:value};
   const app=setup(async(cmd,input)=>input.localOnly ? snapshot() : online(ref,{...body,text:'wrong source body'}));
   app.S.lists.set('a|INBOX',[{...source,seen:false}]);
   await app.openMessage(source);
   assert.equal(app.S.msg.text,'body');
   assert.equal(app.S.msg.localOnly,true);
   assert.equal(app.S.msg.syncing,false);
   assert.ok(app.S.msg.syncError,'identity failure should remain visible alongside the read-only copy');
   assert.equal(app.S.msgError,'');
   assert.equal(app.S.lists.get('a|INBOX')[0].seen,false);
   assert.equal(app.S.live.a.unread,1);
  });
 }
});

test('a zero-byte loading state does not claim that a cached mail is downloading',()=>{
 const t=setup(async()=>null);
 assert.doesNotMatch(t.progressText(0,null),/下载/);
 assert.doesNotMatch(t.progressText(0,2048),/下载/);
 assert.match(t.progressText(1024,2048),/1 KB/);
 assert.doesNotMatch(t.progressText(1024,2048),/下载/,'protocol bytes do not prove a body download');
});


test('a slow local archive never blocks an available online body or downgrades it later',async()=>{
 const local=deferred();
 const t=setup(async(cmd,input)=>cmd==='get_message_ref' ? (input.localOnly ? local.promise : online()) : null);
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 try {
  assert.equal(t.S.msg?.text,body.text,'online body must be visible while local disk read is still pending');
  assert.equal(t.S.msg.localOnly,false);
  assert.equal(t.S.lists.get('a|INBOX')[0].seen,true);
 } finally { local.resolve(snapshot()); }
 await opened;await tick();
 assert.equal(t.S.msg.localOnly,false,'late cache cannot downgrade a verified reader');
 assert.equal(t.S.live.a.unread,0);
});

test('an early network error still waits for the local copy without hiding its body',async()=>{
 const local=deferred();
 const t=setup(async(cmd,input)=>{if(input.localOnly)return local.promise;throw Error('连接中断')});
 t.S.lists.set('a|INBOX',[{...source,seen:false}]);
 const opened=t.openMessage(source);
 await tick();
 local.resolve(snapshot());
 await opened;
 assert.equal(t.S.msg.text,body.text);
 assert.equal(t.S.msg.localOnly,true);
 assert.equal(t.S.msg.syncing,false);
 assert.match(t.S.msg.syncError,/连接中断/);
 assert.equal(t.S.msgError,'');
});
