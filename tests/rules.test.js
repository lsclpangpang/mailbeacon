const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const R = require('../ui/rules.js');

const plain = (v) => JSON.parse(JSON.stringify(v));
const folders = [
  { path: 'INBOX', name: '收件箱', role: 'inbox' }, { path: 'Sent', name: '已发送', role: 'sent' },
  { path: 'Trash', name: '已删除', role: 'trash' }, { path: 'Bills', name: '账单', role: 'other' },
  { path: '__local_drafts', name: '本地草稿', role: 'local-drafts' },
];

test('a new rule starts with one empty condition for the chosen account', () => {
  assert.deepEqual(plain(R.blankRule('a')), {
    id: '', name: '', enabled: true, accountId: 'a', matchAll: true,
    conditions: [{ field: 'from', op: 'contains', value: '' }],
    actions: { moveTo: null, markRead: false, flag: false },
  });
});

test('a rule can be prefilled from a message sender', () => {
  const rule = R.ruleFromMessage({ accountId: 'a', fromName: 'GitHub', fromAddr: 'noreply@github.com' });
  assert.equal(rule.name, '来自 GitHub');
  assert.deepEqual(plain(rule.conditions), [{ field: 'from', op: 'equals', value: 'noreply@github.com' }]);
  assert.equal(R.ruleFromMessage({ accountId: 'a', fromName: '', fromAddr: 'x@y.com' }).name, '来自 x@y.com');
});

test('only real folders other than the inbox are move targets', () => {
  assert.deepEqual(R.moveTargets(folders).map((f) => f.path), ['Trash', 'Bills']);
  assert.equal(R.defaultTarget(folders), 'Bills', '默认选用户自己的文件夹');
  assert.equal(R.defaultTarget([{ path: 'Trash', role: 'trash' }, { path: 'Archive', role: 'archive' }]), 'Archive');
  assert.equal(R.defaultTarget([]), '');
  assert.equal(R.defaultTarget([{ path: '[Gmail]/All Mail', role: 'all' }, { path: 'Junk', role: 'junk' }]), '', '没有自己的文件夹时让用户自己选，不默认「所有邮件」');
});

test('rules read as a sentence', () => {
  const rule = { ...R.blankRule('a'), matchAll: false,
    conditions: [{ field: 'from', op: 'contains', value: 'bank' }, { field: 'attachment', op: 'contains', value: '' }, { field: 'subject', op: 'notContains', value: '广告' }],
    actions: { moveTo: 'Bills', markRead: true, flag: true } };
  assert.equal(R.describe(rule, folders), '发件人包含「bank」 或 有附件 或 主题不包含「广告」 → 移到「账单」、标为已读、加星标');
  const noAttach = { ...rule, matchAll: true, conditions: [{ field: 'attachment', op: 'notContains', value: '' }, { field: 'to', op: 'equals', value: 'me@x.com' }], actions: { moveTo: 'Gone', markRead: false, flag: false } };
  assert.equal(R.describe(noAttach, folders), '没有附件 且 收件人等于「me@x.com」 → 移到「Gone」');
});

test('client-side check explains what is missing before saving', () => {
  const rule = R.blankRule('a');
  assert.equal(R.problem(rule), '请填写条件内容');
  rule.conditions[0].value = 'bank';
  assert.equal(R.problem(rule), '请至少选择一个动作');
  rule.actions.markRead = true;
  assert.equal(R.problem(rule), '');
  rule.conditions = [];
  assert.equal(R.problem(rule), '请至少添加一个条件');
  assert.equal(R.problem({ ...R.blankRule(''), conditions: [{ field: 'attachment', op: 'contains', value: '' }], actions: { moveTo: 'Bills' } }), '请选择邮箱账号');
});

test('moving a rule up or down returns a new order', () => {
  const list = [{ id: 'a' }, { id: 'b' }, { id: 'c' }];
  assert.deepEqual(R.move(list, 1, -1).map((r) => r.id), ['b', 'a', 'c']);
  assert.deepEqual(R.move(list, 2, 1).map((r) => r.id), ['a', 'b', 'c'], '到底了不动');
  assert.deepEqual(list.map((r) => r.id), ['a', 'b', 'c'], '不改原数组');
});

function app(invoke) {
  const body = { innerHTML: '' };
  const overlay = { innerHTML: '', querySelectorAll: () => [], querySelector: (sel) => (sel === '.rules-body' ? body : null) };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: R, MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: { querySelector: (s) => (s === '#overlay' ? overlay : null), addEventListener: () => {} },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = function () {};
    toast = (text, isErr) => { globalThis.toasts.push([text, !!isErr]); };
    globalThis.api = { S, onMailEvent, openRules: typeof openRules === 'function' ? openRules : null, saveRules: typeof saveRules === 'function' ? saveRules : null, runRulesNow: typeof runRulesNow === 'function' ? runRulesNow : null };
  `, Object.assign(context, { toasts: [] }));
  context.api.S.accounts = [{ id: 'a', email: 'a@x.com', name: 'A' }, { id: 'p', email: 'p@x.com', name: 'P', receiveProtocol: 'pop3' }];
  context.api.S.folders = { a: folders };
  return { ...context.api, overlay, body, toasts: context.toasts };
}

test('the rules sheet lists saved rules with their description and order controls', async () => {
  const saved = [{ ...R.blankRule('a'), id: 'r1', name: '银行', conditions: [{ field: 'from', op: 'contains', value: 'bank' }], actions: { moveTo: 'Bills', markRead: false, flag: false } }];
  const t = app(async (cmd) => (cmd === 'get_rules' ? saved : null));
  assert.equal(typeof t.openRules, 'function');
  await t.openRules();
  assert.match(t.overlay.innerHTML, /收信规则/);
  assert.match(t.body.innerHTML, /银行/);
  assert.match(t.body.innerHTML, /发件人包含「bank」 → 移到「账单」/);
  assert.match(t.body.innerHTML, /data-rule-index="0"[\s\S]*data-action="rule-edit"/);
  assert.match(t.body.innerHTML, /<option value="a"/);
  assert.doesNotMatch(t.body.innerHTML, /<option value="p"/, 'POP3 账号不能设置规则');
});

test('saving keeps the server-normalised rules and reports failures without losing them', async () => {
  let fail = false;
  const t = app(async (cmd, input) => {
    if (cmd === 'save_rules') { if (fail) throw '「x」至少需要一个动作'; return input.rules.map((r, i) => ({ ...r, id: r.id || `new${i}` })); }
    return [];
  });
  const rule = { ...R.blankRule('a'), conditions: [{ field: 'from', op: 'contains', value: 'bank' }], actions: { moveTo: 'Bills', markRead: false, flag: false } };
  assert.equal(await t.saveRules([rule]), true);
  assert.equal(t.S.rules[0].id, 'new0');
  fail = true;
  assert.equal(await t.saveRules([]), false);
  assert.equal(t.S.rules.length, 1, '失败时保留已保存的规则');
  assert.deepEqual(plain(t.toasts.at(-1)), ['「x」至少需要一个动作', true]);
});

test('running rules now reports how many mails were sorted and refreshes the inbox', async () => {
  const calls = [];
  const t = app(async (cmd, input) => {
    calls.push(cmd);
    if (cmd === 'run_rules') return { matched: 3, moved: 2, errors: [] };
    return [];
  });
  await t.runRulesNow('a');
  assert.deepEqual(plain(t.toasts.at(-1)), ['已处理 3 封邮件，其中 2 封移到了其他文件夹', false]);
  assert.ok(calls.includes('list_messages'), '刷新收件箱');
  const quiet = app(async (cmd) => (cmd === 'run_rules' ? { matched: 0, moved: 0, errors: [] } : []));
  await quiet.runRulesNow('a');
  assert.deepEqual(plain(quiet.toasts.at(-1)), ['收件箱最近的邮件都没有命中规则', false]);
});

test('the same rule error is shown once rather than on every refresh', () => {
  const t = app(async () => []);
  for (let i = 0; i < 3; i++) t.onMailEvent({ type: 'ruleError', accountId: 'a', detail: '收信规则没能把邮件移到「Gone」' });
  assert.equal(t.toasts.length, 1);
  t.onMailEvent({ type: 'ruleError', accountId: 'a', detail: '别的错误' });
  assert.equal(t.toasts.length, 2);
});

test('a run-now report with problems still says how many mails were sorted', async () => {
  const t = app(async (cmd) => (cmd === 'run_rules' ? { matched: 4, moved: 1, errors: ['收信规则没能把邮件移到「Gone」：NO'] } : []));
  await t.runRulesNow('a');
  assert.deepEqual(plain(t.toasts.at(-1)), ['已处理 4 封邮件，其中 1 封移到了其他文件夹；收信规则没能把邮件移到「Gone」：NO', true]);
});

test('running rules for one account does not reload another account\'s folder', async () => {
  const calls = [];
  const t = app(async (cmd, input) => { calls.push([cmd, input]); return cmd === 'run_rules' ? { matched: 1, moved: 1, errors: [] } : []; });
  t.S.accounts.push({ id: 'b', email: 'b@x.com', name: 'B' });
  t.S.acct = 'b';
  t.S.folder = 'Archive';
  await t.runRulesNow('a');
  const lists = calls.filter(([c]) => c === 'list_messages').map(([, i]) => `${i.accountId}|${i.folder}`);
  assert.deepEqual(lists, ['a|INBOX']);
});
