const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const source = { accountId: 'a', receiveEpoch: 2, folder: 'Projects', uidValidity: 7, uid: 42 };
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
const success = request => ({ items: request.items.map(source => ({ source, status: 'succeeded' })) });
function setup(operation = async request => success(request), read = async () => null) {
  const calls = [], menus = [], notices = [], outcomes = [];
  const context = vm.createContext({
    window: {
      MailBeaconSearch: require('../ui/mail-search'), MailBeaconSelection: require('../ui/mail-selection'),
      MailBeaconOperations: require('../ui/mail-operations'), MailBeaconViews: require('../ui/views'),
      MailBeaconThreads: require('../ui/threads'),
      MailBeaconRules: require('../ui/rules'),
      __TAURI__: { core: { invoke: async (command, input) => {
        calls.push({ command, input });
        if (command === 'execute_mail_operation') return operation(input.request);
        return command === 'get_message_ref' ? read(input) : null;
      } }, event: { listen: async () => {} } },
    },
    document: { activeElement: null, querySelector: () => null, querySelectorAll: () => [], addEventListener() {} },
    console, setTimeout, clearTimeout,
    captureMenu: (_anchor, entries) => menus.push(entries), notice: (...args) => notices.push(args),
    outcome: result => outcomes.push(result),
  });
  const app = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(app.slice(0, app.indexOf('(async function boot()')) + `
    render=renderList=renderReader=renderSidebar=updateBadge=refreshOperationSources=function(){};
    showMenu=captureMenu; toast=notice; showOperationOutcome=outcome;
    globalThis.api={S,setFlag,runMailOperation,showMessageMenu,mergeConfirmedFlags,readOnline:()=>fetchMessage({...S.sel,localOnly:false},openSeq)};
  `, context);
  const { S } = context.api;
  Object.assign(S, { accounts: [{ id: 'a', receiveEpoch: 2, receiveProtocol: 'imap' }], acct: 'a', folder: 'Projects',
    sel: { ...source }, live: { a: { state: 'online', unread: 0 } },
  });
  S.lists.set('a|Projects', [{ ...source, flagged: false, seen: false, localOnly: true }]);
  return { ...context.api, calls, menus, notices, outcomes, row: () => S.lists.get('a|Projects')[0],
    operations: () => calls.filter(call => call.command === 'execute_mail_operation'),
  };
}

test('review: context flag actions retain the complete identity captured when the menu opened', async t => {
  for (const action of ['加星标', '标为已读']) for (const change of ['receiveEpoch', 'uidValidity']) {
    await t.test(`${action}: ${change}`, async () => {
      const app = setup();
      app.showMessageMenu(null, source);
      const item = app.menus[0].find(item => item.label === action);
      assert.ok(item);
      app.row()[change]++;
      if (change === 'receiveEpoch') app.S.accounts[0].receiveEpoch++;
      await item.run();
      assert.equal(app.operations().length, 0, 'a menu must not rebind an old identity to a reused UID');
      assert.equal(app.row().flagged, false);
      assert.equal(app.row().seen, false);
    });
  }
});

test('review: a successful old flag must not patch a different UIDVALIDITY copy in another list', async () => {
  const pending = deferred(), app = setup(() => pending.promise);
  const saved = app.setFlag({ ...source }, 'flagged', true);
  const newer = { ...source, uidValidity: 8, flagged: false, seen: false };
  app.S.results.set('__search', { items: [newer] });
  pending.resolve(success(app.operations()[0].input.request));
  await saved;
  assert.equal(app.row().flagged, true);
  assert.equal(newer.flagged, false, 'same UID in another mailbox generation is another mail');
  assert.equal(app.S.flagOperations.size, 0);
});

test('review: partial bulk flag results only patch successes and leave no pending state', async () => {
  const app = setup(async request => ({ items: request.items.map((source, i) => ({ source, status: i ? 'failed' : 'succeeded', message: i ? 'server refused' : '' })) }));
  const second = { ...source, uid: 43, flagged: false, localOnly: true };
  app.S.lists.get('a|Projects').push(second);
  app.S.batchMode = true;
  for (const row of [app.row(), second]) app.S.batch.add(require('../ui/mail-selection').key(row));
  assert.equal(await app.runMailOperation('flagged', [app.row(), second], { value: true }), false);
  assert.equal(app.row().flagged, true);
  assert.equal(second.flagged, false);
  assert.equal(app.S.batch.size, 1);
  assert.equal(app.S.moving.size, 0);
  assert.equal(app.S.flagOperations.size, 0);
  assert.match(require('../ui/mail-operations').summary(app.outcomes[0]).text, /成功 1 封.*失败 1 封/);
});

test('review: an older body-read completion cannot overwrite a confirmed unread intent', async () => {
  const delayed = deferred();
  const snapshot = { source, message: { subject: 'mail', text: 'body stays readable', attachments: [], uidValidity: 7 },
    localOnly: true, readMarked: false, flagsRevision: 8 };
  const app = setup(async request => ({ items: request.items.map(source => ({ source, status: 'succeeded', flagsRevision: 9 })) }),
    async input => input.localOnly ? snapshot : delayed.promise);
  app.row().seen = true;
  const reading = app.readOnline();
  await app.setFlag({ ...source }, 'seen', false);
  assert.equal(app.row().seen, false);
  delayed.resolve({ ...snapshot, localOnly: false, readMarked: true });
  await reading;
  assert.equal(app.row().seen, false, 'late automatic read status cannot override newer explicit unread');
  assert.equal(app.S.msg.text, 'body stays readable');
});

test('review: a delayed mutation acknowledgement cannot overwrite a later server observation', async t => {
  for (const observedRevision of [9, 10]) await t.test(`observed revision ${observedRevision}`, async () => {
    const delayed = deferred(), app = setup(() => delayed.promise);
    const sending = app.setFlag({ ...source }, 'flagged', true);
    // This is a local mutation counter, not a server MODSEQ. A FETCH after our
    // STORE can still carry revision 9 while observing another client's change.
    Object.assign(app.row(), app.mergeConfirmedFlags(app.row(), { flagged: false }, observedRevision));
    delayed.resolve({ items: [{ source, status: 'succeeded', flagsRevision: 9 }] });
    await sending;
    assert.equal(app.row().flagged, false, `ack revision 9 must not overwrite observed revision ${observedRevision}`);
    assert.equal(app.outcomes[0].items[0].superseded, true);
    assert.equal(app.S.flagOperations.size, 0);
  });
});

test('review: a server observation at the acknowledgement revision can update a confirmed flag', async () => {
  const app = setup(async request => ({ items: request.items.map(source => ({ source, status: 'succeeded', flagsRevision: 9 })) }));
  await app.setFlag({ ...source }, 'flagged', true);
  assert.equal(app.row().flagged, true);
  Object.assign(app.row(), app.mergeConfirmedFlags(app.row(), { flagged: false }, 9));
  assert.equal(app.row().flagged, false, 'equal-revision FETCH remains authoritative after the acknowledgement');
});
