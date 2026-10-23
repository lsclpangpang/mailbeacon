const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke, { realOutcome = false, realList = false } = {}) {
  const bridge = async (cmd, args) => { const value = await invoke(cmd, args); return cmd === "execute_mail_operation" && value === undefined ? {operationId:args.request.operationId,items:args.request.items.map(source=>({source,status:"succeeded"}))} : value; };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const outcomeBox = { html: '', hidden: false, insertAdjacentHTML(_position, html) { this.html += html; }, querySelector() { return null; } };
  const context = vm.createContext({
    window: { MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require("../ui/mail-selection.js"), MailBeaconOperations: require("../ui/mail-operations.js"), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: bridge }, event: { listen: async () => {} } } },
    document: { querySelector: (selector) => selector === '#toast' ? outcomeBox : null, addEventListener: () => {} },
    console, setTimeout, clearTimeout, realOutcome, realList,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = function () {};
    if (!realOutcome) showOperationOutcome = function(result) { S.lastOutcome = result; };
    confirmPermanent = async function() { return S.confirmDelete === true; };
    openMessage = async function (sel) { S.sel = sel; S.msg = null; };
    const reloads = [], originalLoadList = loadList;
    loadList = async function (accountId, folder, options) {
      reloads.push({ accountId, folder, ...options });
      if (realList) return originalLoadList(accountId, folder, options);
    };
    globalThis.app = { S, reloads, loadList, moveCurrent, setFlag, markAllRead, runMailOperation, undoLastOperation, onMailEvent, refreshOperationSources, downloadMessage: typeof downloadMessage === 'function' ? downloadMessage : null };
  `, context);
  const api = context.app;
  api.S.accounts = [{ id: 'a', email: 'a@example.com' }];
  api.S.acct = 'a';
  api.S.folders.a = [{ path: 'INBOX', role: 'inbox' }, { path: 'Work', name: '工作', role: 'other' }];
  api.S.lists.set('a|INBOX', [1, 2, 3].map(uid => ({ accountId: 'a', folder: 'INBOX', receiveEpoch: 1, uidValidity: 7, uid, date: 4 - uid, seen: true })));
  api.S.sel = { accountId: 'a', folder: 'INBOX', uid: 1 };
  return { ...api, outcomeBox };
}

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('a move keeps the source visible until the server confirms success', async () => {
  const request = deferred();
  const { S, moveCurrent } = app(() => request.promise);
  const moving = moveCurrent('other', 'Work');
  assert.equal(S.lists.get('a|INBOX').length, 3);
  assert.equal(S.sel.uid, 1);
  request.resolve();
  await moving;
  assert.equal(S.lists.get('a|INBOX').some(m => m.uid === 1), false);
});

test('failed move retains the source and the selected message', async () => {
  const { S, moveCurrent } = app(async () => { throw new Error('网络中断'); });
  await moveCurrent('other', 'Work');
  assert.equal(S.lists.get('a|INBOX').length, 3);
  assert.equal(S.sel.uid, 1);
});

test('captured source stays stable and completing a move does not steal a newer selection', async () => {
  const request = deferred();
  let args;
  const { S, moveCurrent } = app((cmd, input) => { args = input; return request.promise; });
  const original = { ...S.sel };
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 2 };
  const moving = moveCurrent('other', 'Work', original);
  assert.equal(args.request.items[0].uid, 1);
  request.resolve();
  await moving;
  assert.equal(S.sel.uid, 2);
});

test('repeated move is ignored while the same message is already moving', async () => {
  const request = deferred();
  let calls = 0;
  const { moveCurrent } = app(() => { calls++; return request.promise; });
  const first = moveCurrent('other', 'Work');
  const second = moveCurrent('other', 'Work');
  assert.equal(calls, 1);
  request.resolve();
  await Promise.all([first, second]);
});

test('old inbox snapshots cannot resurrect successfully moved messages', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  const oldSnapshot = S.lists.get('a|INBOX').map(m => ({ ...m }));
  await moveCurrent('other', 'Work');
  onMailEvent({ type: 'inbox', accountId: 'a', messages: oldSnapshot });
  assert.equal(S.lists.get('a|INBOX').some(m => m.uid === 1), false);
});

test('download saves the captured message and rejects duplicate clicks', async () => {
  const request = deferred();
  const calls = [];
  const { S, downloadMessage } = app((cmd, input) => { calls.push({ cmd, input }); return request.promise; });
  assert.equal(typeof downloadMessage, 'function');
  const original = { ...S.sel };
  const first = downloadMessage(original);
  S.sel = { accountId: 'a', folder: 'INBOX', uid: 2 };
  const second = downloadMessage(original);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, 'save_message');
  assert.equal(calls[0].input.uid, 1);
  request.resolve('/Users/me/Downloads/邮件.eml');
  await Promise.all([first, second]);
});

test('invalidating an in-flight destination load releases its loading state', async () => {
  const { S, moveCurrent } = app(async () => {});
  S.loading.add('a|Work');
  S.loading.add('a|__starred');
  await moveCurrent('other', 'Work');
  assert.equal(S.loading.has('a|Work'), false);
  assert.equal(S.loading.has('a|__starred'), false);
});

test('reconnecting clears stale moved-UID filters', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  await moveCurrent('other', 'Work');
  assert.equal(S.removed.size, 1);
  onMailEvent({ type: 'status', accountId: 'a', state: 'online' });
  assert.equal(S.removed.size, 0);
});

test('reconnection accepts a reused UID in the initial inbox snapshot before online', async () => {
  const { S, moveCurrent, onMailEvent } = app(async () => {});
  await moveCurrent('other', 'Work');
  onMailEvent({ type: 'status', accountId: 'a', state: 'connecting' });
  onMailEvent({ type: 'inbox', accountId: 'a', messages: [{ accountId: 'a', folder: 'INBOX', uid: 1, subject: '新的合法邮件' }] });
  onMailEvent({ type: 'status', accountId: 'a', state: 'online' });
  assert.equal(S.lists.get('a|INBOX')[0]?.subject, '新的合法邮件');
});


test('failed flag change keeps confirmed local state', async () => {
  const { S, setFlag } = app(async (cmd,args) => ({items:args.request.items.map(source=>({source,status:'failed',message:'拒绝'}))}));
  const mail = S.lists.get('a|INBOX')[0];
  await setFlag(mail,'seen',false);
  assert.equal(mail.seen,true);
  assert.equal(S.lastOutcome.items[0].status,'failed');
});
test('mark loaded read reports partial failures and changes only confirmed messages', async () => {
  const { S, markAllRead } = app(async (cmd,args) => ({items:args.request.items.map((source,i)=>({source,status:i===0?'succeeded':'unknown'}))}));
  for (const m of S.lists.get('a|INBOX')) m.seen=false;
  await markAllRead();
  assert.equal(S.lists.get('a|INBOX').filter(m=>m.seen).length,1);
  assert.equal(S.lastOutcome.items.filter(m=>m.status==='unknown').length,2);
});
test('permanent delete cancellation sends no mutation command', async () => {
  let calls=0;
  const { S, moveCurrent } = app(async () => { calls++; });
  S.folders.a.find(f=>f.path==='INBOX').role='trash';
  await moveCurrent('trash');
  assert.equal(calls,0);
  assert.equal(S.lists.get('a|INBOX').length,3);
});
test('unknown move never removes a local row or schedules a retry', async () => {
  let calls=0;
  const { S, moveCurrent } = app(async (cmd,args) => { calls++; return {items:args.request.items.map(source=>({source,status:'unknown',message:'断线'}))}; });
  await moveCurrent('other','Work');
  assert.equal(calls,1);
  assert.equal(S.lists.get('a|INBOX').length,3);
});

for (const changed of [{ receiveEpoch: 2 }, { uidValidity: 8 }]) {
  test(`a failed operation never binds its recorded identity to a replacement ${Object.keys(changed)[0]}`, async () => {
    let calls = 0;
    const { S, runMailOperation } = app(async () => { calls++; });
    const original = { ...S.lists.get('a|INBOX')[0] };
    Object.assign(S.lists.get('a|INBOX')[0], changed);
    if (changed.receiveEpoch) S.accounts[0].receiveEpoch = changed.receiveEpoch;
    await runMailOperation('trash', [original]);
    assert.equal(calls, 0, 'must require refresh/reselection instead of mutating the new mail');
    assert.equal(S.lists.get('a|INBOX').length, 3);
  });
}

test('a failed undo clears the forward action and offers no generic retry', async () => {
  const source = { accountId: 'a', receiveEpoch: 1, folder: 'Trash', uidValidity: 7, uid: 9 };
  const { S, undoLastOperation, outcomeBox } = app(async () => ({ operationId:'undo:move', items:[{source,status:'failed',message:'mock failure'}] }), { realOutcome:true });
  S.lastRequest = { action:'trash', target:null };
  S.lastOperation = { operationId:'move', undoToken:'move', expiresAt:Date.now()+9000, items:[{source}] };
  await undoLastOperation();
  assert.equal(S.lastRequest, null);
  assert.doesNotMatch(outcomeBox.html, /operation-retry/);
  assert.match(outcomeBox.html, /operation-refresh/);
});

test('a batch move never opens or marks the next mail read, even when mode is exited in flight', async () => {
  const request = deferred();
  const { S, moveCurrent } = app(() => request.promise);
  S.batchMode = true;
  const moving = moveCurrent('other', 'Work');
  S.batchMode = false;
  request.resolve();
  await moving;
  assert.equal(S.sel, null);
});

const plain = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(resolve => setImmediate(resolve));
function moved(source, folder = 'Deleted Messages') {
  return { source, status:'succeeded', undo:{ original:source, destination:{ ...source, folder, uidValidity:9, uid:91 } } };
}

test('role moves refresh the server-confirmed destination instead of a stale role mapping', async () => {
  const t=app(async (_cmd,args) => ({items:args.request.items.map(source=>moved(source))}));
  t.S.folders.a.push({path:'OldTrash',role:'trash'});
  t.S.lists.set('a|OldTrash',[]);
  t.S.lists.set('a|Deleted Messages',[]);
  await t.moveCurrent('trash');
  assert.deepEqual(plain(t.reloads),[
    {accountId:'a',folder:'INBOX',force:true},
    {accountId:'a',folder:'Deleted Messages',force:true,forceRefresh:true},
  ]);
  assert.equal(t.S.lists.has('a|OldTrash'),true,'a stale role hint must not replace the confirmed target');
});

test('entering an empty trash folder during deletion reloads the actual destination after success', async () => {
  const mutation=deferred(), calls=[];
  const t=app(async (cmd,args) => {
    calls.push({cmd,args});
    if(cmd==='execute_mail_operation')return mutation.promise;
    if(cmd==='list_saved_messages')return {items:[]};
    if(cmd==='list_messages')return {items:args.folder==='Deleted Messages'&&args.forceRefresh?[{...moved(source).undo.destination,subject:'Moved mail'}]:[],complete:true};
  },{realList:true});
  const source={...t.S.lists.get('a|INBOX')[0]};
  t.S.folders.a.push({path:'Deleted Messages',role:'trash'});
  const removing=t.moveCurrent('trash');
  t.S.folder='Deleted Messages';t.S.sel=null;t.S.msg=null;
  t.S.lists.set('a|Deleted Messages',[]);
  mutation.resolve({items:[moved(source)]});
  await removing;await tick();
  assert.deepEqual(plain(t.S.lists.get('a|Deleted Messages')||[]).map(m=>m.uid),[91]);
  assert.ok(calls.some(c=>c.cmd==='list_messages'&&c.args.folder==='Deleted Messages'&&c.args.forceRefresh===true));
});

test('unknown deletion refreshes both candidate folders without fabricating success or a destination row', async () => {
  const t=app(async (_cmd,args)=>({items:args.request.items.map(source=>({source,status:'unknown',message:'connection lost'}))}));
  t.S.folders.a.push({path:'Deleted Messages',role:'trash'});
  t.S.lists.set('a|Deleted Messages',[]);
  assert.equal(await t.moveCurrent('trash'),false);
  assert.equal(t.S.lists.get('a|INBOX').some(m=>m.uid===1),true);
  assert.equal(t.S.lists.get('a|Deleted Messages').length,0);
  assert.equal(t.S.lastOutcome.items[0].status,'unknown');
  assert.deepEqual(plain(t.reloads).map(r=>r.folder),['INBOX','Deleted Messages']);
  assert.ok(t.reloads.every(r=>r.forceRefresh===true));
});

test('a late operation from the old receiving endpoint cannot clear or refresh the replacement account', async () => {
  const pending=deferred(),t=app(()=>pending.promise);
  const source={...t.S.lists.get('a|INBOX')[0]};
  t.S.folders.a.push({path:'Deleted Messages',role:'trash'});
  const removing=t.moveCurrent('trash');
  t.S.accounts[0].receiveEpoch=2;
  const replacement={...source,receiveEpoch:2};
  t.S.lists.set('a|INBOX',[replacement]);
  t.S.lists.set('a|Deleted Messages',[]);
  pending.resolve({items:[moved(source)]});await removing;
  assert.equal(t.S.lists.get('a|INBOX')[0],replacement);
  assert.equal(t.S.lists.has('a|Deleted Messages'),true);
  assert.equal(t.reloads.length,0);
});

test('undo refreshes both actual folders once using fresh server lists', async () => {
  const source={accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:1};
  const forward=moved(source), restored={...source,uid:99};
  const t=app(async()=>({items:[{source:forward.undo.destination,status:'succeeded',undo:{original:forward.undo.destination,destination:restored}}]}));
  t.S.lastOperation={undoToken:'move-1',expiresAt:Date.now()+9000,items:[forward]};
  await t.undoLastOperation();
  assert.deepEqual(plain(t.reloads).sort((a,b)=>a.folder.localeCompare(b.folder)),[
    {accountId:'a',folder:'Deleted Messages',force:true},
    {accountId:'a',folder:'INBOX',force:true,forceRefresh:true},
  ]);
});

test('list refresh is explicit: ordinary and index-completion reloads stay cached; pagination never forces a snapshot', async () => {
  const calls=[],t=app(async(cmd,args)=>{
    if(cmd==='list_saved_messages')return {items:[]};
    if(cmd==='list_messages'){calls.push(plain(args));return {items:[],complete:true};}
  },{realList:true});
  await t.loadList('a','INBOX');
  await t.loadList('a','INBOX',{force:true});
  await t.loadList('a','INBOX',{force:true,forceRefresh:true});
  await t.loadList('a','INBOX',{more:true,force:true,forceRefresh:true});
  assert.deepEqual(calls.map(c=>c.forceRefresh===true),[false,false,true,false]);
});

test('explicit refresh reaches the real sources of a virtual folder', async () => {
  const calls=[],t=app(async(cmd,args)=>{
    if(cmd==='list_messages'){calls.push(plain(args));return {items:[],complete:true};}
  },{realList:true});
  await t.loadList('a','__unread',{force:true,forceRefresh:true});
  assert.deepEqual(calls,[{accountId:'a',folder:'INBOX',before:null,flaggedOnly:false,unreadOnly:true,forceRefresh:true}]);
});

test('confirmed read and star changes keep the complete local list instead of forcing a server rebuild', async () => {
  for(const flag of ['seen','flagged']) {
    const t=app(async()=>undefined);
    const message=t.S.lists.get('a|INBOX')[0];
    await t.setFlag(message,flag,flag==='flagged');
    assert.deepEqual(plain(t.reloads),[{accountId:'a',folder:'INBOX',force:true}],flag);
  }
});

test('unknown flags and explicit verification still request a fresh server list', async () => {
  const t=app(async(_cmd,args)=>({items:args.request.items.map(source=>({source,status:'unknown'}))}));
  await t.setFlag(t.S.lists.get('a|INBOX')[0],'seen',false);
  assert.equal(t.S.lists.get('a|INBOX')[0].seen,true);
  assert.equal(t.reloads[0].forceRefresh,true);
  t.reloads.length=0;
  const source=t.S.lists.get('a|INBOX')[0];
  t.refreshOperationSources([source],null,{action:'seen',results:[{source,status:'succeeded'}],forceRefresh:true});
  assert.equal(t.reloads[0].forceRefresh,true);
});

test('successful deletion preserves the complete source list instead of falling back to forty headers', async () => {
  let index;
  const t=app(async(cmd,args)=>{
    if(cmd==='execute_mail_operation'){
      index=index.filter(m=>m.uid!==args.request.items[0].uid);
      return {items:args.request.items.map(source=>moved(source))};
    }
    if(cmd==='list_saved_messages')return {items:[]};
    if(cmd==='list_messages')return {items:args.folder==='INBOX'?(args.forceRefresh?index.slice(0,40):index):[],complete:!args.forceRefresh};
  },{realList:true});
  const source=t.S.lists.get('a|INBOX')[0];
  index=Array.from({length:300},(_,i)=>({...source,uid:i+1}));
  t.S.lists.set('a|INBOX',index.slice());
  await t.moveCurrent('trash');await tick();
  assert.equal(t.S.lists.get('a|INBOX').length,299);
  assert.equal(t.reloads.find(r=>r.folder==='INBOX').forceRefresh,undefined);
});

test('a folder used as both source and destination is refreshed once with destination verification', () => {
  const t=app(async()=>undefined);
  const inbox={...t.S.lists.get('a|INBOX')[0]},work={...inbox,folder:'Work',uid:7};
  t.refreshOperationSources([inbox,work],null,{action:'move',results:[moved(inbox,'Work'),moved(work,'INBOX')]});
  assert.deepEqual(plain(t.reloads),[
    {accountId:'a',folder:'INBOX',force:true,forceRefresh:true},
    {accountId:'a',folder:'Work',force:true,forceRefresh:true},
  ]);
});

test('permanent deletion has no destination and keeps the confirmed source index', () => {
  const t=app(async()=>undefined),source={...t.S.lists.get('a|INBOX')[0],folder:'Deleted Messages'};
  t.S.folders.a.push({path:source.folder,role:'trash'});
  t.refreshOperationSources([source],null,{action:'trash',results:[{source,status:'succeeded',undo:null}]});
  assert.deepEqual(plain(t.reloads),[{accountId:'a',folder:source.folder,force:true}]);
});
