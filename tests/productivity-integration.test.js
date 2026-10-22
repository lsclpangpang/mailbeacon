const test=require('node:test'),assert=require('node:assert/strict'),fs=require('node:fs'),vm=require('node:vm');
function page(reply){
 const calls=[],timers=new Map();let timer=0;
 const document={activeElement:{},addEventListener(){},querySelector(){return null;},querySelectorAll(){return []}};
 const window={addEventListener(){},MailBeaconSearch:require('../ui/mail-search'),MailBeaconProductivity:require('../ui/productivity-panels'),MailBeaconReaderMotion:require('../ui/reader-motion'),MailBeaconSelection:require('../ui/mail-selection'),MailBeaconThreads:require('../ui/threads'),MailBeaconViews:require('../ui/views'),MailBeaconOperations:require('../ui/mail-operations'),MailBeaconPrefs:require('../ui/prefs'),__TAURI__:{core:{invoke:async(command,args)=>{calls.push({command,args});return reply?reply(command,args):{items:[],coverage:[],hasMore:false};}},event:{listen:async()=>{}}}};
 const context=vm.createContext({window,document,console,setTimeout:fn=>{timers.set(++timer,fn);return timer;},clearTimeout:id=>timers.delete(id)});
 const source=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
 vm.runInContext(source.slice(0,source.indexOf('(async function boot()'))+'\nrenderList=renderSidebar=()=>{};toast=()=>{};globalThis.api={S,currentList,requestServerSearch,listEntries,collectAttachmentMetadata,refreshProductivity,onSendRecord,upsertReminder};',context);
 const {S}=context.api;S.accounts=[{id:'a',receiveEpoch:1,receiveProtocol:'imap'}];S.acct='a';S.folder='INBOX';S.searching=true;
 return {...context.api,calls,async flush(){const fns=[...timers.values()];timers.clear();for(const fn of fns)await fn();}};
}
test('SV12: empty-keyword search, list and navigation share the result sequence',async()=>{
 const t=page();t.S.searchFilters={attachmentOnly:true};t.requestServerSearch();await t.flush();
 assert.equal(t.calls.filter(x=>x.command==='search_mail').length,1);
 const found={accountId:'a',receiveEpoch:1,folder:'Archive',uidValidity:2,uid:2,date:1,seen:false};
 t.S.lists.set('a|INBOX',[{...found,uid:99,folder:'INBOX'}]);t.S.results.set('__search',{query:'',items:[found],hasMore:true,nextCursor:'page-2'});
 assert.deepEqual(t.currentList().map(x=>x.uid),[2]);assert.deepEqual(Array.from(t.listEntries().items,x=>x.uid),[2]);
 t.requestServerSearch({more:true});await t.flush();assert.equal(t.calls.at(-1).args.input.cursor,'page-2');
});
test('SV13: saved view with no conditions executes its fixed definition',async()=>{
 const t=page();t.S.activeSavedView='saved';t.S.savedSearchSpec={specVersion:1,scope:'fixed',targets:[{accountId:'a',receiveEpoch:1,folder:'Archive'}],query:'',sort:'dateDesc'};
 t.requestServerSearch();await t.flush();assert.equal(t.calls.at(-1)?.command,'search_saved_view');assert.equal(t.calls.at(-1).args.spec.targets[0].folder,'Archive');
});
test('RM01: inbox hides exact snooze sources but saved searches still find them',()=>{
 const t=page(),m={accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:9,uid:5,seen:false,date:1};
 t.S.searching=false;t.S.lists.set('a|INBOX',[m]);t.S.mailReminders=[{kind:'snooze',state:'scheduled',source:m,dueAt:Date.now()+60000}];
 assert.equal(t.currentList().length,0);
 t.S.searching=true;t.S.searchFilters={unreadOnly:true};t.S.results.set('__search',{query:'',items:[m]});assert.equal(t.currentList().length,1);
});

test('AT04: attachment collection advances past its first bounded batch',async()=>{
 const t=page();t.S.lists.set('a|INBOX',Array.from({length:25},(_,i)=>({accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:9,uid:i+1,attachment:true})));
 const targets=[{accountId:'a',receiveEpoch:1,folder:'INBOX'}];
 await t.collectAttachmentMetadata(targets);await t.collectAttachmentMetadata(targets);
 const calls=t.calls.filter(x=>x.command==='index_message_attachments');
 assert.equal(calls.length,25);assert.equal(new Set(calls.map(x=>x.args.source.uid)).size,25);
});

test('X07: boot refresh keeps events received after its read started',async()=>{
 let resolveSends,resolveReminders;
 const t=page(command=>command==='list_active_sends'?new Promise(r=>resolveSends=r):command==='list_mail_reminders'?new Promise(r=>resolveReminders=r):[]);
 const reading=t.refreshProductivity();
 t.onSendRecord({id:'send',accountId:'a',version:3,status:'accepted',copyStatus:'saved'});
 t.upsertReminder({id:'reminder',accountId:'a',version:4,state:'completed'});
 resolveSends([{id:'send',accountId:'a',version:1,status:'buffered'}]);
 resolveReminders([{id:'reminder',accountId:'a',version:1,state:'scheduled'}]);await reading;
 assert.equal(t.S.bufferRecords[0].status,'accepted');assert.equal(t.S.mailReminders[0].state,'completed');
});
