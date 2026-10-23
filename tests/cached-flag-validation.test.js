const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const source = { accountId: 'a', receiveEpoch: 2, folder: 'INBOX', uidValidity: 7, uid: 42 };
const body = { subject: 'Cached mail', text: 'Full cached body', attachments: [], uidValidity: 7 };
const snapshot = (localOnly, ref = source) => ({ source: ref, message: { ...body }, localOnly, readMarked: false, snapshotAt: 100 });
function setup(handler = async (_cmd, input) => snapshot(input.localOnly), operationStatus = 'succeeded', operationHandler = null) {
  const calls = [], notices = [], outcomes = [], overlay = { innerHTML: '' };
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search'), MailBeaconViews: require('../ui/views'),
      MailBeaconSelection: require('../ui/mail-selection'), MailBeaconOperations: require('../ui/mail-operations'),
      MailBeaconThreads: require('../ui/threads'),
      __TAURI__: { core: { invoke: async (cmd, input) => {
        calls.push({ cmd, input });
        if (cmd === 'execute_mail_operation' && operationHandler) return operationHandler(input.request);
        if (cmd === 'execute_mail_operation') return { items: input.request.items.map(ref => ({ source: ref, status: operationStatus })) };
        if (cmd === 'prefetch_message') return false;
        return handler(cmd, input);
      } }, event: { listen: async () => {} } },
    },
    document: { activeElement: null, querySelector: selector => selector === '#overlay' ? overlay : null, querySelectorAll: () => [], addEventListener() {} },
    console, setTimeout, clearTimeout, notice: (...args) => notices.push(args), outcome: result => outcomes.push(result),
  });
  const code = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(code.slice(0, code.indexOf('(async function boot()')) + `
    render=renderList=renderReader=renderSidebar=updateBadge=revealSelectedRow=refreshOperationSources=loadPreviews=function(){};
    toast=notice; showOperationOutcome=outcome;
    globalThis.api={S,openMessage,setFlag,onKey,onMailEvent,runMailOperation,retryRead:()=>fetchMessage({...S.sel,localOnly:false},openSeq)};`, context);
  const api = context.api;
  Object.assign(api.S, {
    accounts: [{ id: 'a', receiveEpoch: 2, receiveProtocol: 'imap' }], acct: 'a', folder: 'INBOX',
    live: { a: { state: 'online', unread: 0 } },
  });
  api.S.lists.set('a|INBOX', [{ ...source, seen: true, flagged: false, localOnly: true }]);
  return { ...api, calls, notices, outcomes,
    message: () => api.S.lists.get('a|INBOX')[0],
    key: (key, repeat = false) => api.onKey({ key, repeat, target: { closest: () => null }, preventDefault() {} }),
    operations: () => calls.filter(call => call.cmd === 'execute_mail_operation'),
    reads: () => calls.filter(call => call.cmd === 'get_message_ref'),
  };
}

const success = request => ({ items: request.items.map(ref => ({ source: ref, status: 'succeeded' })) });

test('CF01: cached S submits a guarded flag immediately without reading the body or marking it seen', async () => {
  const operation = deferred();
  const t = setup(undefined, 'succeeded', () => operation.promise);
  await t.openMessage(source);
  t.message().seen = false;
  const reads = t.reads().length;
  const pending = t.key('s'); await tick();
  assert.equal(t.operations().length, 1);
  assert.equal(t.reads().length, reads, 'a flag must not fetch the body');
  assert.equal(t.S.msg.text, body.text);
  assert.equal(t.message().flagged, false, 'pending is not confirmed success');
  assert.equal(t.message().seen, false);
  assert.equal(t.S.flagOperations.size, 1, 'the UI can show a pending state');
  assert.equal(t.notices.length, 0, 'normal validation is quiet');
  assert.deepEqual(JSON.parse(JSON.stringify(t.operations()[0].input.request.items)), [source]);
  operation.resolve(success(t.operations()[0].input.request)); await pending;
  assert.equal(t.message().flagged, true);
  assert.equal(t.message().seen, false);
  assert.equal(t.S.msg.localOnly, true, 'a flag does not authorize unrelated offline actions');
  assert.equal(t.S.flagOperations.size, 0);
});

test('CF02: S does not wait for an in-flight body read', async () => {
  const network = deferred();
  const t = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : network.promise);
  t.message().localOnly = false;
  const opening = t.openMessage(source); await tick();
  const pending = t.key('s'); await tick();
  const submitted = t.operations().length;
  network.resolve(snapshot(false)); await Promise.all([opening, pending]);
  assert.equal(submitted, 1, 'submit before the body response');
  assert.equal(t.reads().length, 2);
  assert.equal(t.message().flagged, true);
});

test('CF03: repeated S while saving is pending is coalesced into one mutation', async () => {
  const operation = deferred();
  const t = setup(undefined, 'succeeded', () => operation.promise);
  await t.openMessage(source);
  const first = t.key('s'), second = t.key('s'), repeat = t.key('s', true); await tick();
  assert.equal(t.operations().length, 1);
  operation.resolve(success(t.operations()[0].input.request)); await Promise.all([first, second, repeat]);
  assert.equal(t.message().flagged, true);
  assert.equal(t.operations().length, 1);
});

test('CF04: missing, partial or failed body loading cannot block an independent flag', async t => {
  for (const state of [null, {...body, partial:true}, {...body, localOnly:true, syncError:'正文不可用'}]) await t.test(String(state?.partial || state?.syncError || 'missing'), async () => {
    const app = setup(); app.S.sel = {...source}; app.S.msg = state;
    await app.key('s');
    assert.equal(app.operations().length, 1);
    assert.equal(app.reads().length, 0);
    assert.equal(app.message().flagged, true);
    assert.equal(app.S.msg, state);
  });
});

test('CF05: switching mail while a flag saves preserves the original operation and the new selection', async () => {
  const operation = deferred();
  const app = setup(undefined, 'succeeded', () => operation.promise);
  await app.openMessage(source);
  const pending = app.key('s'); await tick();
  app.S.lists.get('a|INBOX').push({...source,uid:43,flagged:false});
  app.S.sel = {...source,uid:43}; app.S.msg = {text:'next mail'};
  operation.resolve(success(app.operations()[0].input.request)); await pending;
  assert.equal(app.message().flagged, true);
  assert.equal(app.S.lists.get('a|INBOX')[1].flagged, false);
  assert.equal(app.S.sel.uid, 43);
  assert.equal(app.S.msg.text, 'next mail');
});

test('CF06: a successful reply for another identity never updates this mail', async t => {
  for (const [field, value] of Object.entries({ accountId: 'b', receiveEpoch: 3, folder: 'Archive', uidValidity: 8, uid: 43 })) await t.test(field, async () => {
    const app = setup(undefined, 'succeeded', async () => ({items:[{source:{...source,[field]:value},status:'succeeded'}]}));
    await app.openMessage(source); await app.key('s');
    assert.equal(app.message().flagged, false);
    assert.equal(app.outcomes.at(-1).items[0].status, 'unknown');
  });
});

test('CF07: incomplete or stale account/mailbox identity is rejected before sending', async t => {
  for (const change of ['missing-validity','epoch','validity','removed-account']) await t.test(change, async () => {
    const app = setup();
    if(change==='missing-validity') app.message().uidValidity = null;
    if(change==='epoch') app.S.accounts[0].receiveEpoch++;
    if(change==='validity') app.message().uidValidity++;
    if(change==='removed-account') app.S.accounts=[];
    await app.setFlag(change==='missing-validity' ? app.message() : source,'flagged',true);
    assert.equal(app.operations().length, 0);
    assert.equal(app.reads().length, 0);
  });
});

test('CF08: U and context-menu flagging use the same direct guarded path without requiring the mail to be open', async () => {
  const app = setup();
  await app.setFlag(app.message(),'seen',false);
  await app.setFlag(app.message(),'flagged',true);
  assert.deepEqual(app.operations().map(call=>call.input.request.action), ['seen','flagged']);
  assert.equal(app.reads().length,0);
  assert.equal(app.message().seen,false);
});

test('CF09: failed, unknown and thrown operations preserve flags and body and clear pending state', async t => {
  for (const status of ['failed','unknown','throw']) await t.test(status, async () => {
    const app = setup(undefined, status, status==='throw' ? async()=>{throw Error('connection lost');} : null);
    await app.openMessage(source); await app.key('s');
    assert.equal(app.operations().length,1);
    assert.equal(app.message().flagged,false);
    assert.equal(app.S.msg.text,body.text);
    assert.equal(app.S.flagOperations.size,0);
    assert.equal(app.S.moving.size,0);
    if(status==='throw') assert.match(app.notices.at(-1)[0],/未确认/);
    else assert.equal(app.outcomes.at(-1).items[0].status,status);
  });
});

test('CF12: late completion after account or UIDVALIDITY change cannot patch the new mailbox', async t => {
  for(const change of ['epoch','validity','removed-account']) await t.test(change, async()=>{
    const operation=deferred(), app=setup(undefined,'succeeded',()=>operation.promise);
    await app.openMessage(source); const pending=app.key('s'); await tick();
    if(change==='epoch') app.S.accounts[0].receiveEpoch++;
    if(change==='validity') app.message().uidValidity++;
    if(change==='removed-account') app.S.accounts=[];
    operation.resolve(success(app.operations()[0].input.request)); await pending;
    assert.equal(app.message().flagged,false);
    assert.equal(app.S.flagOperations.size,0);
  });
});

test('CF13: local snapshots still cannot be deleted or moved through this flag exception', async () => {
  const app=setup(); await app.openMessage(source);
  await app.runMailOperation('trash',[app.message()]);
  assert.equal(app.operations().length,0);
  assert.match(app.notices.at(-1)[0],/离线/);
});

test('CF10: validation preserves the displayed complete body when both its disk cache and server become unavailable', async () => {
  let cachePresent = true;
  const app = setup(async (_cmd, input) => {
    if (input.localOnly && cachePresent) return snapshot(true);
    throw Error(input.localOnly ? '缓存已清理或归档卷离线' : '服务器连接失败');
  });
  await app.openMessage(source);
  cachePresent = false;
  await app.retryRead();
  assert.equal(app.S.msg.text, body.text);
  assert.equal(app.S.msg.localOnly, true);
  assert.equal(app.S.msg.syncing, false);
  assert.equal(app.S.msgError, '', 'the error card must not hide an already verified complete local body');
  assert.match(app.S.msg.syncError, /服务器连接失败/);
  assert.equal(app.operations().length, 0);
  assert.equal(app.message().flagged, false);
});

test('CF11: memory fallback is never borrowed from an incomplete body or another full source identity', async t => {
  for (const change of ['partial', 'accountId', 'receiveEpoch', 'folder', 'uidValidity', 'uid']) await t.test(change, async () => {
    let cachePresent = true;
    const app = setup(async (_cmd, input) => {
      if (input.localOnly && cachePresent) return snapshot(true);
      throw Error('校验不可用');
    });
    await app.openMessage(source);
    if (change === 'partial') app.S.msg.partial = true;
    else app.S.msg.source = { ...source, [change]: typeof source[change] === 'number' ? source[change] + 1 : 'other' };
    cachePresent = false;
    await app.retryRead();
    assert.match(app.S.msgError, /校验不可用/);
    assert.equal(app.operations().length, 0);
  });
});


test('CF14: late flags, inbox and new-mail snapshots cannot undo a confirmed star, but newer server changes apply', async t => {
  for(const type of ['flags','inbox','newMail']) await t.test(type, async()=>{
    const app=setup(undefined,'succeeded',async request=>({items:request.items.map(ref=>({source:ref,status:'succeeded',flagsRevision:10}))}));
    await app.openMessage(source); await app.key('s');
    const event=revision=>({type,accountId:'a',receiveEpoch:2,folder:'INBOX',uidValidity:7,flagsRevision:revision,minUid:1,
      items:[{uid:42,seen:true,flagged:false}],messages:[{...source,seen:true,flagged:false}]});
    app.onMailEvent(event(9));
    assert.equal(app.message().flagged,true,'late snapshot must not undo a saved star');
    app.onMailEvent(event(11));
    assert.equal(app.message().flagged,false,'a newer change from another client still applies');
    app.onMailEvent(event(9));
    assert.equal(app.message().flagged,false,'old snapshots cannot roll back the newer observed change');
  });
});

test('CF15: a delayed flags event from another mailbox generation cannot remove or alter current messages', async()=>{
  const app=setup(); app.message().uidValidity=8; app.message().flagged=true;
  app.onMailEvent({type:'flags',accountId:'a',receiveEpoch:2,folder:'INBOX',uidValidity:7,flagsRevision:20,minUid:1,items:[]});
  assert.equal(app.message().uidValidity,8);
  assert.equal(app.message().flagged,true);
});

test('CF16: an older snapshot missing a confirmed mail cannot remove it, but a newer deletion can', async t => {
  for (const type of ['flags', 'inbox']) await t.test(type, async () => {
    const app=setup(undefined,'succeeded',async request=>({items:request.items.map(ref=>({source:ref,status:'succeeded',flagsRevision:10}))}));
    await app.setFlag(source,'flagged',true);
    app.S.lists.set('a|__starred',[{...app.message()}]);
    const event=revision=>({type,accountId:'a',receiveEpoch:2,folder:'INBOX',uidValidity:7,flagsRevision:revision,minUid:1,
      items:[],messages:[{...source,uid:41,seen:true,flagged:false}]});
    app.onMailEvent(event(9));
    assert.ok(app.S.lists.get('a|INBOX').some(m=>m.uid===42),'late snapshot must keep the confirmed source');
    if(type==='flags') assert.equal(app.S.lists.get('a|__starred').length,1);
    app.onMailEvent(event(11));
    assert.equal(app.S.lists.get('a|INBOX').some(m=>m.uid===42),false,'a newer deletion still takes effect');
    if(type==='flags') assert.equal(app.S.lists.get('a|__starred').length,0);
  });
});

test('CF17: late acknowledgements preserve newer flags and describe that subsequent sync truthfully', async t => {
 for (const revision of [10, 11]) await t.test(`snapshot revision ${revision}`, async () => {
  const delayed=deferred();
  const app=setup(undefined,'succeeded',()=>delayed.promise);
  const saving=app.setFlag(source,'flagged',true);
  app.onMailEvent({type:'flags',accountId:'a',receiveEpoch:2,folder:'INBOX',uidValidity:7,flagsRevision:revision,minUid:1,
    items:[{uid:42,seen:true,flagged:false}]});
  delayed.resolve({items:[{source,status:'succeeded',flagsRevision:10}]});
  await saving;
  assert.equal(app.message().flagged,false);
  assert.match(require('../ui/mail-operations').summary(app.outcomes.at(-1)).text,/后续同步/);
 });
});
