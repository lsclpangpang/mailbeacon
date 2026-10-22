const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

// Point at an isolated ui copy to check the same assertions against a baseline.
const ui = process.env.MAILBEACON_UI_DIR || path.join(__dirname, '../ui');
const plain = value => JSON.parse(JSON.stringify(value));

function memoryStorage() {
  const entries = new Map();
  return {
    getItem: key => entries.get(String(key)) ?? null,
    setItem: (key, value) => entries.set(String(key), String(value)),
    removeItem: key => entries.delete(String(key)),
    clear: () => entries.clear(),
  };
}

function setup({ storage = memoryStorage() } = {}) {
  const sidebar = { innerHTML: '' }, menus = [], navigations = [];
  const context = vm.createContext({
    window: {
      localStorage: storage,
      MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'),
      MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'),
      MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'),
      __TAURI__: { core: { invoke: async () => [] }, event: { listen: async () => {} } },
    },
    document: { addEventListener() {}, querySelector: selector => selector === '#sidebar' ? sidebar : null },
    console, setTimeout, clearTimeout, localStorage: storage,
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
  return { ...api, menus, navigations, sidebar, storage };
}

// All three addresses mapped to the same color in the original email-hash palette.
const collidingAccounts = () => [0, 2, 4].map(index => ({
  id: `account-${index}`, kind: 'gmail', email: `account${index}@example.test`, name: `Account ${index}`,
}));
const badgeColor = (t, account) => {
  const color = t.accountBadge(account).match(/class="account-color" style="background:([^"]+)"/)?.[1];
  assert.ok(color, 'each account badge must expose its source color');
  return color;
};
const badgeColors = t => Object.fromEntries(t.S.accounts.map(account => [account.id, badgeColor(t, account)]));

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

test('accounts whose email hashes collide receive distinct source colors', () => {
  const t = setup();
  t.S.accounts = collidingAccounts();
  const colors = Object.values(badgeColors(t));
  assert.equal(new Set(colors).size, 3, 'three simultaneous accounts must remain visually distinguishable');
});

test('assigned account colors survive reordering, account switching, status changes and account additions or removals', () => {
  const t = setup();
  const accounts = collidingAccounts();
  t.S.accounts = accounts;
  const original = badgeColors(t);

  t.S.accounts = [accounts[2], accounts[0], accounts[1]];
  for (const selected of ['all', accounts[1].id, accounts[0].id]) {
    t.S.acct = selected;
    t.S.live = Object.fromEntries(accounts.map((account, index) => [account.id, { state: ['online', 'connecting', 'error'][index] }]));
    t.renderSidebar();
    assert.deepEqual(badgeColors(t), original);
  }

  const added = { id: 'new-account', kind: 'custom', email: 'account11@example.test', name: 'Added account' };
  t.S.accounts = [added, accounts[2], accounts[0]];
  const withAdded = badgeColors(t);
  for (const account of [accounts[2], accounts[0]]) assert.equal(withAdded[account.id], original[account.id]);
  assert.equal(new Set(Object.values(withAdded)).size, t.S.accounts.length, 'a new account must not take an active account color');

  t.S.accounts = [accounts[1], added, accounts[0], accounts[2]];
  for (const account of accounts) assert.equal(badgeColor(t, account), original[account.id], 'restoring an account must restore its assigned color');
  assert.equal(badgeColor(t, added), withAdded[added.id]);
  assert.equal(new Set(Object.values(badgeColors(t))).size, t.S.accounts.length, 'restoring an account must not create a color collision');
});

test('account colors persist across app setup with the same local storage and a different account order', () => {
  const storage = memoryStorage();
  const first = setup({ storage });
  first.S.accounts = collidingAccounts();
  const original = badgeColors(first);
  assert.ok(storage.getItem('mb.account-colors'), 'assigned account colors must be saved locally');

  const restarted = setup({ storage });
  restarted.S.accounts = collidingAccounts().reverse();
  assert.deepEqual(badgeColors(restarted), original, 'restart must reuse the saved account-to-color assignment');
});

test('account colors stay unique after the initial color palette is exhausted', () => {
  const t = setup();
  t.S.accounts = Array.from({ length: 24 }, (_, index) => ({
    id: `many-${index}`, kind: 'custom', email: `many-${index}@example.test`, name: `Account ${index}`,
  }));
  const original = badgeColors(t);
  assert.equal(new Set(Object.values(original)).size, t.S.accounts.length);
  t.S.accounts.reverse();
  assert.deepEqual(badgeColors(t), original, 'generated colors must remain assigned to the same account');
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
