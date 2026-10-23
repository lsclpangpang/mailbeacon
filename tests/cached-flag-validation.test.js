const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const tick = () => new Promise(resolve => setImmediate(resolve));
const source = { accountId: 'a', receiveEpoch: 2, folder: 'INBOX', uidValidity: 7, uid: 42 };
const body = { subject: 'Cached mail', text: 'Full cached body', attachments: [], uidValidity: 7 };
const snapshot = (localOnly, ref = source) => ({ source: ref, message: { ...body }, localOnly, readMarked: false, snapshotAt: 100 });
function setup(handler = async (_cmd, input) => snapshot(input.localOnly), operationStatus = 'succeeded') {
  const calls = [], notices = [], outcomes = [], overlay = { innerHTML: '' };
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search'), MailBeaconViews: require('../ui/views'),
      MailBeaconSelection: require('../ui/mail-selection'), MailBeaconOperations: require('../ui/mail-operations'),
      MailBeaconThreads: require('../ui/threads'),
      __TAURI__: { core: { invoke: async (cmd, input) => {
        calls.push({ cmd, input });
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
    render=renderList=renderReader=renderSidebar=updateBadge=revealSelectedRow=refreshOperationSources=function(){};
    toast=notice; showOperationOutcome=outcome;
    globalThis.api={S,openMessage,setFlag,onKey};`, context);
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

test('CF01: S validates a cached selected mail online before changing its star, preserving the visible body', async () => {
  const network = deferred();
  const t = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : network.promise);
  await t.openMessage(source);
  const pending = t.key('s'); await tick();
  assert.deepEqual(t.reads().map(call => call.input.localOnly), [true, true, false]);
  assert.equal(t.S.msg.text, body.text);
  assert.equal(t.message().flagged, false);
  assert.equal(t.operations().length, 0);
  network.resolve(snapshot(false)); await pending;
  assert.equal(t.operations().length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(t.operations()[0].input.request.items)), [source]);
  assert.equal(t.message().flagged, true);
  assert.equal(t.S.msg.localOnly, false);
});

test('CF02: an S shortcut during a cache-first online open reuses the pending validation', async () => {
  const network = deferred();
  const t = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : network.promise);
  t.message().localOnly = false;
  const opening = t.openMessage(source); await tick();
  assert.equal(t.S.msg.syncing, true);
  const pending = t.key('s'); await tick();
  assert.equal(t.reads().length, 2, 'S must not start another fetch');
  network.resolve(snapshot(false)); await Promise.all([opening, pending]);
  assert.equal(t.operations().length, 1);
  assert.equal(t.message().flagged, true);
});

test('CF03: repeated S while validation is pending results in one flag mutation', async () => {
  const network = deferred();
  const t = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : network.promise);
  await t.openMessage(source);
  const first = t.key('s'), second = t.key('s'), repeat = t.key('s', true); await tick();
  assert.equal(t.reads().filter(call => !call.input.localOnly).length, 1);
  network.resolve(snapshot(false)); await Promise.all([first, second, repeat]);
  assert.equal(t.operations().length, 1);
});

test('CF04: failed validation and local fallback remain read-only with no flag mutation', async t => {
  for (const mode of ['failure', 'fallback']) await t.test(mode, async () => {
    const app = setup(async (_cmd, input) => {
      if (input.localOnly) return snapshot(true);
      if (mode === 'failure') throw Error('连接超时：合成测试');
      return { ...snapshot(true), syncError: '服务器未确认此邮件' };
    });
    await app.openMessage(source); await app.key('s');
    assert.equal(app.operations().length, 0);
    assert.equal(app.message().flagged, false);
    assert.equal(app.S.msg.localOnly, true);
    assert.equal(app.S.msg.text, body.text);
    assert.match(app.notices.at(-1)?.[0] || '', mode === 'failure' ? /连接超时/ : /服务器未确认/);
  });
});

test('CF05: selection, account and mailbox identity changes cancel a queued flag', async t => {
  for (const change of ['selection', 'epoch', 'validity', 'removed-account']) await t.test(change, async () => {
    const network = deferred();
    const app = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : network.promise);
    await app.openMessage(source);
    const pending = app.key('s'); await tick();
    if (change === 'selection') app.S.sel = { ...source, uid: 43 };
    if (change === 'epoch') app.S.accounts[0].receiveEpoch++;
    if (change === 'validity') app.S.lists.set('a|INBOX', [{ ...source, uidValidity: 8, flagged: false, localOnly: true }]);
    if (change === 'removed-account') app.S.accounts = [];
    network.resolve(snapshot(false)); await pending;
    assert.equal(app.operations().length, 0, change);
    assert.equal(app.message().flagged, false);
  });
});

test('CF06: a server response for another source never authorizes a flag', async t => {
  for (const [field, value] of Object.entries({ accountId: 'b', receiveEpoch: 3, folder: 'Archive', uidValidity: 8, uid: 43 })) await t.test(field, async () => {
    const app = setup(async (_cmd, input) => input.localOnly ? snapshot(true) : snapshot(false, { ...source, [field]: value }));
    await app.openMessage(source); await app.key('s');
    assert.equal(app.operations().length, 0);
    assert.equal(app.message().flagged, false);
    assert.equal(app.S.msg.localOnly, true);
    assert.match(app.notices.at(-1)?.[0] || '', /身份/);
  });
});

test('CF07: incomplete identity cannot trigger validation or mutate a local copy', async () => {
  const t = setup();
  t.S.sel = { ...source, uidValidity: null };
  Object.assign(t.message(), { uidValidity: null });
  t.S.msg = { ...body, localOnly: true };
  await t.key('s');
  assert.equal(t.reads().length, 0);
  assert.equal(t.operations().length, 0);
});

test('CF08: U uses the same validated flag path while ordinary online flagging stays direct', async () => {
  const t = setup(); await t.openMessage(source);
  await t.key('u');
  assert.equal(t.operations().length, 1);
  assert.equal(t.operations()[0].input.request.action, 'seen');
  assert.equal(t.operations()[0].input.request.value, false);
  const reads = t.reads().length;
  await t.key('s');
  assert.equal(t.operations().length, 2);
  assert.equal(t.reads().length, reads);
});

test('CF09: successful validation never makes a failed or unknown flag result look successful', async t => {
  for (const status of ['failed', 'unknown']) await t.test(status, async () => {
    const app = setup(undefined, status); await app.openMessage(source); await app.key('s');
    assert.equal(app.operations().length, 1);
    assert.equal(app.message().flagged, false);
    assert.equal(app.outcomes.at(-1).items[0].status, status);
  });
});

test('CF10: validation preserves the displayed complete body when both its disk cache and server become unavailable', async () => {
  let cachePresent = true;
  const app = setup(async (_cmd, input) => {
    if (input.localOnly && cachePresent) return snapshot(true);
    throw Error(input.localOnly ? '缓存已清理或归档卷离线' : '服务器连接失败');
  });
  await app.openMessage(source);
  cachePresent = false;
  await app.key('s');
  assert.equal(app.S.msg.text, body.text);
  assert.equal(app.S.msg.localOnly, true);
  assert.equal(app.S.msg.syncing, false);
  assert.equal(app.S.msgError, '', 'the error card must not hide an already verified complete local body');
  assert.match(app.S.msg.syncError, /服务器连接失败/);
  assert.match(app.notices.at(-1)?.[0] || '', /服务器连接失败/);
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
    await app.key('s');
    assert.match(app.S.msgError, /校验不可用/);
    assert.equal(app.operations().length, 0);
  });
});
