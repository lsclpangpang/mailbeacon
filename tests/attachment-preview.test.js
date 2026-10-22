const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const api=fs.existsSync(require('node:path').join(__dirname,'../ui/attachment-center.js'))?require('../ui/attachment-center.js'):{};
function create(options){assert.equal(typeof api.createPreview,'function','preview must own request identity and cancellation');return api.createPreview(options);}
const item=id=>({source:{accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:4,uid:id},partId:0,rawHash:'hash'+id,parserVersion:1,filename:'a.png',mime:'image/png'});
test('AT08: late preview A cannot overwrite B; each request sends the expected content identity',async()=>{
 const calls=[],pending=[],seen=[];
 const p=create({invoke:(command,args)=>{calls.push([command,args]);return command==='preview_attachment'?new Promise(resolve=>pending.push(resolve)):Promise.resolve();},onChange:x=>seen.push(x),randomId:(()=>{let n=0;return()=>String(++n);})()});
 const a=p.open(item(1));await Promise.resolve();
 const b=p.open(item(2));await Promise.resolve();await Promise.resolve();
 pending[1]({kind:'image',dataUrl:'data:image/png;base64,Qg=='});await b;
 pending[0]({kind:'image',dataUrl:'data:image/png;base64,QQ=='});await a;
 assert.equal(seen.at(-1).result.dataUrl,'data:image/png;base64,Qg==');
 assert.deepEqual(calls.filter(x=>x[0]==='preview_attachment')[0][1].item,item(1));
 assert.ok(calls.some(x=>x[0]==='cancel_attachment_preview'&&x[1].requestId==='1'));
});
test('AT08: closing a downloading preview never reopens it on completion',async()=>{
 let resolve;const seen=[];
 const p=create({invoke:(cmd)=>cmd==='preview_attachment'?new Promise(r=>resolve=r):Promise.resolve(),onChange:x=>seen.push(x),randomId:()=> 'one'});
 const loading=p.open(item(1));await Promise.resolve();await p.close();
 resolve({kind:'image',dataUrl:'data:image/png;base64,AA=='});await loading;
 assert.equal(seen.at(-1).status,'closed');assert.equal(seen.some(x=>x.status==='ready'),false);
});
test('AT12: an old native close event cannot close a newer preview',async()=>{
 const seen=[];let n=0;
 const p=create({invoke:async()=>({kind:'nativePdf'}),onChange:x=>seen.push(x),randomId:()=>String(++n)});
 await p.open(item(1));await p.open(item(2));p.nativeClosed({requestId:'1'});
 assert.equal(seen.at(-1).status,'ready');p.nativeClosed({requestId:'2'});assert.equal(seen.at(-1).status,'closed');
});

test('AT04: a cancelled collection cannot reset the control for its replacement',async t=>{
 const handlers=new Map(),elements=new Map(),collections=[];
 const container={
  innerHTML:'',
  addEventListener(type,handler,{signal}){
   handlers.set(type,handler);
   signal.addEventListener('abort',()=>handlers.delete(type),{once:true});
  },
  querySelector(selector){
   if(!elements.has(selector))elements.set(selector,{value:selector==='[data-center-filter="kind"]'?'all':'',textContent:'',innerHTML:'',hidden:false});
   return elements.get(selector);
  },
 };
 const button={
  dataset:{centerAction:'collect'},isConnected:true,textContent:'补充本地附件',
  closest(selector){return selector==='[data-center-action]'?this:null;},
 };
 const center=api.mount({
  container,targets:[{accountId:'a',receiveEpoch:1,folder:'INBOX'}],accounts:[],
  invoke:async()=>({items:[],coverage:[],hasMore:false,nextCursor:null}),
  onCollect:(targets,signal)=>new Promise(resolve=>collections.push({targets,signal,resolve})),
 });
 t.after(()=>{center.dispose();for(const collection of collections)collection.resolve();});
 const click=()=>handlers.get('click')({target:button});
 await Promise.resolve();
 click(); // A is waiting for its current message read.
 click(); // Cancel A while its read is still pending.
 click(); // Start B before A settles.
 assert.equal(collections.length,2);
 assert.equal(collections[0].signal.aborted,true);
 assert.equal(collections[1].signal.aborted,false);
 assert.equal(button.textContent,'取消补充');
 collections[0].resolve();
 await new Promise(setImmediate);
 assert.equal(button.textContent,'取消补充','A completion must preserve B cancellation control');
 assert.equal(collections[1].signal.aborted,false);
 click();
 assert.equal(collections.length,2,'the visible cancel control must cancel B without starting another collection');
 assert.equal(collections[1].signal.aborted,true);
 assert.equal(button.textContent,'补充本地附件');
});
