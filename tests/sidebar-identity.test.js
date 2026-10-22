const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// Point at an isolated ui copy to check the same assertions against a baseline.
const ui = process.env.MAILBEACON_UI_DIR || path.join(__dirname, '../ui');
const plain = value => JSON.parse(JSON.stringify(value));

function setup() {
  const sidebar = { innerHTML: '' }, menus = [], navigations = [];
  const context = vm.createContext({
    window: {
      MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'),
      MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'),
      MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'),
      __TAURI__: { core: { invoke: async () => [] }, event: { listen: async () => {} } },
    },
    document: { addEventListener() {}, querySelector: selector => selector === '#sidebar' ? sidebar : null },
    console, setTimeout, clearTimeout,
    captureMenu: (_anchor, entries) => menus.push(entries),
    navigate: (folder, account) => navigations.push({ folder, account }),
  });
  const source = fs.readFileSync(path.join(ui, 'app.js'), 'utf8');
  vm.runInContext(source.slice(0, source.indexOf('(async function boot()')) + `
    showMenu = captureMenu;
    go = navigate;
    globalThis.api = { S, renderSidebar, accountMenu, syncSummary, rowHtml,
      accountBadge: typeof accountBadge === 'function' ? accountBadge : undefined };
  `, context);
  const api = context.api;
  api.S.accounts = [
    { id: 'a', kind: 'gmail', email: 'personal@example.test', name: 'Personal' },
    { id: 'b', kind: 'exmail', email: 'work@example.test', name: 'Work' },
  ];
  api.S.acct = 'all';
  api.S.live = { a: { state: 'online', detail: 'IMAP IDLE connected' }, b: { state: 'online', detail: 'Ready' } };
  return { ...api, menus, navigations, sidebar };
}

for (const [kind, label] of [['gmail', 'Gmail'], ['exmail', '腾讯企业邮'], ['qq', 'QQ 邮箱'], ['163', '网易 163'], ['126', '网易 126'], ['custom', '其他']]) {
  test(`account identity follows explicit ${kind} kind, including custom-domain addresses`, () => {
    const t = setup();
    assert.equal(typeof t.accountBadge, 'function', 'account badges must have one reusable renderer');
    const html = t.accountBadge({ kind, email: 'same@custom-company.test', name: 'Same' });
    assert.match(html, new RegExp(`data-provider="${kind}"`));
    assert.ok(html.includes(`title="${label}"`));
    assert.match(html, /aria-hidden="true"/);
  });
}

test('all accounts uses its own neutral identity without an individual source color', () => {
  const t = setup();
  assert.equal(typeof t.accountBadge, 'function');
  const html = t.accountBadge(null);
  assert.match(html, /data-provider="all"/);
  assert.match(html, /title="所有账户"/);
  assert.doesNotMatch(html, /account-color/);
});

test('missing and untrusted kinds fall back to the generic mail badge safely', () => {
  const t = setup();
  assert.equal(typeof t.accountBadge, 'function');
  for (const kind of [undefined, 'outlook', '\"><img src=x onerror=alert(1)>']) {
    const html = t.accountBadge({ kind, email: '\"><svg onload=alert(1)>@gmail.com' });
    assert.match(html, /data-provider="custom"/);
    assert.doesNotMatch(html, /<img|onerror|onload/);
    assert.doesNotMatch(html, /data-provider="gmail"/, 'the address domain must not override account kind');
  }
});

test('account labels and full address titles remain escaped in the sidebar', () => {
  const t = setup();
  t.S.accounts[0] = { id: 'a"<', kind: 'gmail', name: '<script>name</script>', email: 'x"<&\'@example.test' };
  t.S.acct = 'all';
  t.renderSidebar();
  assert.match(t.sidebar.innerHTML, /data-switch="a&quot;&lt;"/);
  assert.match(t.sidebar.innerHTML, /title="x&quot;&lt;&amp;&#39;@example.test"/);
  assert.doesNotMatch(t.sidebar.innerHTML, /<script>|<img/);
  t.S.acct = t.S.accounts[0].id;
  t.renderSidebar();
  assert.match(t.sidebar.innerHTML, /&lt;script&gt;name&lt;\/script&gt;/);
});

test('account picker supplies identity and exactly one selection for all and individual accounts', () => {
  const t = setup();
  for (const selected of ['all', 'a', 'b']) {
    t.S.acct = selected;
    t.accountMenu({});
    const entries = t.menus.at(-1).filter(entry => entry !== '-' && entry.check !== undefined);
    assert.deepEqual(plain(entries.map(entry => entry.account?.id ?? (entry.account === null ? 'all' : 'missing'))), ['all', 'a', 'b']);
    assert.deepEqual(plain(entries.filter(entry => entry.check).map(entry => entry.account?.id ?? 'all')), [selected]);
    entries.find(entry => entry.account?.id === 'b').run();
    assert.deepEqual(t.navigations.at(-1), { folder: 'INBOX', account: 'b' });
  }
});

test('account source color is identical in badges and aggregate message rows', () => {
  const t = setup();
  assert.equal(typeof t.accountBadge, 'function');
  for (const a of t.S.accounts) {
    const badge = t.accountBadge(a);
    const row = t.rowHtml({ latest: { accountId: a.id, folder: 'INBOX', uid: 1, fromName: 'Sender', fromAddr: 'sender@example.test', subject: 'Subject', date: Date.now(), seen: true }, count: 1, unread: 0 }, false, true);
    const sourceColor = badge.match(/class="account-color" style="background:([^"]+)"/)?.[1];
    assert.ok(sourceColor);
    assert.equal(row.match(/class="acct-dot" style="background:([^"]+)"/)?.[1], sourceColor);
  }
});

for (const [state, cls, text] of [['online', '', '已同步 · 实时推送'], ['connecting', 'connecting', '正在同步…'], ['error', 'error', 'personal@example.test 登录失败'], ['offline', 'offline', '网络断开，正在重连…']]) {
  test(`${state} status preserves its meaning and full account details in the title`, () => {
    const t = setup();
    const detail = 'Very long server detail: '.repeat(20) + '"<&\' END';
    t.S.live.a = { state, detail };
    assert.deepEqual(plain(t.syncSummary()), { cls, text });
    t.renderSidebar();
    const html = t.sidebar.innerHTML;
    assert.ok(html.includes(text));
    assert.ok(html.includes('personal@example.test：Very long server detail: '));
    assert.match(html, /&quot;&lt;&amp;&#39; END/);
    assert.ok(html.includes('work@example.test：Ready'));
    assert.match(html, /class="sync-text"/);
  });
}

test('settings and sync status share a bottom container after the scrollable navigation', () => {
  const t = setup();
  t.renderSidebar();
  const html = t.sidebar.innerHTML;
  const bottom = html.indexOf('<div class="side-bottom">');
  assert.ok(bottom > html.indexOf('<div class="side-scroll">'));
  assert.ok(html.indexOf('data-action="settings"') > bottom);
  assert.ok(html.indexOf('class="side-foot"') > bottom);
  assert.equal((html.match(/data-action="settings"/g) || []).length, 1);
});
