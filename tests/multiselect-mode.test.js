const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const M = require('../ui/mail-selection.js');
function app() {
  const events={}, winEvents={}, calls=[], menus=[], rafs=new Map(); let raf=0;
  const overlay={innerHTML:''};
  const rows={scrollTop:0,scrollHeight:20000,clientHeight:300,isConnected:true,classList:{add(){},remove(){}},getBoundingClientRect:()=>({top:100,bottom:400}),setPointerCapture(){},releasePointerCapture(){},focus(){},querySelectorAll:()=>[],querySelector:()=>null};
  const list={};
  const document={activeElement:null,addEventListener:(name,fn)=>{events[name]=fn;},querySelector:selector=>selector==='#overlay'?overlay:selector==='.rows'?rows:selector==='#list'?list:null,querySelectorAll:()=>[]};
  const mods={MailBeaconSelection:M,MailBeaconNativeMenu:require('../ui/native-menu.js'),MailBeaconSearch:require('../ui/mail-search.js'),MailBeaconOperations:require('../ui/mail-operations.js'),MailBeaconViews:require('../ui/views.js'),MailBeaconPrefs:require('../ui/prefs.js'),MailBeaconBodyCache:require('../ui/body-cache.js'),MailBeaconRules:require('../ui/rules.js'),MailBeaconThreads:require('../ui/threads.js'),MailBeaconExtras:require('../ui/compose-extras.js')};
  const context=vm.createContext({window:{...mods,addEventListener:(name,fn)=>{winEvents[name]=fn;},__TAURI__:{core:{invoke:async(cmd,args)=>calls.push([cmd,args])},event:{listen:async()=>{}}}},document,console,setTimeout,clearTimeout,requestAnimationFrame:fn=>{rafs.set(++raf,fn);return raf;},cancelAnimationFrame:id=>rafs.delete(id),capture:(...args)=>calls.push(args),captureMenu:entries=>menus.push(entries)});
  const source=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
  vm.runInContext(source.slice(0,source.indexOf('(async function boot()'))+`
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = function() {};
    requestServerSearch = () => capture('search');
    showMenu = (_anchor,entries) => captureMenu(entries);
    openMessage = sel => capture('open',sel);
    paintBatchSelection = function() {};
    focusBatchRow = index => capture('focus',index);
    globalThis.api={S,rowHtml,bulkBar,selectMailRow,clearBatch,selectionGroups,onKey,
      enterBatchMode:typeof enterBatchMode==='function'?enterBatchMode:null,
      reconcileBatchScope:typeof reconcileBatchScope==='function'?reconcileBatchScope:null,
      startBatchDrag:typeof startBatchDrag==='function'?startBatchDrag:null,
      moveBatchDrag:typeof moveBatchDrag==='function'?moveBatchDrag:null,
      finishBatchDrag:typeof finishBatchDrag==='function'?finishBatchDrag:null,
      onSearchOptionsChange:typeof onSearchOptionsChange==='function'?onSearchOptionsChange:null};
  `,context);
  const S=context.api.S; S.accounts=[{id:'a',email:'a@example.com',receiveEpoch:1}];S.acct='a';S.ui={};S.folders.a=[{path:'INBOX',role:'inbox'},{path:'Trash',role:'trash'}];S.live.a={state:'online'};
  const mails=Array.from({length:200},(_,i)=>({accountId:'a',folder:'INBOX',receiveEpoch:1,uidValidity:7,uid:i+1,subject:`mail ${i+1}`,seen:false,date:201-i}));S.lists.set('a|INBOX',mails);S.sel={...mails[0]};S.msg={text:'existing body'};
  const target=(index=0)=>({classList:{contains:()=>false},dataset:{open:`a|${index+1}|INBOX`},closest(selector){if(selector==='.rows')return rows;if(selector==='#list')return list;if(selector.includes('[data-open]'))return this;return null;},focus(){document.activeElement=this;}});
  const event=(index=0,extra={})=>({target:target(index),key:'',pointerType:'mouse',pointerId:1,button:0,isPrimary:true,clientX:100,clientY:110+index*66,preventDefault(){this.prevented=true;},stopPropagation(){},...extra});
  return {...context.api,calls,events,winEvents,menus,overlay,rows,mails,event,target,rafs};
}

test('normal reading has no checkbox and keeps selected-reader appearance',()=>{
 const a=app();const html=a.rowHtml({latest:a.mails[0],count:1,unread:1},false,false);
 assert.doesNotMatch(html,/mail-checkbox|data-select-mail=/);assert.match(html,/ sel/);
});
test('explicit multi-select has checkboxes, an empty toolbar and separate reader appearance',()=>{
 const a=app();assert.equal(typeof a.enterBatchMode,'function');a.enterBatchMode();
 assert.equal(a.S.batchMode,true);assert.equal(a.S.batch.size,0);
 const html=a.rowHtml({latest:a.mails[0],count:1,unread:1},false,false);
 assert.match(html,/mail-checkbox/);assert.doesNotMatch(html,/ row[^>]* sel/);
 assert.match(a.bulkBar(),/已选 0 封/);assert.match(a.bulkBar(),/完成/);assert.match(a.bulkBar(),/全选已加载/);
});
test('list menu enters multi-select without selecting all messages',async()=>{
 const a=app();const button={dataset:{action:'list-menu'},disabled:false};await a.events.click({target:{classList:{contains:()=>false},closest:selector=>selector==='[data-action]'?button:null}});
 const entry=a.menus[0].find(x=>x.label==='多选邮件');assert.ok(entry);entry.run();
 assert.equal(a.S.batchMode,true);assert.equal(a.S.batch.size,0);
});
test('clicking rows toggles selection in multi-select without opening or marking mail read',async()=>{
 const a=app();a.enterBatchMode();await a.events.click(a.event(2));assert.equal(a.S.batch.has(M.key(a.mails[2])),true);
 await a.events.click(a.event(2));assert.equal(a.S.batch.size,0);assert.equal(a.mails[2].seen,false);assert.equal(a.calls.some(x=>x[0]==='open'),false);
});
test('a normal row click still opens the message',async()=>{
 const a=app();await a.events.click(a.event(2));assert.equal(a.calls[0][0],'open');assert.equal(a.calls[0][1].uid,3);
});
test('clear keeps multi-select active while Escape exits even with zero selections',()=>{
 const a=app();a.enterBatchMode();a.selectMailRow(M.key(a.mails[0]));a.clearBatch({exit:false});assert.equal(a.S.batchMode,true);
 a.onKey(a.event(0,{key:'Escape'}));assert.equal(a.S.batchMode,false);assert.equal(a.S.sel.uid,1);
});
test('scope changes exit mode but same-scope refresh only prunes missing identities',()=>{
 const a=app();a.enterBatchMode();a.selectMailRow(M.key(a.mails[0]));a.S.lists.set('a|INBOX',a.mails.slice(1));a.reconcileBatchScope();
 assert.equal(a.S.batchMode,true);assert.equal(a.S.batch.size,0);a.S.filter='unread';a.reconcileBatchScope();assert.equal(a.S.batchMode,false);
});
test('Cmd+A enters multi-select and Space/Enter only toggle the focused row',()=>{
 const a=app();a.onKey(a.event(0,{key:'a',metaKey:true}));assert.equal(a.S.batchMode,true);assert.equal(a.S.batch.size,200);
 a.onKey(a.event(1,{key:' '}));assert.equal(a.S.batch.has(M.key(a.mails[1])),false);
 a.onKey(a.event(1,{key:'Enter'}));assert.equal(a.S.batch.has(M.key(a.mails[1])),true);assert.equal(a.calls.some(x=>x[0]==='open'),false);
});
test('j/k move focus and reader letter shortcuts never operate a stale reader in mode',()=>{
 const a=app();a.enterBatchMode();a.onKey(a.event(2,{key:'j'}));assert.deepEqual(a.calls.at(-1),['focus',3]);
 for(const key of ['r','a','f','e','s','u','Delete'])a.onKey(a.event(2,{key}));
 assert.equal(a.calls.some(x=>x[0]==='open'),false);assert.equal(a.S.batch.size,0);
});
test('drag starts after threshold, grows and shrinks from a stable snapshot',()=>{
 const a=app();a.enterBatchMode();a.S.batch.add(M.key(a.mails[9]));a.startBatchDrag(a.event(1));a.moveBatchDrag(a.event(1,{clientY:178}));assert.equal(a.S.batch.size,1);
 a.moveBatchDrag(a.event(4));assert.deepEqual([...a.S.batch].sort(),[9,1,2,3,4].map(i=>M.key(a.mails[i])).sort());
 a.moveBatchDrag(a.event(2));assert.deepEqual([...a.S.batch].sort(),[9,1,2].map(i=>M.key(a.mails[i])).sort());
 a.finishBatchDrag(false);assert.equal(a.S.batch.size,3);
});
test('dragging from a selected row deselects range and cancellation restores the snapshot',()=>{
 const a=app();a.enterBatchMode();a.S.batch=M.all(a.selectionGroups().slice(0,6));a.startBatchDrag(a.event(1));a.moveBatchDrag(a.event(3));
 assert.equal(a.S.batch.size,3);a.finishBatchDrag(true);assert.equal(a.S.batch.size,6);
});
test('touch, right click and normal mode do not start selection drags',()=>{
 const a=app();for(const extra of [{},{pointerType:'touch'},{button:2}]){a.S.batchMode=!!Object.keys(extra).length;a.startBatchDrag(a.event(1,extra));a.moveBatchDrag(a.event(4));}
 assert.equal(a.S.batch.size,0);
});
test('dragging accounts for virtual scroll offset and a cancelled gesture cannot alter a new scope',()=>{
 const a=app();a.enterBatchMode();a.rows.scrollTop=6600;a.startBatchDrag(a.event(100,{clientY:110}));a.moveBatchDrag(a.event(103,{clientY:308}));assert.equal(a.S.batch.size,4);
 a.S.folder='Other';a.finishBatchDrag(true);a.reconcileBatchScope();assert.equal(a.S.batch.size,0);assert.equal(a.S.batchMode,false);
});

test('toolbar buttons keep native Space and Enter activation in multi-select',()=>{
 const a=app();a.enterBatchMode();
 for(const action of ['bulk-done','bulk-all','bulk-clear','bulk-trash'])for(const key of [' ','Enter']){
   const button={dataset:{action},closest:selector=>selector==='#list'?{}:selector==='button'?button:null};
   const event=a.event(0,{target:button,key});a.onKey(event);assert.equal(event.prevented,undefined,`${action}: ${key}`);
 }
});
test('pointerup uses its final position and suppresses only its tail click',async()=>{
 const a=app();a.enterBatchMode();a.startBatchDrag(a.event(1));a.events.pointerup(a.event(3));assert.equal(a.S.batch.size,3);
 await a.events.click(a.event(3,{detail:1}));assert.equal(a.S.batch.size,3,'tail click must not toggle endpoint');
 a.startBatchDrag(a.event(3));a.events.pointerup(a.event(3));await a.events.click(a.event(3,{detail:1}));assert.equal(a.S.batch.size,2,'new real click must work');
});
test('cancel, lost capture, blur and overlay interruption restore a drag snapshot',()=>{
 for(const reason of ['pointercancel','lostpointercapture','blur','overlay']) {
   const a=app();a.enterBatchMode();a.S.batch.add(M.key(a.mails[8]));a.startBatchDrag(a.event(1));a.moveBatchDrag(a.event(3));
   if(reason==='blur')a.winEvents.blur();else if(reason==='overlay'){a.overlay.innerHTML='<form>edit</form>';a.moveBatchDrag(a.event(4));}else a.events[reason](a.event(3));
   assert.deepEqual([...a.S.batch],[M.key(a.mails[8])],reason);
 }
});
test('auto-scroll moves across virtual rows without rebuilding the list',()=>{
 const a=app();a.enterBatchMode();a.rows.scrollTop=6600;a.startBatchDrag(a.event(100,{clientY:110}));a.moveBatchDrag(a.event(104,{clientY:399}));
 const before=a.S.batch.size;
 for(let i=0;i<12;i++){const [id,fn]=a.rafs.entries().next().value;a.rafs.delete(id);fn();}
 assert.ok(a.rows.scrollTop>6600);assert.ok(a.S.batch.size>before);assert.equal(a.calls.some(x=>x[0]==='open'),false);a.finishBatchDrag(false);
});
test('zero-selected mode disables native single-message actions',()=>{
 const a=app();a.enterBatchMode();const menu=require('../ui/native-menu.js').states(a.S,{message:a.mails[0]});
 for(const id of ['reply','forward','archive','trash','toggle-seen','toggle-flagged','download-message'])assert.equal(menu.find(x=>x.id===id).enabled,false,id);
});

for (const [label,target] of [
  ['search range',{id:'search-scope',value:'all'}],
  ['excluded folders',{id:'search-include-excluded',checked:true}],
  ['unread filter',{dataset:{searchFilter:'unreadOnly'},type:'checkbox',checked:true}],
  ['flagged filter',{dataset:{searchFilter:'flaggedOnly'},type:'checkbox',checked:true}],
  ['attachment filter',{dataset:{searchFilter:'attachmentOnly'},type:'checkbox',checked:true}],
  ['sender',{dataset:{searchFilter:'sender'},value:'alice'}],
  ['start date',{dataset:{searchFilter:'after'},value:'2026-10-01'}],
  ['end date',{dataset:{searchFilter:'before'},value:'2026-10-22'}],
]) test(`changing ${label} immediately exits multi-select and cancels its drag`,()=>{
 const a=app();a.S.searching=true;a.enterBatchMode();a.S.batch.add(M.key(a.mails[8]));
 a.startBatchDrag(a.event(1));a.moveBatchDrag(a.event(3));assert.ok(a.rafs.size);
 assert.equal(typeof a.onSearchOptionsChange,'function');a.onSearchOptionsChange({target});
 assert.equal(a.S.batchMode,false);assert.equal(a.S.batch.size,0);assert.equal(a.rafs.size,0);
 a.moveBatchDrag(a.event(5));assert.equal(a.S.batch.size,0);assert.equal(a.calls.at(-1)[0],'search');
});

test('opening search exits a selection made under the normal list filter',()=>{
 const a=app();a.S.filter='unread';a.enterBatchMode();a.S.batch.add(M.key(a.mails[1]));
 a.S.searching=true;a.reconcileBatchScope();assert.equal(a.S.batchMode,false);assert.equal(a.S.batch.size,0);
});
