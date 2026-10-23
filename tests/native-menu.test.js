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
const composeActions = ['reply','reply-all','forward'];
const originalMailActions = mailActions.filter(id => !composeActions.includes(id));

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
  for(const change of ['missing','partial','error']) {
    const {S,message}=sample();
    if(change==='missing') S.msg=null;
    if(change==='partial') S.msg={partial:true,text:'partial'};
    if(change==='error') S.msgError='正文加载失败';
    const items=states(S,message);
    for(const id of composeActions) assert.equal(items[id].enabled,false,`${change}: ${id}`);
    assert.equal(items.trash.enabled,true,'loading does not block independent mail operations');
  }
});

test('complete cached bodies permit composing and online flag validation, while other source mutations stay disabled', () => {
  for(const change of ['offline','error','connecting','local-message','local-body','syncing','sync-error']) {
    const {S,message}=sample();
    if(['offline','error','connecting'].includes(change)) S.live.a.state=change;
    if(change==='local-message') message.localOnly=true;
    if(change==='local-body') S.msg.localOnly=true;
    if(change==='syncing') Object.assign(S.msg,{localOnly:true,syncing:true});
    if(change==='sync-error') Object.assign(S.msg,{localOnly:true,syncError:'标记已读失败'});
    const items=states(S,message);
    for(const id of composeActions) assert.equal(items[id].enabled,true,`${change}: ${id}`);
    for(const id of originalMailActions) {
      const canValidate = ['toggle-seen','toggle-flagged'].includes(id) && !['offline','error','connecting'].includes(change);
      assert.equal(items[id].enabled,canValidate,`${change}: ${id}`);
    }
  }
});

test('cached flag commands require a complete current source identity before offering online validation', () => {
  for (const field of ['receiveEpoch','uidValidity','uid']) {
    const {S,message}=sample(); message.localOnly=true; S.msg.localOnly=true;
    message[field]=S.sel[field]=0;
    for (const id of ['toggle-seen','toggle-flagged']) assert.equal(states(S,message)[id].enabled,false,field);
  }
});

test('cached reply eligibility retains selection, body and interaction guards', () => {
  for(const change of ['busy','batch-mode','missing-account','missing-selection','partial','missing-body','error','modal','pending']) {
    const {S,message}=sample();
    message.localOnly=true; S.msg.localOnly=true; S.live.a.state='offline';
    if(change==='busy') S.moving.add('a|INBOX|7');
    if(change==='batch-mode') S.batchMode=true;
    if(change==='missing-account') S.accounts=[];
    if(change==='missing-selection') S.sel=null;
    if(change==='partial') S.msg.partial=true;
    if(change==='missing-body') S.msg=null;
    if(change==='error') S.msgError='正文加载失败';
    const items=states(S,message,{modal:change==='modal',pending:change==='pending'});
    for(const id of mailActions) assert.equal(items[id].enabled,false,`${change}: ${id}`);
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

function app({native=true,platform='macos',realDraft=false,preview=false,invokeImpl=async()=>null,actionImpl=()=>{},listenImpl=()=>{}}={}) {
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
  const appMenu={hidden:true,attributes:{},setAttribute(name,value){this.attributes[name]=value;},getBoundingClientRect:()=>({left:10.5,top:11.5,bottom:41.5})};
  const document={activeElement:focus,body:{},addEventListener:(name,fn)=>{listeners[name]=fn;},querySelector(selector){
    if(selector==='#app-menu') return appMenu;
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
  window.MailBeaconAttachments={previewActive:()=>preview};
  window.MailBeaconPlatform=require('../ui/platform.js').forPlatform(platform);
  const context=vm.createContext({window,document,console,Date:class extends Date{static now(){return now;}},setTimeout:(fn,ms)=>{timers.set(++timerId,{fn,ms});return timerId;},clearTimeout:id=>timers.delete(id),MutationObserver:class{observe(){}},capture:(...args)=>{actions.push(args);return actionImpl(...args);}});
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
    ${realDraft ? '' : 'draftFor = mode => ({mode});'}
    globalThis.api={S,initNativeMenu,syncNativeMenu,runNativeMenuAction,nativeMenuStates,openShortcuts,closeOverlay,originalHistory,openApplicationMenu:typeof openApplicationMenu==='function'?openApplicationMenu:null};
  `,context);
  const trayRegistration = source.match(/^  await listen\("tray-compose".*$/m)[0].trim().replace(/^await /, '');
  vm.runInContext(trayRegistration,context);
  const {S,message}=sample(); Object.assign(context.api.S,S); context.api.S.lists.set('a|INBOX',[message]);
  const flush=async()=>{for(let i=0;i<8;i++)await Promise.resolve();};
  return {...context.api,calls,actions,listeners,nativeListeners,overlay,historyRecords,timers,appMenu,flush,setNow:value=>{now=value;},setActiveElement:value=>{document.activeElement=value;}};
}

test('compact application menu is available on native Windows including the welcome screen', async () => {
  for(const platform of ['windows','macos','linux']) {
    const a=app({platform}); a.S.accounts=[];
    await a.initNativeMenu(); await a.flush();
    assert.equal(a.appMenu.hidden,platform!=='windows',platform);
  }
  const preview=app({platform:'windows',native:false});
  await preview.initNativeMenu();
  assert.equal(preview.appMenu.hidden,true,'no dead native menu button in browser previews');
});

test('application menu refreshes availability before opening at CSS coordinates and blocks duplicate popups', async () => {
  let closePopup;
  const a=app({platform:'windows',invokeImpl:cmd=>cmd==='popup_native_menu'?new Promise(resolve=>{closePopup=resolve;}):null});
  await a.initNativeMenu(); await a.flush(); a.calls.length=0;
  a.S.filter='unread';
  const opening=a.openApplicationMenu(a.appMenu); await a.flush();
  assert.deepEqual(a.calls.map(c=>c.cmd),['set_native_menu_state','popup_native_menu']);
  assert.equal(byId(a.calls[0].input.states)['filter-unread'].checked,true);
  assert.equal(JSON.stringify(a.calls[1].input),JSON.stringify({x:10.5,y:45.5}));
  assert.equal(a.appMenu.attributes['aria-expanded'],'true');
  await a.openApplicationMenu(a.appMenu);
  assert.equal(a.calls.length,2);
  closePopup(); await opening;
  assert.equal(a.appMenu.attributes['aria-expanded'],'false');
  assert.equal(a.actions.some(a=>a[0]==='restore-focus'),false,'do not steal the editor focus');
});

test('application menu reports failures and permits another attempt', async () => {
  const a=app({platform:'windows',invokeImpl:cmd=>{if(cmd==='popup_native_menu')throw new Error('popup unavailable');}});
  await a.initNativeMenu(); await a.flush();
  await assert.doesNotReject(a.openApplicationMenu(a.appMenu));
  assert.equal(a.appMenu.attributes['aria-expanded'],'false');
  assert.ok(a.actions.some(a=>a[0]==='failure'&&/popup unavailable/.test(a[1])));
  await a.openApplicationMenu(a.appMenu);
  assert.equal(a.calls.filter(c=>c.cmd==='popup_native_menu').length,2);
});

test('application menu does not invoke native popups on another platform or a browser preview', async () => {
  for(const options of [{platform:'macos'},{platform:'windows',native:false}]) {
    const a=app(options); await a.openApplicationMenu(a.appMenu);
    assert.equal(a.calls.length,0);
  }
});

test('application menu button click uses the native popup action', async () => {
  const a=app({platform:'windows'});
  await a.initNativeMenu(); await a.flush();
  a.appMenu.dataset={action:'app-menu'};
  const target={classList:{contains:()=>false},closest:selector=>selector==='[data-action]'?a.appMenu:null};
  await a.listeners.click({target});
  assert.equal(a.calls.filter(c=>c.cmd==='popup_native_menu').length,1);
});

test('mouse opening application menu preserves the editor selection without blocking unrelated buttons', () => {
  const a=app({platform:'windows'});
  let prevented=0;
  a.listeners.mousedown({target:{closest:selector=>selector.includes('[data-action="app-menu"]')?a.appMenu:null},preventDefault(){prevented++;}});
  assert.equal(prevented,1);
  a.listeners.mousedown({target:{closest:()=>null},preventDefault(){prevented++;}});
  assert.equal(prevented,1);
});

test('keyboard menu opening restores the last visible editable target before native editing commands', async () => {
  for(const available of [true,false]) {
    const order=[];
    const a=app({platform:'windows',invokeImpl:cmd=>order.push(cmd)});
    const input={isConnected:available,disabled:false,matches:()=>true,getClientRects:()=>[{}],closest:()=>null,focus(){order.push('focus-input');}};
    a.listeners.focusin({target:input});
    a.setActiveElement(a.appMenu);
    await a.openApplicationMenu(a.appMenu);
    assert.equal(order.includes('focus-input'),available);
    if(available) assert.ok(order.indexOf('focus-input')<order.indexOf('popup_native_menu'));
  }
});

test('application menu retains native Enter and Space activation in batch mode', () => {
  const a=app({platform:'windows'}); a.S.batchMode=true;
  for(const key of ['Enter',' ']) {
    let prevented=false;
    a.listeners.keydown({key,target:{closest:selector=>selector==='#app-menu'?a.appMenu:null},preventDefault(){prevented=true;}});
    assert.equal(prevented,false,key);
  }
});

test('native event executes once; webview Cmd+N does not execute a duplicate', async () => {
  const a=app(); await a.initNativeMenu(); await a.flush();
  a.listeners.keydown({key:'n',metaKey:true,target:{closest:()=>null},preventDefault(){}});
  assert.equal(a.actions.length,0);
  await a.nativeListeners['native-menu']({payload:'compose'});
  assert.deepEqual(a.actions.map(x=>x[0]),['compose']);
});

test('native accelerators retain default delivery without running WebView actions', async () => {
  const a=app(); await a.initNativeMenu(); await a.flush();
  for(const extra of [{key:'n'},{key:'N',shiftKey:true},{key:'f'},{key:','}]) {
    let prevented=false;
    a.listeners.keydown({metaKey:true,target:{closest:()=>null},preventDefault(){prevented=true;},...extra});
    assert.equal(prevented,false,extra.key);
  }
  assert.equal(a.actions.length,0,'native menu events own these actions');
});

const windowsShortcuts = [
  { key:'n', action:'compose' },
  { key:'N', shiftKey:true, action:'check-mail' },
  { key:'f', action:'focus-search' },
  { key:',', action:'settings' },
];
function pressWindowsShortcut(a, extra) {
  let prevented=false;
  const completion=a.listeners.keydown({ctrlKey:true,target:{closest:()=>null},preventDefault(){prevented=true;},...extra});
  return { prevented, completion };
}

test('native Windows handles the four application shortcuts once when WebView2 delivers keydown', async () => {
  const a=app({platform:'windows'}); await a.initNativeMenu(); await a.flush();
  for(const {action,...event} of windowsShortcuts) {
    const before=a.actions.length;
    const pressed=pressWindowsShortcut(a,event);
    assert.equal(pressed.prevented,true,action);
    await pressed.completion;
    assert.deepEqual(a.actions.slice(before).map(x=>x[0]),[action]);
  }
});

test('native Windows rejects repeat and invalid application shortcut modifiers', async () => {
  const a=app({platform:'windows'}); await a.initNativeMenu(); await a.flush();
  for(const {action,...event} of windowsShortcuts) {
    const repeated=pressWindowsShortcut(a,{...event,repeat:true});
    assert.equal(repeated.prevented,true,`prevent browser repeat: ${action}`);
    await repeated.completion;
    for(const invalid of [{metaKey:true},{ctrlKey:false,metaKey:true},{altKey:true},{isComposing:true},{keyCode:229}]) {
      const pressed=pressWindowsShortcut(a,{...event,...invalid});
      assert.equal(pressed.prevented,false,`${action}: ${JSON.stringify(invalid)}`);
      await pressed.completion;
    }
  }
  for(const event of [{key:'f',shiftKey:true},{key:',',shiftKey:true},{key:'c'},{key:'v'},{key:'x'},{key:'z'}]) {
    assert.equal(pressWindowsShortcut(a,event).prevented,false,JSON.stringify(event));
  }
  assert.equal(a.actions.length,0);
});

test('native Windows shortcuts honor modal, account and checking availability', async () => {
  for(const unavailable of ['modal','empty','checking']) {
    const a=app({platform:'windows'}); await a.initNativeMenu(); await a.flush();
    if(unavailable==='modal') a.overlay.innerHTML='<form>unsaved draft</form>';
    if(unavailable==='empty') a.S.accounts=[];
    if(unavailable==='checking') a.S.checking=true;
    const shortcuts=windowsShortcuts.filter(({action})=>unavailable==='modal'||(unavailable==='empty'?action!=='settings':action==='check-mail'));
    for(const {action,...event} of shortcuts) {
      const pressed=pressWindowsShortcut(a,event);
      assert.equal(pressed.prevented,true,`${unavailable}: ${action}`);
      await pressed.completion;
    }
    assert.equal(a.actions.length,0,unavailable);
    if(unavailable==='modal') assert.equal(a.overlay.innerHTML,'<form>unsaved draft</form>');
    if(unavailable==='empty') {
      await pressWindowsShortcut(a,{key:','}).completion;
      assert.deepEqual(a.actions.map(x=>x[0]),['settings']);
    }
  }
});

test('native Windows keyboard and menu dispatch share the pending-action guard', async () => {
  let finish;
  const a=app({platform:'windows',actionImpl:action=>action==='check-mail'?new Promise(resolve=>{finish=resolve;}):undefined});
  await a.initNativeMenu(); await a.flush();
  const checking=pressWindowsShortcut(a,{key:'N',shiftKey:true});
  assert.equal(checking.prevented,true);
  for(const {action,...event} of windowsShortcuts) {
    const pressed=pressWindowsShortcut(a,event);
    assert.equal(pressed.prevented,true,action);
    await pressed.completion;
  }
  await a.nativeListeners['native-menu']({payload:'check-mail'});
  assert.deepEqual(a.actions.map(x=>x[0]),['check-mail']);
  finish(); await checking.completion;
  await pressWindowsShortcut(a,{key:','}).completion;
  assert.deepEqual(a.actions.map(x=>x[0]),['check-mail','settings']);
});

test('native Windows keyboard preserves attachment preview and reports action failures', async () => {
  const preview=app({platform:'windows',preview:true}); await preview.initNativeMenu(); await preview.flush();
  for(const {action,...event} of windowsShortcuts) await pressWindowsShortcut(preview,event).completion;
  assert.equal(preview.actions.length,0);
  const broken=app({platform:'windows',actionImpl:action=>{if(action==='compose')throw new Error('compose error');}});
  await broken.initNativeMenu(); await broken.flush();
  await assert.doesNotReject(Promise.resolve(pressWindowsShortcut(broken,{key:'n'}).completion));
  const failure=broken.actions.find(x=>x[0]==='failure');
  assert.ok(failure,'report the shortcut failure through existing feedback');
  assert.match(failure[1],/compose error/);
});

test('browser preview keeps compose, check-mail, search, and settings shortcuts', async () => {
  const a=app({native:false}); await a.initNativeMenu();
  for(const extra of [{key:'n'},{key:'N',shiftKey:true},{key:'f'},{key:','}]) {
    let prevented=false;
    a.listeners.keydown({metaKey:true,target:{closest:()=>null},preventDefault(){prevented=true;},...extra});
    assert.equal(prevented,true,extra.key);
  }
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

test('native reply event builds a draft from the complete cached message while offline', async () => {
  const a=app({realDraft:true});
  a.S.live.a.state='offline';
  a.S.lists.get('a|INBOX')[0].localOnly=true;
  a.S.msg={localOnly:true,syncError:'标记已读失败',uidValidity:9,from:{name:'Sender',addr:'sender@example.com'},to:[{addr:'a@example.com'}],cc:[],subject:'Cached subject',text:'Cached body',date:1000,messageId:'<cached@example.com>',references:['<parent@example.com>']};
  await a.initNativeMenu(); await a.flush();
  await a.nativeListeners['native-menu']({payload:'reply'});
  assert.equal(a.actions.length,1);
  const [action,draft]=a.actions[0];
  assert.equal(action,'compose');
  assert.equal(draft.to,'Sender <sender@example.com>');
  assert.equal(draft.subject,'Re: Cached subject');
  assert.equal(draft.quote.text,'Cached body');
  assert.equal(draft.inReplyTo,'<cached@example.com>');
  assert.equal(draft.quoteSource.uidValidity,9);
  assert.equal(draft.quoteSource.receiveEpoch,2);
  assert.deepEqual(a.calls.map(call=>call.cmd).filter(cmd=>cmd!=='set_native_menu_state'),[],'opening a reply must not fetch or mark the source message');
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
test('native cached flag commands are independent of missing or partial body loading', () => {
  for (const body of [null, {partial:true}, {localOnly:true}]) {
    const {S,message}=sample(); message.localOnly=true; S.msg=body;
    const items=states(S,message);
    assert.equal(items['toggle-flagged'].enabled,true);
    assert.equal(items['toggle-seen'].enabled,true);
  }
});
