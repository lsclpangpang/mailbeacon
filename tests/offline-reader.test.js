const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
const deferred=()=>{let resolve,reject;const promise=new Promise((y,n)=>{resolve=y;reject=n});return{promise,resolve,reject}};
const tick=()=>new Promise(r=>setImmediate(r));
const source={accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:42};
const body={subject:'Saved',from:{name:'Sender',addr:'sender@example.test'},to:[],cc:[],text:'body',attachments:[],uidValidity:7};
function setup(handler){
 const calls=[];const context=vm.createContext({window:{MailBeaconSearch:require('../ui/mail-search.js'),MailBeaconViews:require('../ui/views.js'),MailBeaconThreads:require('../ui/threads.js'),__TAURI__:{core:{invoke:async(cmd,input)=>{calls.push({cmd,input});return handler(cmd,input)}},event:{listen:async()=>{}}}},document:{activeElement:null,querySelector:()=>null,querySelectorAll:()=>[],addEventListener:()=>{}},console,setTimeout,clearTimeout});
 const sourceCode=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
 vm.runInContext(sourceCode.slice(0,sourceCode.indexOf('(async function boot()'))+`\nrender=renderList=renderReader=renderSidebar=updateBadge=revealSelectedRow=redrawNotifySection=function(){};loadPreviews=async function(){};toast=function(){};globalThis.api={S,openMessage,loadList,loadFolders,moreSavedMessages};`,context);
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
