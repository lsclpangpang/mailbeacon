const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

const plain = value => JSON.parse(JSON.stringify(value));
function node(extra = {}) {
  const classes = new Set();
  return { hidden: false, disabled: false, textContent: '', value: '',
    classList: { add: c => classes.add(c), remove: c => classes.delete(c), contains: c => classes.has(c), toggle: (c, yes) => yes ? classes.add(c) : classes.delete(c) },
    ...extra };
}
function form(accountId, enabled = false, directory = '', filterMode = 'none', keywords = '') {
  const nodes = {
    '[name="enabled"]': node({ checked: enabled }),
    '[name="directory"]': node({ value: directory }),
    '[name="filterMode"]': node({ value: filterMode }),
    '[name="keywords"]': node({ value: keywords, disabled: filterMode === 'none' }),
    'button[type="submit"]': node({ textContent: '保存设置' }),
    '[data-action="choose-download-directory"]': node(),
    '.auto-download-status': node({ hidden: true }),
    '.auto-download-runtime': node(),
    '.auto-download-last': node({ hidden: true }),
  };
  return { dataset: { autoDownloadAccount: accountId }, nodes,
    querySelector: selector => nodes[selector] || null,
    querySelectorAll: selector => Object.entries(nodes).filter(([key]) => key.startsWith('[name=') ? (selector.includes('input') && !/filterMode|keywords/.test(key)) || (selector.includes('select') && key.includes('filterMode')) || (selector.includes('textarea') && key.includes('keywords')) : selector.includes('button') && /button|data-action/.test(key)).map(([, value]) => value),
  };
}
function app(invoke = async () => {}, forms = []) {
  const listeners = {};
  const overlay = { innerHTML: '', querySelectorAll: () => forms };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: { querySelector: s => s === '#overlay' ? overlay : null, addEventListener: (n, f) => { listeners[n] = f; } },
    console, setTimeout, clearTimeout,
  });
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    render = renderList = renderReader = updateBadge = loadPreviews = loadFolders = function () {};
    globalThis.api = { S, openSettings, onMailEvent, saveSignature,
      settings: typeof autoDownloadSettings === 'function' ? autoDownloadSettings : null,
      save: typeof saveAutoDownload === 'function' ? saveAutoDownload : null,
      choose: typeof chooseAutoDownloadDirectory === 'function' ? chooseAutoDownloadDirectory : null,
      changed: typeof autoDownloadChanged === 'function' ? autoDownloadChanged : null,
      status: typeof renderAutoDownloadStatus === 'function' ? renderAutoDownloadStatus : null };
  `, context);
  const api = context.api;
  api.S.accounts = [{ id: 'a', email: 'a@example.com', autoDownload: { enabled: false, directory: '' } }, { id: 'b', email: 'b@example.com', autoDownload: { enabled: true, directory: '/Users/me/B' } }];
  return { ...api, listeners, overlay,
    startBoot: () => vm.runInContext(source.slice(source.indexOf('(async function boot()'), source.indexOf('// 第一次添加账号后')), context),
  };
}
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test('old accounts default to automatic downloads off with no path', () => {
  const { settings } = app();
  assert.equal(typeof settings, 'function');
  assert.deepEqual(plain(settings({ id: 'legacy' })), { enabled: false, directory: '', filterMode: 'none', keywords: [] });
});

test('settings expose independent account controls with explicit new-mail scope', () => {
  const { openSettings, overlay } = app();
  openSettings();
  assert.match(overlay.innerHTML, /data-auto-download-account="a"/);
  assert.match(overlay.innerHTML, /data-auto-download-account="b"/);
  assert.match(overlay.innerHTML, /自动下载新邮件/);
  assert.match(overlay.innerHTML, /历史邮件/);
  assert.match(overlay.innerHTML, /\.eml/);
});

test('saving affects only that account after backend success and prevents duplicate submission', async () => {
  const request = deferred(), calls = [], f = form('a', true, '/Users/me/邮件');
  const { S, save } = app((cmd, args) => { calls.push({ cmd, args }); return request.promise; }, [f]);
  assert.equal(typeof save, 'function');
  const pending = save(f);
  await save(f);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].cmd, 'save_auto_download');
  assert.deepEqual(plain(calls[0].args), { accountId: 'a', enabled: true, directory: '/Users/me/邮件', filterMode: 'none', keywords: [] });
  assert.equal(S.accounts[0].autoDownload.enabled, false);
  assert.equal(S.accounts[1].autoDownload.directory, '/Users/me/B');
  request.resolve({ id: 'a', autoDownload: { enabled: true, directory: '/Users/me/邮件' } });
  await pending;
  assert.equal(S.accounts[0].autoDownload.enabled, true);
  assert.equal(f.nodes['button[type="submit"]'].disabled, false);
  assert.match(f.nodes['.auto-download-status'].textContent, /已开启/);
});

test('offline or invalid-path save errors keep the prior configuration and editable draft', async () => {
  const f = form('a', true, '/not-writable');
  const { S, save, changed } = app(async () => { throw new Error('目录不可写'); }, [f]);
  assert.equal(typeof save, 'function');
  await save(f);
  assert.equal(S.accounts[0].autoDownload.enabled, false);
  assert.equal(f.nodes['[name="directory"]'].value, '/not-writable');
  assert.equal(f.nodes['.auto-download-status'].classList.contains('err'), true);
  assert.match(f.nodes['.auto-download-status'].textContent, /目录不可写/);
  assert.equal(f.nodes['button[type="submit"]'].disabled, false);
  changed(f);
  assert.equal(f.nodes['.auto-download-status'].classList.contains('err'), false);
  assert.match(f.nodes['.auto-download-status'].textContent, /尚未保存/);
});

test('disabling works with an empty path and reports success only after backend response', async () => {
  const f = form('b', false, '');
  const { S, save } = app(async (cmd, args) => ({ id: args.accountId, autoDownload: { enabled: false, directory: '' } }), [f]);
  assert.equal(typeof save, 'function');
  await save(f);
  assert.equal(S.accounts[1].autoDownload.enabled, false);
  assert.match(f.nodes['.auto-download-status'].textContent, /已关闭/);
});

test('folder picker cancellation preserves typed path and never saves settings', async () => {
  const calls = [], f = form('a', true, '/Users/me/original');
  const { choose } = app(async (cmd, args) => { calls.push({ cmd, args }); return null; }, [f]);
  assert.equal(typeof choose, 'function');
  await choose(f);
  assert.deepEqual(plain(calls), [{ cmd: 'choose_download_directory', args: { directory: '/Users/me/original' } }]);
  assert.equal(f.nodes['[name="directory"]'].value, '/Users/me/original');
  assert.equal(f.nodes['[data-action="choose-download-directory"]'].disabled, false);
});

test('folder picker selection changes only the draft and picker failures are visible', async () => {
  const f = form('a', true, '/Users/me/original');
  const { S, choose } = app(async () => '/Users/me/新的目录', [f]);
  assert.equal(typeof choose, 'function');
  await choose(f);
  assert.equal(f.nodes['[name="directory"]'].value, '/Users/me/新的目录');
  assert.equal(S.accounts[0].autoDownload.enabled, false);
  const second = app(async () => { throw new Error('目录选择器不可用'); }, [f]);
  await second.choose(f);
  assert.match(f.nodes['.auto-download-status'].textContent, /目录选择器不可用/);
  assert.equal(f.nodes['[name="directory"]'].value, '/Users/me/新的目录');
});

test('runtime events update only the matching status and never clobber form edits', () => {
  const a = form('a', true, '/Users/me/unsaved'), b = form('b', true, '/Users/me/B');
  const { S, onMailEvent } = app(undefined, [a, b]);
  onMailEvent({ type: 'autoDownload', accountId: 'a', state: 'error', detail: '磁盘已满，稍后重试', lastPath: '/Users/me/old.eml', savedCount: 2 });
  assert.equal(S.autoDownloads?.a?.state, 'error');
  assert.match(a.nodes['.auto-download-runtime'].textContent, /磁盘已满/);
  assert.equal(a.nodes['.auto-download-runtime'].classList.contains('err'), true);
  assert.match(a.nodes['.auto-download-last'].textContent, /old.eml/);
  assert.equal(a.nodes['[name="directory"]'].value, '/Users/me/unsaved');
  assert.equal(a.nodes['[name="enabled"]'].checked, true);
  assert.equal(b.nodes['.auto-download-runtime'].textContent, '');
  onMailEvent({ type: 'autoDownload', accountId: 'a', state: 'waiting', detail: '等待新邮件', savedCount: 3 });
  assert.equal(a.nodes['.auto-download-runtime'].classList.contains('err'), false);
  assert.match(a.nodes['.auto-download-runtime'].textContent, /3/);
});

test('concurrent signature and automatic download responses cannot overwrite each other', async () => {
  const signature = deferred(), download = deferred(), f = form('a', true, '/Users/me/Mail');
  const { S, save, saveSignature } = app(cmd => cmd === 'save_signature' ? signature.promise : download.promise, [f]);
  const signatureNodes = { 'button[type="submit"]': node(), '.signature-status': node(), textarea: node({ value: '新的签名' }) };
  const signatureForm = { dataset: { signatureAccount: 'a' }, querySelector: s => signatureNodes[s] };
  const signaturePending = saveSignature(signatureForm);
  const downloadPending = save(f);
  download.resolve({ id: 'a', signature: '旧的签名', autoDownload: { enabled: true, directory: '/Users/me/Mail' } });
  await downloadPending;
  signature.resolve({ id: 'a', signature: '新的签名', autoDownload: { enabled: false, directory: '' } });
  await signaturePending;
  assert.equal(S.accounts[0].signature, '新的签名');
  assert.equal(S.accounts[0].autoDownload.enabled, true);
});

test('startup restores per-account status without overwriting a newer event received during snapshot', async () => {
  const request = deferred();
  const { S, startBoot, onMailEvent } = app(() => request.promise);
  const pending = startBoot();
  await Promise.resolve();
  onMailEvent({ type: 'autoDownload', accountId: 'a', state: 'error', detail: '新的磁盘错误', savedCount: 2 });
  request.resolve({ accounts: S.accounts, live: {}, autoDownloads: {
    a: { state: 'waiting', detail: '旧状态' }, b: { state: 'waiting', detail: '等待新邮件', savedCount: 5 },
  } });
  await pending;
  assert.equal(S.autoDownloads.a.detail, '新的磁盘错误');
  assert.equal(S.autoDownloads.b.savedCount, 5);
});


test('keyword filtering is one mutually exclusive select with clear scope and saved values', () => {
  const { S, openSettings, overlay } = app();
  S.accounts[0].autoDownload = { enabled: true, directory: '/Users/me/Mail', filterMode: 'blacklist', keywords: ['优惠', '<GitHub>'] };
  openSettings();
  assert.match(overlay.innerHTML, /<select[^>]+name="filterMode"/);
  assert.doesNotMatch(overlay.innerHTML, /<select[^>]+multiple/);
  assert.match(overlay.innerHTML, /value="none"[^>]*>不过滤/);
  assert.match(overlay.innerHTML, /value="whitelist"[^>]*>白名单/);
  assert.match(overlay.innerHTML, /value="blacklist" selected[^>]*>黑名单/);
  assert.match(overlay.innerHTML, /name="keywords"/);
  assert.match(overlay.innerHTML, /优惠\n&lt;GitHub&gt;/);
  for (const text of ['任一关键词', '主题', '发件人', '正文', '附件', '大小写', '保存后的新邮件', '100', '200']) assert.ok(overlay.innerHTML.includes(text), text);
});

test('keyword save normalizes blanks and case duplicates then accepts backend normalization for that account', async () => {
  const request = deferred(), calls = [], f = form('a', true, '/Users/me/Mail', 'whitelist', '  GitHub  \n\n优惠\n github\n优惠\n');
  const { S, save } = app((cmd, args) => { calls.push({ cmd, args }); return request.promise; }, [f]);
  const other = plain(S.accounts[1]);
  const pending = save(f);
  assert.deepEqual(plain(calls[0].args), { accountId: 'a', enabled: true, directory: '/Users/me/Mail', filterMode: 'whitelist', keywords: ['GitHub', '优惠'] });
  for (const key of ['[name="enabled"]', '[name="directory"]', '[name="filterMode"]', '[name="keywords"]', 'button[type="submit"]']) assert.equal(f.nodes[key].disabled, true, key);
  request.resolve({ id: 'a', autoDownload: { enabled: true, directory: '/Users/me/Mail', filterMode: 'whitelist', keywords: ['github', '优惠'] } });
  await pending;
  assert.equal(f.nodes['[name="keywords"]'].value, 'github\n优惠');
  assert.equal(f.nodes['[name="filterMode"]'].value, 'whitelist');
  assert.equal(f.nodes['[name="keywords"]'].disabled, false);
  assert.equal(f.nodes['[name="filterMode"]'].disabled, false);
  assert.deepEqual(plain(S.accounts[1]), other);
});

test('blacklist and whitelist reject an empty list even while automatic downloads are disabled', async () => {
  for (const filterMode of ['blacklist', 'whitelist']) for (const enabled of [true, false]) {
    const calls = [], f = form('a', enabled, '/Users/me/Mail', filterMode, ' \n \n');
    const { save } = app((...args) => calls.push(args), [f]);
    await save(f);
    assert.equal(calls.length, 0);
    assert.match(f.nodes['.auto-download-status'].textContent, /至少.*关键词/);
    assert.equal(f.nodes['[name="filterMode"]'].disabled, false);
  }
});

test('keyword limits reject oversized lists and entries without losing typed values', async () => {
  for (const [keywords, expected] of [[Array.from({ length: 101 }, (_, i) => '词' + i).join('\n'), /100/], ['词'.repeat(201), /200/]]) {
    const calls = [], f = form('a', false, '', 'blacklist', keywords);
    const { save } = app((...args) => calls.push(args), [f]);
    await save(f);
    assert.equal(calls.length, 0);
    assert.match(f.nodes['.auto-download-status'].textContent, expected);
    assert.equal(f.nodes['[name="keywords"]'].value, keywords);
  }
});

test('a 200-character Unicode keyword is accepted and none mode retains inactive keywords', async () => {
  const keywords = '😀'.repeat(200), calls = [], f = form('a', false, '', 'none', keywords + '\n  GitHub  ');
  const { save } = app(async (cmd, args) => { calls.push(args); return { id: 'a', autoDownload: args }; }, [f]);
  await save(f);
  assert.deepEqual(plain(calls[0].keywords), [keywords, 'GitHub']);
  assert.equal(calls[0].filterMode, 'none');
  assert.equal(f.nodes['[name="keywords"]'].disabled, true);
  assert.equal(f.nodes['[name="keywords"]'].value, keywords + '\nGitHub');
});

test('changing filter mode preserves keyword draft and controls its availability', () => {
  const f = form('a', false, '', 'none', '保留的关键词');
  const { listeners } = app(undefined, [f]);
  const target = f.nodes['[name="filterMode"]'];
  target.closest = () => f;
  target.value = 'blacklist';
  listeners.change({ target });
  assert.equal(f.nodes['[name="keywords"]'].disabled, false);
  assert.match(f.nodes['.auto-download-status'].textContent, /尚未保存/);
  target.value = 'none';
  listeners.change({ target });
  assert.equal(f.nodes['[name="keywords"]'].disabled, true);
  assert.equal(f.nodes['[name="keywords"]'].value, '保留的关键词');
});

test('failed filter save preserves the mode and text and runtime updates leave them alone', async () => {
  const f = form('a', true, '/Users/me/Mail', 'blacklist', ' 广告\n促销 ');
  const { S, save, onMailEvent } = app(async () => { throw new Error('保存失败'); }, [f]);
  await save(f);
  onMailEvent({ type: 'autoDownload', accountId: 'a', state: 'waiting', detail: '等待新邮件' });
  assert.equal(f.nodes['[name="filterMode"]'].value, 'blacklist');
  assert.equal(f.nodes['[name="keywords"]'].value, ' 广告\n促销 ');
  assert.equal(f.nodes['[name="keywords"]'].disabled, false);
  assert.equal(f.nodes['[name="filterMode"]'].disabled, false);
  assert.equal(S.accounts[0].autoDownload.enabled, false);
});
