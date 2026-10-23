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
    renderSidebar=countRender; render=renderList=renderSendBuffer=toast=()=>{};
    globalThis.api={S,onMailEvent,refresh:typeof refreshSidebarCounts==='function'?refreshSidebarCounts:null,
      schedule:typeof scheduleSidebarCounts==='function'?scheduleSidebarCounts:null,
      draftSaved,onSendRecord,upsertReminder,renderSidebar:actualSidebar,apply:typeof applySidebarCounts==='function'?applySidebarCounts:null};`,context);
  context.api.S.accounts=[{id:'a',receiveEpoch:1,connectionRevision:1},{id:'b',receiveEpoch:1,connectionRevision:1}];
  return {...context.api,calls,tasks,sidebar,get renders(){return renders;}, async flush(){for(const [id,fn] of [...tasks])if(tasks.delete(id))await fn();}};
}

test('review: confirmed server draft save bypasses an already warm count cache', async () => {
  const response = value => ({accounts:[{...row(),folders:[{folder:'Drafts',count:{value,exact:true}}]}]});
  const t=setup(async(cmd,args)=>cmd==='refresh_sidebar_counts'?response(args.force?3:2):response(2));
  t.draftSaved({accountId:'a',folder:'Drafts',receiveEpoch:1,uid:9});
  await t.flush();
  assert.equal(t.S.sidebarCounts.get('a').folders[0].count.value,3,'a known server mutation must not reuse the 30-second old total');
});

test('review: confirmed sent-copy persistence bypasses an already warm count cache', async () => {
  const response = value => ({accounts:[{...row(),folders:[{folder:'Sent',count:{value,exact:true}}]}]});
  const t=setup(async(cmd,args)=>cmd==='refresh_sidebar_counts'?response(args.force?3:2):response(2));
  t.S.bufferRecords=[{id:'send',accountId:'a',status:'accepted',copyStatus:'pending',version:1}];
  t.onSendRecord({id:'send',accountId:'a',status:'accepted',copyStatus:'saved',version:2});
  await t.flush();
  assert.equal(t.S.sidebarCounts.get('a').folders[0].count.value,3,'saved sent copy is a confirmed mailbox mutation');
});

test('review: a late reminder event for a removed account cannot increase all-account count', () => {
  const t=setup();t.S.accounts=t.S.accounts.filter(a=>a.id!=='b');t.S.remindersLoaded=true;
  t.upsertReminder({id:'late',accountId:'b',receiveEpoch:1,kind:'followup',state:'scheduled',version:1});
  const count=require('../ui/sidebar-counts.js').resolve(t.S,'__reminders');
  assert.equal(count.value,0);
});

test('review: count responses update existing badges without replacing focused sidebar controls', () => {
  const t=setup(), attributes={};
  const badge={textContent:'old',classList:{toggle(name,value){attributes[name]=value;}},setAttribute(name,value){attributes[name]=value;}};
  const focusedButton={dataset:{go:'__starred'},querySelector(){return badge;}};
  t.sidebar.querySelectorAll=()=>[focusedButton];
  t.sidebar.innerHTML='preserved sidebar';t.sidebar.scrollTop=120;
  t.apply({accounts:[row('a',1,23),row('b',1,0)]});
  assert.equal(t.renders,0,'count response must not call full renderSidebar');
  assert.equal(t.sidebar.innerHTML,'preserved sidebar');
  assert.equal(t.sidebar.scrollTop,120);
  assert.equal(badge.textContent,'23');
  assert.equal(attributes['aria-label'],'共 23 项');
});

test('review: account configuration changes while reading counts cannot discard a queued forced refresh', async () => {
  const pending=deferred();let local=0;
  const t=setup(async cmd=>cmd==='get_sidebar_counts'&&++local===1?pending.promise:{accounts:[row()]});
  t.schedule({accountId:'a',remote:true,force:true});const first=t.flush();
  t.S.accounts[1].connectionRevision=2;
  pending.resolve({accounts:[row()]});await first;await t.flush();
  const remote=t.calls.filter(call=>call.cmd==='refresh_sidebar_counts');
  assert.equal(remote.length,1,'a pending mailbox mutation survives another account configuration change');
  assert.equal(remote[0].args.accountId,'a');assert.equal(remote[0].args.force,true);
});
