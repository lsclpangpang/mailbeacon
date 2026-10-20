const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const plain = value => JSON.parse(JSON.stringify(value));
const flush = () => new Promise(resolve => setImmediate(resolve));
function node(extra = {}) {
  const classes = new Set();
  const attrs = new Map();
  return { hidden: false, disabled: false, checked: false, textContent: '',
    setAttribute: (name, value) => attrs.set(name, String(value)), getAttribute: name => attrs.get(name), removeAttribute: name => attrs.delete(name),
    classList: { toggle: (c, yes) => yes ? classes.add(c) : classes.delete(c), contains: c => classes.has(c) }, ...extra };
}
function section(accountId) {
  const nodes = Object.fromEntries(['.bulk-download-runtime', '.bulk-download-progress', '.bulk-download-progress-label', '.bulk-download-progress-bar', '.bulk-download-counts', '.bulk-download-directory', '.bulk-download-folder', '.bulk-download-error', '.bulk-download-inherit', '.bulk-download-saved-rule', '.bulk-download-task-rule', '[data-action="start-bulk-download"]', '[data-action="cancel-bulk-download"]'].map(s => [s, node()]));
  return { dataset: { bulkDownloadAccount: accountId }, nodes, querySelector: s => nodes[s] || null };
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const status = (state, extra = {}) => ({ state, detail: state, directory: '/tmp/archive/account', currentFolder: null, inheritFilters: false, filterMode: 'none', savedCount: 0, existingCount: 0, skippedCount: 0, failedCount: 0, vanishedCount: 0, processedCount: 0, totalCount: null, foldersScanned: 0, foldersDone: 0, folderCount: 5, ...extra });
function accountBox(id) {
  const nodes = { '.settings-account-toggle': node(), '.settings-account-details': node({ innerHTML: '<input value="未保存草稿">' }), '.account-bulk-summary': node() };
  return { dataset: { settingsAccount: id }, nodes, querySelector: s => nodes[s] || null };
}
function app(invoke = async () => {}, sections = [], accountBoxes = []) {
  const listeners = {}, overlay = { innerHTML: '', querySelectorAll: s => s === '.bulk-download-section' ? sections : s === '.settings-account' ? accountBoxes : [] };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: { querySelector: s => s === '#overlay' ? overlay : null, addEventListener: (n, f) => { listeners[n] = f; } }, console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = loadFolders = function () {};
    globalThis.api = { S, openSettings, onMailEvent,
      toggleAccount: typeof toggleSettingsAccount === 'function' ? toggleSettingsAccount : null,
      start: typeof startBulkDownload === 'function' ? startBulkDownload : null,
      cancel: typeof cancelBulkDownload === 'function' ? cancelBulkDownload : null,
      renderStatus: typeof renderBulkDownloadStatus === 'function' ? renderBulkDownloadStatus : null };
  `, context);
  context.api.S.accounts = [{ id: 'a', email: 'a@example.com', autoDownload: { enabled: true, directory: '/tmp/auto-a', filterMode: 'whitelist', keywords: ['账单'] } }, { id: 'b', email: 'b@example.com', autoDownload: { enabled: false, directory: '' } }];
  return { ...context.api, overlay, listeners, startBoot: () => vm.runInContext(source.slice(source.indexOf('(async function boot()'), source.indexOf('// 第一次添加账号后')), context) };
}

test('settings provide independent full-account downloads outside automatic download forms with explicit scope', () => {
  const { openSettings, overlay } = app();
  openSettings();
  assert.match(overlay.innerHTML, /data-bulk-download-account="a"/);
  assert.match(overlay.innerHTML, /data-bulk-download-account="b"/);
  assert.match(overlay.innerHTML, /下载此账号全部邮件/);
  assert.match(overlay.innerHTML, /所有文件夹[\s\S]*已发送[\s\S]*垃圾[\s\S]*已删除/);
  assert.match(overlay.innerHTML, /继承该账号的黑白名单规则/);
  assert.match(overlay.innerHTML, /未勾选时不做过滤/);
  assert.match(overlay.innerHTML, /已保存[\s\S]*自动下载关闭/);
  assert.match(overlay.innerHTML, /完整 \.eml（含附件）/);
  assert.match(overlay.innerHTML, /关闭[\s\S]*中断/);
  assert.match(overlay.innerHTML, /同一[个]?目录[\s\S]*继续/);
  for (const form of overlay.innerHTML.matchAll(/<form class="auto-download-form"[\s\S]*?<\/form>/g)) assert.doesNotMatch(form[0], /data-bulk-download-account/);
});

test('cancelling the directory picker never starts a task or changes automatic settings', async () => {
  const calls = [], box = section('a');
  const { start, S } = app(async (cmd, args) => { calls.push({ cmd, args }); return null; }, [box]);
  assert.equal(typeof start, 'function');
  const before = plain(S.accounts);
  await start(box);
  assert.deepEqual(plain(calls), [{ cmd: 'choose_download_directory', args: { directory: '/tmp/auto-a' } }]);
  assert.deepEqual(plain(S.accounts), before);
  assert.equal(box.nodes['[data-action="start-bulk-download"]'].disabled, false);
});

test('active task buttons describe the current work and only terminal states offer another download', () => {
  const a = section('a'), { onMailEvent } = app(undefined, [a]);
  for (const [state, label] of [['preparing', '正在准备…'], ['scanning', '正在统计…'], ['downloading', '正在下载…'], ['cancelling', '正在取消…']]) {
    onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status(state) });
    assert.equal(a.nodes['[data-action="start-bulk-download"]'].textContent, label);
    assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, true);
  }
  for (const state of ['complete', 'cancelled', 'error']) {
    onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status(state) });
    assert.equal(a.nodes['[data-action="start-bulk-download"]'].textContent, '再次选择文件夹下载…');
    assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, false);
  }
});

test('settings accounts use separate accessible disclosure buttons with scoped initial and remembered expansion', () => {
  const a = accountBox('a'), b = accountBox('b'), { S, openSettings, toggleAccount, overlay } = app(undefined, [], [a, b]);
  openSettings();
  assert.match(overlay.innerHTML, /data-settings-account="a"/);
  assert.match(overlay.innerHTML, /aria-controls="settings-account-details-a"/);
  assert.match(overlay.innerHTML, /<button[^>]+settings-account-toggle[^>]+aria-expanded="true"/);
  for (const button of overlay.innerHTML.matchAll(/<button[^>]+settings-account-toggle[^>]*>[\s\S]*?<\/button>/g)) assert.doesNotMatch(button[0], /data-remove/);
  assert.equal(typeof toggleAccount, 'function');
  assert.equal(a.nodes['.settings-account-details'].hidden, false);
  assert.equal(b.nodes['.settings-account-details'].hidden, true);
  toggleAccount(a);
  toggleAccount(b);
  openSettings();
  assert.equal(a.nodes['.settings-account-details'].hidden, true);
  assert.equal(b.nodes['.settings-account-details'].hidden, false);
  assert.equal(a.nodes['.settings-account-toggle'].getAttribute('aria-expanded'), 'false');
  assert.equal(b.nodes['.settings-account-toggle'].getAttribute('aria-expanded'), 'true');
  S.acct = 'a';
  openSettings();
  assert.equal(a.nodes['.settings-account-details'].hidden, true);
});

test('account collapse preserves DOM draft nodes and runtime summaries update without reopening the details', () => {
  const a = accountBox('a'), b = accountBox('b'), box = section('a');
  const { openSettings, toggleAccount, onMailEvent, overlay } = app(undefined, [box], [a, b]);
  openSettings();
  const content = a.nodes['.settings-account-details'];
  content.unsavedSignature = '新签名'; content.unsavedKeywords = '尚未保存的词';
  const originalMarkup = overlay.innerHTML;
  assert.equal(typeof toggleAccount, 'function');
  toggleAccount(a);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { totalCount: 10, processedCount: 4, savedCount: 2, skippedCount: 2 }) });
  assert.equal(a.nodes['.settings-account-details'], content);
  assert.equal(content.hidden, true);
  assert.equal(content.unsavedSignature, '新签名');
  assert.equal(content.unsavedKeywords, '尚未保存的词');
  assert.equal(overlay.innerHTML, originalMarkup);
  assert.match(a.nodes['.account-bulk-summary'].textContent, /正在下载.*已处理 4 \/ 总计 10 封 · 40%/);
  assert.doesNotMatch(b.nodes['.account-bulk-summary'].textContent, /40%/);
  toggleAccount(a);
  assert.equal(content.hidden, false);
});

test('progress uses authoritative processed totals, excludes filtered-mail ambiguity, and counts vanished mail separately', () => {
  const a = section('a'), header = accountBox('a'), { onMailEvent } = app(undefined, [a], [header]);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { totalCount: 20, processedCount: 10, savedCount: 2, existingCount: 2, skippedCount: 4, failedCount: 1, vanishedCount: 1 }) });
  assert.equal(a.nodes['.bulk-download-progress-label'].textContent, '已处理 10 / 总计 20 封 · 50%');
  assert.equal(a.nodes['.bulk-download-progress-bar'].getAttribute('value'), '50');
  assert.match(a.nodes['.bulk-download-counts'].textContent, /跳过 4 封.*已从服务器移除 1 封.*失败 1 项/);
  assert.match(header.nodes['.account-bulk-summary'].textContent, /已处理 10 \/ 总计 20 封 · 50%/);
});

test('scanning is indeterminate and cancellable; empty completion and failed totals cannot display false success', () => {
  const a = section('a'), header = accountBox('a'), { onMailEvent } = app(undefined, [a], [header]);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('scanning', { foldersScanned: 2, totalCount: null }) });
  assert.match(a.nodes['.bulk-download-progress-label'].textContent, /正在统计邮件总数.*2 \/ 5/);
  assert.equal(a.nodes['.bulk-download-progress-bar'].getAttribute('value'), undefined);
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].hidden, false);
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, true);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { totalCount: 0, processedCount: 0 }) });
  assert.equal(a.nodes['.bulk-download-progress-label'].textContent, '已处理 0 / 总计 0 封 · 100%');
  assert.equal(a.nodes['.bulk-download-progress-bar'].getAttribute('value'), '100');
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('error', { totalCount: 4, processedCount: 4, failedCount: 1 }) });
  assert.equal(a.nodes['.bulk-download-progress'].classList.contains('err'), true);
  assert.equal(a.nodes['.bulk-download-progress'].classList.contains('complete'), false);
  assert.equal(header.nodes['.account-bulk-summary'].classList.contains('err'), true);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('cancelled', { totalCount: null, foldersScanned: 2 }) });
  assert.doesNotMatch(a.nodes['.bulk-download-progress-label'].textContent, /正在统计/);
});

test('start prevents duplicate picker and download commands across reopened settings while other accounts remain usable', async () => {
  const picker = deferred(), startRequest = deferred(), calls = [], a = section('a'), b = section('b');
  const { start, openSettings, S } = app((cmd, args) => { calls.push({ cmd, args }); return cmd === 'choose_download_directory' ? picker.promise : startRequest.promise; }, [a, b]);
  assert.equal(typeof start, 'function');
  const before = plain(S.accounts), pending = start(a);
  await start(a);
  openSettings();
  await start(section('a'));
  assert.equal(calls.length, 1);
  assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, true);
  assert.equal(b.nodes['[data-action="start-bulk-download"]'].disabled, false);
  picker.resolve('/tmp/chosen');
  await flush();
  assert.deepEqual(plain(calls[1]), { cmd: 'start_bulk_download', args: { accountId: 'a', directory: '/tmp/chosen', inheritFilters: false } });
  startRequest.resolve(status('preparing'));
  await pending;
  assert.equal(S.bulkDownloads.a.state, 'preparing');
  assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, true);
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].disabled, false);
  assert.deepEqual(plain(S.accounts), before);
});

test('newer progress events win over stale start and cancel responses', async () => {
  const started = deferred(), cancelled = deferred(), a = section('a');
  const { start, cancel, S, onMailEvent } = app(cmd => cmd === 'choose_download_directory' ? Promise.resolve('/tmp/chosen') : cmd === 'start_bulk_download' ? started.promise : cancelled.promise, [a]);
  assert.equal(typeof start, 'function');
  const pending = start(a);
  await flush();
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { savedCount: 8, currentFolder: '已发送' }) });
  started.resolve(status('preparing'));
  await pending;
  assert.equal(S.bulkDownloads.a.savedCount, 8);
  const stopping = cancel(a);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('cancelled', { savedCount: 9 }) });
  cancelled.resolve(status('cancelling'));
  await stopping;
  assert.equal(S.bulkDownloads.a.state, 'cancelled');
  assert.equal(S.bulkDownloads.a.savedCount, 9);
});

test('progress updates only matching runtime nodes, keeps editable settings drafts and survives reopening', () => {
  const a = section('a'), b = section('b'), { S, onMailEvent, openSettings, overlay } = app(undefined, [a, b]);
  S.autoDownloads.a = { state: 'waiting' };
  overlay.innerHTML = '设置表单中有尚未保存的签名和过滤关键词';
  const before = plain(S.accounts);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { detail: '正在保存', savedCount: 3, existingCount: 2, failedCount: 1, foldersDone: 2, currentFolder: '<工作资料>' }) });
  assert.equal(S.bulkDownloads?.a?.savedCount, 3);
  assert.match(a.nodes['.bulk-download-counts'].textContent, /已保存 3 封[\s\S]*已有 2 封[\s\S]*失败 1 项[\s\S]*2 \/ 5/);
  assert.match(a.nodes['.bulk-download-directory'].textContent, /\/tmp\/archive\/account/);
  assert.match(a.nodes['.bulk-download-folder'].textContent, /<工作资料>/);
  assert.equal(b.nodes['.bulk-download-runtime'].textContent, '');
  assert.deepEqual(plain(S.accounts), before);
  assert.equal(S.autoDownloads.a.state, 'waiting');
  assert.equal(overlay.innerHTML, '设置表单中有尚未保存的签名和过滤关键词');
  openSettings();
  assert.match(a.nodes['.bulk-download-runtime'].textContent, /正在保存/);
});

test('completion that wins a cancellation race stays complete and no new cancel request is allowed', async () => {
  const request = deferred(), calls = [], a = section('a');
  const { cancel, onMailEvent, S } = app((cmd, args) => { calls.push({ cmd, args }); return request.promise; }, [a]);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading') });
  const pending = cancel(a);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { savedCount: 5 }) });
  request.resolve(status('cancelling'));
  await pending;
  await cancel(a);
  assert.equal(calls.length, 1);
  assert.equal(S.bulkDownloads.a.state, 'complete');
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].hidden, true);
  assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, false);
});

test('picker and start errors recover controls and offer retry without changing account settings', async () => {
  for (const failing of ['choose_download_directory', 'start_bulk_download']) {
    const a = section('a'), { start, S } = app(async cmd => { if (cmd === failing) throw new Error('目录不可写'); return '/tmp/chosen'; }, [a]);
    assert.equal(typeof start, 'function');
    const before = plain(S.accounts);
    await start(a);
    assert.match(a.nodes['.bulk-download-error'].textContent, /目录不可写/);
    assert.equal(a.nodes['.bulk-download-error'].hidden, false);
    assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, false);
    assert.deepEqual(plain(S.accounts), before);
  }
});

test('cancel is account scoped, suppresses duplicate requests and preserves task status on command error', async () => {
  const request = deferred(), calls = [], a = section('a'), b = section('b');
  const { cancel, onMailEvent, S } = app((cmd, args) => { calls.push({ cmd, args }); return request.promise; }, [a, b]);
  assert.equal(typeof cancel, 'function');
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { savedCount: 5 }) });
  onMailEvent({ type: 'bulkDownload', accountId: 'b', ...status('downloading', { savedCount: 10 }) });
  const pending = cancel(a);
  await cancel(a);
  assert.deepEqual(plain(calls), [{ cmd: 'cancel_bulk_download', args: { accountId: 'a' } }]);
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].disabled, true);
  assert.equal(b.nodes['[data-action="cancel-bulk-download"]'].disabled, false);
  request.reject(new Error('取消请求失败'));
  await pending;
  assert.equal(S.bulkDownloads.a.savedCount, 5);
  assert.equal(S.bulkDownloads.b.savedCount, 10);
  assert.match(a.nodes['.bulk-download-error'].textContent, /取消请求失败/);
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].disabled, false);
});

test('terminal events permit restarting and reuse the chosen parent directory, not the account output subfolder', async () => {
  const a = section('a'), calls = [];
  const { start, onMailEvent } = app(async (cmd, args) => { calls.push({ cmd, args }); return cmd === 'choose_download_directory' ? '/tmp/chosen' : status('complete'); }, [a]);
  assert.equal(typeof start, 'function');
  await start(a);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('error', { detail: '部分邮件下载失败', failedCount: 2 }) });
  assert.equal(a.nodes['[data-action="start-bulk-download"]'].disabled, false);
  assert.equal(a.nodes['[data-action="cancel-bulk-download"]'].hidden, true);
  assert.equal(a.nodes['.bulk-download-runtime'].classList.contains('err'), true);
  await start(a);
  assert.deepEqual(plain(calls[2]), { cmd: 'choose_download_directory', args: { directory: '/tmp/chosen' } });
});

test('startup restores account task status and keeps events newer than the snapshot', async () => {
  const request = deferred(), { startBoot, onMailEvent, S } = app(() => request.promise);
  const pending = startBoot();
  await Promise.resolve();
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { savedCount: 20 }) });
  request.resolve({ accounts: S.accounts, live: {}, bulkDownloads: { a: status('downloading', { savedCount: 10 }), b: status('cancelled', { savedCount: 2 }) } });
  await pending;
  assert.equal(S.bulkDownloads?.a?.savedCount, 20);
  assert.equal(S.bulkDownloads?.b?.state, 'cancelled');
});

function changeInheritance(listeners, box, checked) {
  const input = box.nodes['.bulk-download-inherit'];
  input.checked = checked;
  input.classList.toggle('bulk-download-inherit', true);
  input.closest = selector => selector === '.bulk-download-inherit' ? input : selector === '.bulk-download-section' ? box : null;
  listeners.change({ target: input });
}

test('inheritance is unchecked by default, account scoped, and retained when settings reopen', () => {
  const a = section('a'), b = section('b');
  const { openSettings, listeners, S } = app(undefined, [a, b]);
  const before = plain(S.accounts);
  openSettings();
  assert.equal(a.nodes['.bulk-download-inherit'].checked, false);
  changeInheritance(listeners, a, true);
  a.nodes['.bulk-download-inherit'].checked = false;
  openSettings();
  assert.equal(a.nodes['.bulk-download-inherit'].checked, true);
  assert.equal(b.nodes['.bulk-download-inherit'].checked, false);
  assert.deepEqual(plain(S.accounts), before);
});

test('inheritance captures the toggle before directory selection and disables changes until terminal status', async () => {
  const picker = deferred(), started = deferred(), calls = [], a = section('a'), b = section('b');
  const { start, listeners, openSettings, onMailEvent } = app((cmd, args) => { calls.push({ cmd, args }); return cmd === 'choose_download_directory' ? picker.promise : started.promise; }, [a, b]);
  changeInheritance(listeners, a, true);
  const pending = start(a);
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, true);
  assert.equal(b.nodes['.bulk-download-inherit'].disabled, false);
  changeInheritance(listeners, a, false);
  openSettings();
  assert.equal(a.nodes['.bulk-download-inherit'].checked, true);
  picker.resolve('/tmp/chosen');
  await flush();
  assert.equal(calls[1].args.inheritFilters, true);
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, true);
  started.resolve(status('preparing', { inheritFilters: true, filterMode: 'whitelist' }));
  await pending;
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, true);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('cancelled', { inheritFilters: true, filterMode: 'whitelist' }) });
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, false);
  assert.equal(a.nodes['.bulk-download-inherit'].checked, true);
});

test('saved rule hint ignores draft form input and active task reports its original effective mode', () => {
  const a = section('a'), { S, listeners, renderStatus, onMailEvent, overlay } = app(undefined, [a]);
  changeInheritance(listeners, a, true);
  assert.match(a.nodes['.bulk-download-saved-rule'].textContent, /已保存.*白名单.*账单/);
  overlay.innerHTML = '未保存的黑名单：广告';
  renderStatus('a');
  assert.doesNotMatch(a.nodes['.bulk-download-saved-rule'].textContent, /广告/);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { inheritFilters: true, filterMode: 'whitelist', skippedCount: 3 }) });
  S.accounts[0].autoDownload = { enabled: false, directory: '', filterMode: 'blacklist', keywords: ['新规则'] };
  renderStatus('a');
  assert.match(a.nodes['.bulk-download-saved-rule'].textContent, /已保存.*黑名单.*新规则/);
  assert.match(a.nodes['.bulk-download-task-rule'].textContent, /本次.*白名单/);
  assert.doesNotMatch(a.nodes['.bulk-download-task-rule'].textContent, /黑名单|新规则/);
  assert.match(a.nodes['.bulk-download-counts'].textContent, /跳过 3 封/);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { detail: '', inheritFilters: true, filterMode: 'whitelist', skippedCount: 3 }) });
  assert.match(a.nodes['.bulk-download-runtime'].textContent, /符合规则的邮件下载完成/);
  assert.doesNotMatch(a.nodes['.bulk-download-runtime'].textContent, /全部邮件下载完成/);
});

test('inheriting inactive rules clearly reports no filtering and cancelled picker keeps the selection', async () => {
  const a = section('a'), { S, listeners, start, renderStatus, onMailEvent } = app(async () => null, [a]);
  S.accounts[0].autoDownload = { enabled: false, directory: '', filterMode: 'none', keywords: ['旧草稿'] };
  changeInheritance(listeners, a, true);
  renderStatus('a');
  assert.match(a.nodes['.bulk-download-saved-rule'].textContent, /未启用.*不做过滤/);
  assert.doesNotMatch(a.nodes['.bulk-download-saved-rule'].textContent, /旧草稿/);
  await start(a);
  assert.equal(a.nodes['.bulk-download-inherit'].checked, true);
  assert.equal(a.nodes['.bulk-download-inherit'].disabled, false);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { inheritFilters: true, filterMode: 'none' }) });
  assert.match(a.nodes['.bulk-download-task-rule'].textContent, /继承.*未启用.*不做过滤/);
});


test('field rule previews are bounded and active tasks display their own scope snapshot', () => {
  const a = section('a'), { S, renderStatus, onMailEvent } = app(undefined, [a]);
  S.accounts[0].autoDownload = { filterMode: 'blacklist', keywordScope: 'fields', keywords: ['不应显示的旧任意词'], fieldKeywords: { subject: ['<img src=x>', '😀'.repeat(200), '第三个', '第四个'], sender: ['销售组'], recipient: ['customer@example.com'], body: [] } };
  renderStatus('a');
  const preview = a.nodes['.bulk-download-saved-rule'].textContent;
  for (const text of ['按字段', '邮件主题', '<img src=x>', '发送人', '销售组', '收件人', 'customer@example.com', '4 个']) assert.ok(preview.includes(text), text);
  assert.ok([...preview].length < 450);
  assert.doesNotMatch(preview, /旧任意词|第四个/);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('downloading', { inheritFilters: true, filterMode: 'blacklist', keywordScope: 'fields' }) });
  assert.match(a.nodes['.bulk-download-task-rule'].textContent, /按字段.*任意一个字段/);
  S.accounts[0].autoDownload = { filterMode: 'whitelist', keywordScope: 'any', keywords: ['新规则'] };
  renderStatus('a');
  assert.match(a.nodes['.bulk-download-saved-rule'].textContent, /任意命中.*新规则/);
  assert.match(a.nodes['.bulk-download-task-rule'].textContent, /黑名单.*按字段/);
  assert.doesNotMatch(a.nodes['.bulk-download-task-rule'].textContent, /白名单|新规则/);
  onMailEvent({ type: 'bulkDownload', accountId: 'a', ...status('complete', { inheritFilters: true, filterMode: 'blacklist' }) });
  assert.match(a.nodes['.bulk-download-task-rule'].textContent, /任意命中/);
});
