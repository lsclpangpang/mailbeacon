const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const menu = require('../ui/native-menu.js');

function sample() {
  const message = { accountId:'a', receiveEpoch:2, folder:'INBOX', uidValidity:9, uid:7, seen:true, flagged:false };
  const S = { accounts:[{id:'a',receiveEpoch:2,email:'a@example.com'}], acct:'a', live:{a:{state:'online'}}, folders:{a:[{path:'INBOX',role:'inbox'},{path:'Archive',role:'archive'},{path:'Trash',role:'trash'}]}, sel:{...message}, msg:{text:'正文'}, batch:new Set(), moving:new Set(), downloading:new Set(), filter:'all', searching:false, ui:{collapsed:false}, threads:false, lastOperation:null };
  return { S, message };
}
const byId = states => Object.fromEntries(states.map(s => [s.id,s]));
const states = (S,message,extra={}) => byId(menu.states(S,{message,now:1000,...extra}));
const mailActions = ['reply','reply-all','forward','archive','move','trash','toggle-seen','toggle-flagged','download-message'];

test('native menu derives checked items and message labels from current state', () => {
  const {S,message} = sample();
  S.ui.collapsed=true; S.threads=true; S.filter='unread';
  const items=states(S,message);
  assert.equal(items['toggle-sidebar'].checked,false);
  assert.equal(items['toggle-threads'].checked,true);
  assert.equal(items['filter-all'].checked,false);
  assert.equal(items['filter-unread'].checked,true);
  assert.equal(items['filter-starred'].checked,false);
  assert.equal(items['toggle-seen'].text,'标为未读');
  assert.equal(items['toggle-flagged'].text,'加星标');
  assert.equal(items.trash.text,'删除邮件');
  for (const id of mailActions) assert.equal(items[id].enabled,true,id);
});

test('empty account state keeps help/settings available but disables mailbox commands', () => {
  const {S}=sample(); S.accounts=[]; S.sel=null;
  const items=states(S,null);
  for (const id of ['compose','check-mail','create-folder','search',...mailActions]) assert.equal(items[id].enabled,false,id);
  assert.equal(items.settings.enabled,true);
  assert.equal(items.shortcuts.enabled,true);
});

test('every app action is unavailable while a modal or native action is pending', () => {
  const {S,message}=sample();
  for(const extra of [{modal:true},{pending:true}]) {
    const items=states(S,message,extra);
    for(const item of Object.values(items)) assert.equal(item.enabled,false,item.id);
  }
});

test('partial, missing and failed bodies cannot be replied to or forwarded', () => {
  for(const body of [null,{partial:true,text:'partial'}]) {
    const {S,message}=sample(); S.msg=body;
    const items=states(S,message);
    for(const id of ['reply','reply-all','forward']) assert.equal(items[id].enabled,false,id);
    assert.equal(items.trash.enabled,true,'loading does not block independent mail operations');
  }
});

test('offline/local-only and busy messages cannot be mutated through native menus', () => {
  for(const change of ['offline','error','connecting','local-message','local-body','busy']) {
    const {S,message}=sample();
    if(['offline','error','connecting'].includes(change)) S.live.a.state=change;
    if(change==='local-message') message.localOnly=true;
    if(change==='local-body') S.msg.localOnly=true;
    if(change==='busy') S.moving.add('a|INBOX|7');
    for(const id of mailActions) assert.equal(states(S,message)[id].enabled,false,`${change}: ${id}`);
  }
});

test('batch selection and mailbox identity changes never target the old reader', () => {
  for(const change of ['batch','epoch','uidvalidity','selection']) {
    const {S,message}=sample();
    if(change==='batch') S.batch.add('another-message');
    if(change==='epoch') S.accounts[0].receiveEpoch=3;
    if(change==='uidvalidity') S.sel.uidValidity=10;
    if(change==='selection') S.sel.uid=8;
    for(const id of mailActions) assert.equal(states(S,message)[id].enabled,false,`${change}: ${id}`);
  }
});

test('archive, trash, local folders and POP3 match the supported operations', () => {
  const {S,message}=sample();
  S.folders.a=S.folders.a.filter(f=>f.role!=='archive'&&f.role!=='trash');
  assert.equal(states(S,message).archive.enabled,false);
  assert.equal(states(S,message).trash.enabled,false);
  S.folders.a.push({path:'INBOX',role:'trash'}); S.folders.a.shift();
  assert.equal(states(S,message).trash.text,'彻底删除…');
  assert.equal(states(S,message).trash.enabled,true);
  S.accounts[0].receiveProtocol='pop3';
  for(const id of ['archive','move','trash','toggle-flagged','create-folder']) assert.equal(states(S,message)[id].enabled,false,id);
  S.accounts[0].receiveProtocol='imap';
  for(const folder of ['__outbox','__local_drafts']) {
    S.sel.folder=message.folder=folder;
    for(const id of mailActions) assert.equal(states(S,message)[id].enabled,false,`${folder}: ${id}`);
  }
});

test('undo expires precisely, check-mail is busy, downloads do not repeat and filters do not pretend to affect active search', () => {
  const {S,message}=sample();
  S.lastOperation={undoToken:'token',expiresAt:1001};
  assert.equal(states(S,message)['undo-mail-operation'].enabled,true);
  assert.equal(states(S,message,{now:1001})['undo-mail-operation'].enabled,false);
  S.checking=true; S.downloading.add('a|INBOX|7'); S.searching=true;
  assert.equal(states(S,message)['check-mail'].enabled,false);
  assert.equal(states(S,message)['check-mail'].text,'正在收取…');
  assert.equal(states(S,message)['download-message'].enabled,false);
  for(const id of ['filter-all','filter-unread','filter-starred']) assert.equal(states(S,message)[id].enabled,false,id);
});

function app({native=true,invokeImpl=async()=>null,actionImpl=()=>{},listenImpl=()=>{}}={}) {
  const source=fs.readFileSync(require.resolve('../ui/app.js'),'utf8');
  const calls=[],actions=[],listeners={},nativeListeners={},timers=new Map();
  let timerId=0, now=1000;
  const overlay={innerHTML:''};
  const query={focus(){actions.push(['focus-search']);}};
  const focus={isConnected:true,focus(){actions.push(['restore-focus']);}};
  const close={focus(){document.activeElement=close;actions.push(['focus-close']);}};
  const content={focus(){document.activeElement=content;actions.push(['focus-content']);}};
  const historyRecords={innerHTML:''};
  const history={get isConnected(){return overlay.innerHTML.includes('operation-history');},querySelector:selector=>selector==='.operation-records'?historyRecords:close};
  overlay.querySelector=selector=>selector==='.operation-history'?history:null;
  const anchor={getBoundingClientRect:()=>({left:1,top:1,bottom:2})};
  const document={activeElement:focus,body:{},addEventListener:(name,fn)=>{listeners[name]=fn;},querySelector(selector){
    if(selector==='#overlay') return overlay;
    if(selector==='#q') return query;
    if(selector==='.operation-history') return history;
    if(selector==='#reader [data-action="move-menu"]') return anchor;
    if(selector==='.shortcut-sheet') return overlay.innerHTML.includes('shortcut-sheet')?{querySelectorAll:()=>[close,content]}:null;
    if(selector==='.shortcut-sheet [data-action="overlay-close"]') return close;
    return null;
  }};
  const window={MailBeaconNativeMenu:menu,MailBeaconSearch:require('../ui/mail-search.js'),MailBeaconSelection:require('../ui/mail-selection.js'),MailBeaconOperations:require('../ui/mail-operations.js'),MailBeaconViews:require('../ui/views.js'),MailBeaconPrefs:require('../ui/prefs.js'),MailBeaconBodyCache:require('../ui/body-cache.js'),MailBeaconRules:require('../ui/rules.js'),MailBeaconThreads:require('../ui/threads.js'),MailBeaconExtras:require('../ui/compose-extras.js'),__TAURI__:{core:{invoke:async(cmd,input)=>{calls.push({cmd,input});return invokeImpl(cmd,input);}},event:{listen:async(name,fn)=>{listenImpl(name);nativeListeners[name]=fn;return()=>{delete nativeListeners[name];};}}},addEventListener(){}};
  if(native) window.__TAURI_INTERNALS__={};
  const context=vm.createContext({window,document,console,Date:{now:()=>now},setTimeout:(fn,ms)=>{timers.set(++timerId,{fn,ms});return timerId;},clearTimeout:id=>timers.delete(id),MutationObserver:class{observe(){}},capture:(...args)=>{actions.push(args);return actionImpl(...args);}});
  vm.runInContext(source.slice(0,source.indexOf('(async function boot()'))+`
    const originalHistory = showOperationHistory;
    render = renderList = renderReader = function() {};
    hideReaderHint = function() {};
    toast = (text,error) => capture(error ? 'failure' : 'toast',text);
    openCompose = d => capture('compose',d);
    openSettings = () => capture('settings');
    openSendRecords = () => capture('send-records');
    showOperationHistory = () => capture('operation-history');
    openCreateFolder = (source,accountId) => capture('create-folder',source,accountId);
    checkMail = () => capture('check-mail');
    downloadMessage = source => capture('download-message',source);
    moveCurrent = (role,target,source) => capture('move',role,target,source);
    setFlag = (source,flag,value) => capture('flag',source,flag,value);
    showMoveMenu = (anchor,source) => capture('move-menu',source);
    undoLastOperation = () => capture('undo-mail-operation');
    draftFor = mode => ({mode});
    globalThis.api={S,initNativeMenu,syncNativeMenu,runNativeMenuAction,nativeMenuStates,openShortcuts,closeOverlay,originalHistory};
  `,context);
  const trayRegistration = source.match(/^  await listen\("tray-compose".*$/m)[0].trim().replace(/^await /, '');
  vm.runInContext(trayRegistration,context);
  const {S,message}=sample(); Object.assign(context.api.S,S); context.api.S.lists.set('a|INBOX',[message]);
  const flush=async()=>{for(let i=0;i<8;i++)await Promise.resolve();};
  return {...context.api,calls,actions,listeners,nativeListeners,overlay,historyRecords,timers,flush,setNow:value=>{now=value;}};
}

test('native event executes once; webview Cmd+N does not execute a duplicate', async () => {
  const a=app(); await a.initNativeMenu(); await a.flush();
  a.listeners.keydown({key:'n',metaKey:true,target:{closest:()=>null},preventDefault(){}});
  assert.equal(a.actions.length,0);
  await a.nativeListeners['native-menu']({payload:'compose'});
  assert.deepEqual(a.actions.map(x=>x[0]),['compose']);
});

test('browser preview keeps compose, check-mail, search, and settings shortcuts', async () => {
  const a=app({native:false}); await a.initNativeMenu();
  for(const extra of [{key:'n'},{key:'N',shiftKey:true},{key:'f'},{key:','}]) a.listeners.keydown({metaKey:true,target:{closest:()=>null},preventDefault(){},...extra});
  await a.flush();
  assert.deepEqual(a.actions.map(x=>x[0]),['compose','check-mail','focus-search','settings']);
  assert.equal(a.calls.length,0,'browser mock must not receive native IPC');
});

test('native event rechecks modal, no-account and batch state after the enabled snapshot', async () => {
  for(const change of ['modal','empty','batch']) {
    const a=app(); await a.initNativeMenu(); await a.flush();
    if(change==='modal') a.overlay.innerHTML='<form>unsaved</form>';
    if(change==='empty') a.S.accounts=[];
    if(change==='batch') a.S.batch.add('other');
    await a.runNativeMenuAction(change==='batch'?'trash':'compose');
    assert.equal(a.actions.length,0,change);
    if(change==='modal') assert.equal(a.overlay.innerHTML,'<form>unsaved</form>');
  }
});

test('native actions retain the full mailbox identity and reuse existing operations', async () => {
  const a=app();
  for(const action of ['reply','reply-all','forward','archive','move','trash','toggle-seen','toggle-flagged','download-message']) await a.runNativeMenuAction(action);
  const source=a.actions.find(x=>x[0]==='move')[3];
  assert.equal(source.receiveEpoch,2); assert.equal(source.uidValidity,9); assert.equal(source.uid,7);
  assert.deepEqual(a.actions.map(x=>x[0]),['compose','compose','compose','move','move-menu','move','flag','flag','download-message']);
  assert.equal(a.actions[6][2],'seen'); assert.equal(a.actions[6][3],false);
});

test('sync coalesces identical states and updates expired undo without rendering', async () => {
  const a=app(); a.S.lastOperation={undoToken:'token',expiresAt:1050};
  await a.initNativeMenu(); await a.flush();
  a.syncNativeMenu();a.syncNativeMenu(); await a.flush();
  assert.equal(a.calls.filter(x=>x.cmd==='set_native_menu_state').length,1);
  assert.equal(byId(a.calls[0].input.states)['undo-mail-operation'].enabled,true);
  a.setNow(1051);
  for(const timer of [...a.timers.values()]) timer.fn();
  await a.flush();
  assert.equal(a.calls.length,2);
  assert.equal(byId(a.calls[1].input.states)['undo-mail-operation'].enabled,false);
});

test('shortcut sheet is accessible, closes on Escape, and restores focus', () => {
  const a=app(); a.openShortcuts();
  assert.match(a.overlay.innerHTML,/aria-labelledby="shortcuts-title"/);
  assert.match(a.overlay.innerHTML,/Space/); assert.match(a.overlay.innerHTML,/输入框/);
  assert.equal(a.actions[0][0],'focus-close');
  a.listeners.keydown({key:'Escape',target:{closest:()=>null},preventDefault(){}});
  assert.equal(a.overlay.innerHTML,'');
  assert.equal(a.actions.at(-1)[0],'restore-focus');
});

test('system text editing shortcuts are untouched in text inputs', () => {
  const a=app();
  for(const key of ['a','z','c','v','x']) {
    let prevented=false;
    a.listeners.keydown({key,metaKey:true,target:{closest:selector=>selector.includes('input')?{}:null},preventDefault(){prevented=true;}});
    assert.equal(prevented,false,key);
  }
  assert.equal(a.actions.length,0);
});


test('opening operation history reserves its sheet before waiting and never replaces a newer composer', async () => {
  let finish;
  const a=app({invokeImpl:cmd=>cmd==='list_mail_operations'?new Promise(resolve=>{finish=resolve;}):Promise.resolve()});
  const loading=a.originalHistory();
  assert.match(a.overlay.innerHTML,/operation-history/,'reserve a modal before awaiting records');
  a.overlay.innerHTML='<section class="compose-sheet">unsaved draft</section>';
  finish([]); await loading;
  assert.equal(a.overlay.innerHTML,'<section class="compose-sheet">unsaved draft</section>');
});

test('pending native action rejects a second action until completion', async () => {
  let finish;
  const a=app({actionImpl:action=>action==='check-mail'?new Promise(resolve=>{finish=resolve;}):undefined});
  const checking=a.runNativeMenuAction('check-mail');
  await a.runNativeMenuAction('settings');
  assert.deepEqual(a.actions.map(x=>x[0]),['check-mail']);
  finish(); await checking;
  await a.runNativeMenuAction('settings');
  assert.deepEqual(a.actions.map(x=>x[0]),['check-mail','settings']);
});

test('state IPC is serialized and includes changes received while the previous update is pending', async () => {
  const completions=[];
  const a=app({invokeImpl:cmd=>cmd==='set_native_menu_state'?new Promise(resolve=>completions.push(resolve)):Promise.resolve()});
  await a.initNativeMenu(); await a.flush();
  a.S.filter='flagged'; a.syncNativeMenu(); await a.flush();
  assert.equal(a.calls.length,1);
  completions.shift()(); await a.flush();
  assert.equal(a.calls.length,2);
  assert.equal(byId(a.calls[1].input.states)['filter-starred'].checked,true);
  completions.shift()(); await a.flush();
});

test('native menu failures use the existing visible error feedback', async () => {
  const a=app({actionImpl:action=>{if(action==='compose')throw new Error('compose error');}});
  await a.initNativeMenu(); await a.flush();
  await a.nativeListeners['native-menu']({payload:'compose'});
  a.nativeListeners['native-menu-error']({payload:'打开链接失败'});
  assert.match(a.actions.find(x=>x[0]==='failure')[1],/compose error/);
  assert.equal(a.actions.at(-1)[1],'打开链接失败');
});


test('all-account create-folder selects an available IMAP account even when first account cannot create folders', async () => {
  for(const unavailable of ['pop3','offline']) {
    const a=app(); a.S.acct='all';
    a.S.accounts.push({id:'b',email:'b@example.com',receiveProtocol:'imap'});
    a.S.live.b={state:'online'};
    if(unavailable==='pop3') a.S.accounts[0].receiveProtocol='pop3'; else a.S.live.a.state='offline';
    assert.equal(byId(a.nativeMenuStates())['create-folder'].enabled,true,unavailable);
    await a.runNativeMenuAction('create-folder');
    assert.equal(a.actions[0][2],'b');
  }
});

test('menu listener failure is reported without rejecting app initialization and removes partial listeners', async () => {
  const a=app({listenImpl:name=>{if(name==='native-menu')throw new Error('listener unavailable');}});
  await assert.doesNotReject(a.initNativeMenu());
  assert.ok(a.actions.some(x=>x[0]==='failure'&&/listener unavailable/.test(x[1])));
  assert.equal(a.nativeListeners['native-menu-error'],undefined);
});

test('tray compose honors modal and empty-account guards', async () => {
  const a=app();
  a.overlay.innerHTML='<form>unsaved settings</form>';
  await a.nativeListeners['tray-compose']();
  assert.equal(a.actions.length,0);
  a.overlay.innerHTML=''; a.S.accounts=[];
  await a.nativeListeners['tray-compose']();
  assert.equal(a.actions.length,0);
});

test('keyboard users can tab into the scrolling shortcut region and back', () => {
  const a=app(); a.openShortcuts();
  assert.match(a.overlay.innerHTML,/class="shortcut-content"[^>]*tabindex="0"/);
  const press=extra=>a.listeners.keydown({key:'Tab',target:{closest:()=>null},preventDefault(){},...extra});
  press({}); assert.equal(a.actions.at(-1)[0],'focus-content');
  press({shiftKey:true}); assert.equal(a.actions.at(-1)[0],'focus-close');
  press({shiftKey:true}); assert.equal(a.actions.at(-1)[0],'focus-content');
});
