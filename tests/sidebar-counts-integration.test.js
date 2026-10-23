const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const deferred = () => { let resolve; const promise = new Promise(r => resolve=r); return {promise,resolve}; };
const row = (id='a', epoch=1, value=5) => ({accountId:id,receiveEpoch:epoch,folders:[],starred:{value,exact:true}});
function setup(respond=async()=>({accounts:[row()]})) {
  const calls=[], tasks=new Map(); let timer=0, renders=0;
  const sidebar={innerHTML:''};
  const window={MailBeaconSidebarCounts:require('../ui/sidebar-counts.js'),MailBeaconViews:require('../ui/views.js'),MailBeaconProductivity:require('../ui/productivity-panels.js'),
    __TAURI__:{core:{invoke:async(cmd,args)=>{calls.push({cmd,args});return respond(cmd,args);}},event:{listen:async()=>{}}}};
  const context=vm.createContext({window,document:{addEventListener(){},querySelector(selector){return selector==='#sidebar'?sidebar:null;}},console,
    setTimeout(fn){tasks.set(++timer,fn);return timer;},clearTimeout(id){tasks.delete(id);}, countRender(){renders++;}});
  const source=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
  vm.runInContext(source.slice(0,source.indexOf('(async function boot()'))+`
    const actualSidebar=renderSidebar;
    renderSidebar=countRender; render=renderList=()=>{};
    globalThis.api={S,onMailEvent,refresh:typeof refreshSidebarCounts==='function'?refreshSidebarCounts:null,
      schedule:typeof scheduleSidebarCounts==='function'?scheduleSidebarCounts:null,
      renderSidebar:actualSidebar,apply:typeof applySidebarCounts==='function'?applySidebarCounts:null};`,context);
  context.api.S.accounts=[{id:'a',receiveEpoch:1,connectionRevision:1},{id:'b',receiveEpoch:1,connectionRevision:1}];
  return {...context.api,calls,tasks,sidebar,get renders(){return renders;}, async flush(){for(const [id,fn] of [...tasks])if(tasks.delete(id))await fn();}};
}
test('local sidebar refresh uses a count snapshot, never a paginated message list',async()=>{
  const t=setup(); await t.refresh();
  assert.deepEqual(t.calls.map(c=>c.cmd),['get_sidebar_counts']);
  assert.equal(t.S.sidebarCounts.get('a').starred.value,5); assert.equal(t.renders,0);
});
test('count refresh is coalesced and remote refresh runs only for requested accounts',async()=>{
  const t=setup();
  t.schedule({accountId:'a',remote:true});t.schedule({accountId:'a',remote:true,force:true});t.schedule();
  assert.equal(t.calls.length,0);assert.equal(t.tasks.size,1); await t.flush();
  assert.deepEqual(t.calls.map(c=>c.cmd),['get_sidebar_counts','refresh_sidebar_counts']);
  assert.equal(t.calls[1].args.accountId,'a');assert.equal(t.calls[1].args.force,true);
});
test('old account epoch and removed accounts cannot repopulate sidebar counts',async()=>{
  const t=setup();t.apply({accounts:[row(),row('b')]});
  t.S.accounts=[{id:'a',receiveEpoch:2}];t.apply({accounts:[row(),row('b')]});
  assert.equal(t.S.sidebarCounts.size,0);
  t.apply({accounts:[row('a',2,9)]});assert.equal(t.S.sidebarCounts.get('a').starred.value,9);
});
test('older delayed local request cannot overwrite newer star count',async()=>{
  const first=deferred();let n=0;const t=setup(async()=>++n===1?first.promise:{accounts:[row('a',1,7)]});
  const old=t.refresh();await t.refresh();first.resolve({accounts:[row('a',1,2)]});await old;
  assert.equal(t.S.sidebarCounts.get('a').starred.value,7);
});
test('account repair during IPC discards the former endpoint snapshot',async()=>{
  const pending=deferred(),t=setup(()=>pending.promise);const request=t.refresh();
  t.S.accounts[0].receiveEpoch=2;pending.resolve({accounts:[row()]});await request;
  assert.equal(t.S.sidebarCounts.size,0);
});
test('a background count failure preserves known counts and never writes false zero',async()=>{
  const t=setup(async()=>{throw Error('offline');});t.apply({accounts:[row()]});await t.refresh();
  assert.equal(t.S.sidebarCounts.get('a').starred.value,5);
});
test('a confirmed mutation while STATUS is running queues one forced refresh after it completes',async()=>{
  const pending=deferred();let remote=0;
  const t=setup(async cmd=>cmd==='get_sidebar_counts'?{accounts:[row()]}:++remote===1?pending.promise:{accounts:[row('a',1,8)]});
  t.schedule({accountId:'a',remote:true}); const first=t.flush();
  await new Promise(resolve=>setImmediate(resolve));
  t.schedule({accountId:'a',remote:true,force:true});await t.flush();
  assert.equal(remote,1,'do not let a forced request get swallowed by the backend running-job guard');
  pending.resolve({accounts:[row()]});await first;await t.flush();
  assert.equal(remote,2);assert.equal(t.calls.filter(c=>c.cmd==='refresh_sidebar_counts')[1].args.force,true);
  assert.equal(t.S.sidebarCounts.get('a').starred.value,8);
});
test('index progress requests local counts without starting remote mail reads',async()=>{
  const t=setup();t.onMailEvent({type:'localIndex'});await t.flush();
  assert.deepEqual(t.calls.map(c=>c.cmd),['get_sidebar_counts']);
});
test('real sidebar renders all folder and productivity counts including exact zero and partial counts',()=>{
  const t=setup(),count=(value,exact=true)=>({value,exact});
  t.S.acct='a';t.S.live.a={state:'online',unread:392};t.S.remindersLoaded=true;
  t.S.folders.a=[{path:'Drafts',name:'草稿',role:'drafts'},{path:'Sent',name:'已发送',role:'sent'},{path:'Trash',name:'已删除',role:'trash'},{path:'Custom',name:'自建',role:'other'}];
  t.S.sidebarCounts.set('a',{...row(),starred:count(3,false),attachments:count(8,false),sendRecords:count(150),folders:t.S.folders.a.map((f,i)=>({folder:f.path,count:count(i,i===0)}))});
  t.renderSidebar();
  const badge=key=>t.sidebar.innerHTML.match(new RegExp(`data-(?:go|action)="${key}"[^>]*>[\\s\\S]*?<span class="count[^\"]*"[^>]*>([^<]+)</span>[\\s\\S]*?</button>`))?.[1];
  for(const [key,value] of Object.entries({INBOX:'392',__unread:'392',__starred:'3',__snoozed:'0',__reminders:'0',__attachments:'8','send-records':'150',Drafts:'0',Sent:'1',Trash:'2',Custom:'3'}))assert.equal(badge(key),value,key);
});
