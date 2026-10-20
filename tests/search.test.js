const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app() {
  const listeners = {};
  const overlay = { innerHTML: '', querySelectorAll: () => [] };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke: async () => {} }, event: { listen: async () => {} } } },
    document: {
      activeElement: null,
      querySelector: selector => selector === '#overlay' ? overlay : null,
      addEventListener: (name, handler) => { listeners[name] = handler; },
    },
    console,
    setTimeout,
    clearTimeout,
    listeners,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    let renderCount = 0;
    renderList = () => { renderCount++; };
    render = () => {};
    globalThis.searchApi = { S, currentList, onMailEvent, markAllRead, listeners, get renderCount() { return renderCount; } };
  `, context);
  const api = context.searchApi;
  // vm 里创建的数组与测试里的不是同一个 realm，deepStrictEqual 前先转成本地数组。
  return { ...api, currentList: () => [...api.currentList()], get renderCount() { return api.renderCount; } };
}

function input(value = '') {
  return {
    id: 'q', value, selectionStart: value.length,
    closest: () => null,
    focus() {},
    setSelectionRange() {},
  };
}

test('search waits for IME composition and never appends uncommitted pinyin', () => {
  const search = app();
  const { S, listeners } = search;
  const q = input();

  listeners.compositionstart({ target: q });
  q.value = '工贸bong\'zigong';
  listeners.input({ target: q, isComposing: true });
  assert.equal(S.query, '');
  assert.equal(search.renderCount, 0);

  q.value = '工贸';
  listeners.compositionend({ target: q });
  assert.equal(S.query, '工贸');
  assert.equal(search.renderCount, 1);
  assert.equal(S.searchComposing, false);
});

test('search matches sender, recipient, subject and loaded preview text', () => {
  const { S, currentList } = app();
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.lists.set('a|INBOX', [
    { accountId: 'a', folder: 'INBOX', uid: 1, fromName: 'GitHub', fromAddr: 'noreply@github.com', to: '我', subject: '构建完成', date: 2, seen: true },
    { accountId: 'a', folder: 'INBOX', uid: 2, fromName: '同事', fromAddr: 'team@example.com', to: '项目群', subject: '周报', date: 1, seen: true },
  ]);
  S.previews.set('a|INBOX|1', 'Release workflow passed');
  S.previews.set('a|INBOX|2', '接口联调完成');

  for (const [query, uid] of [['github', 1], ['项目', 2], ['workflow', 1], ['构建', 1]]) {
    S.query = query;
    assert.deepEqual(currentList().map(message => message.uid), [uid]);
  }
  S.query = '不存在';
  assert.deepEqual(currentList(), []);
});


test('server search results are shown as-is, including body-only matches', () => {
  const { S, currentList } = app();
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.lists.set('a|INBOX', [
    { accountId: 'a', folder: 'INBOX', uid: 1, fromName: 'x', fromAddr: 'x@example.com', to: '', subject: '无关', date: 1, seen: true },
  ]);
  S.query = '合同';
  assert.deepEqual(currentList(), []);
  // 关键词只在正文后半部分：服务器命中，但本地摘要里没有。
  S.results.set('a|INBOX', { query: '合同', items: [
    { accountId: 'a', folder: 'INBOX', uid: 7, fromName: 'y', fromAddr: 'y@example.com', to: '', subject: '附件', date: 2, seen: true },
  ] });
  assert.deepEqual(currentList().map(m => m.uid), [7]);
  // 结果属于旧关键词时不展示，回到本地即时筛选。
  S.query = '无关';
  assert.deepEqual(currentList().map(m => m.uid), [1]);
});

test('sync events during a search do not replace results or the cached list', () => {
  const { S, currentList, onMailEvent } = app();
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  const page = (from, n) => Array.from({ length: n }, (_, i) => (
    { accountId: 'a', folder: 'INBOX', uid: from - i, fromName: '', fromAddr: '', to: '', subject: 's', date: from - i, seen: true }));
  S.lists.set('a|INBOX', page(300, 200));
  S.query = '周报';
  S.results.set('a|INBOX', { query: '周报', items: page(42, 1) });
  onMailEvent({ type: 'inbox', accountId: 'a', messages: page(301, 100) });
  assert.deepEqual(currentList().map(m => m.uid), [42]);
  assert.equal(S.lists.get('a|INBOX').length, 100);
});

test('mark-all-read and flag sync update both search results and the folder list', async () => {
  const { S, onMailEvent, markAllRead } = app();
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  const msg = (uid) => ({ accountId: 'a', folder: 'INBOX', uid, fromName: '', fromAddr: '', to: '', subject: 's', date: uid, seen: false, flagged: false });
  S.lists.set('a|INBOX', [msg(1), msg(2)]);
  S.query = 's';
  S.results.set('a|INBOX', { query: 's', items: [msg(1), msg(2)] });
  await markAllRead().catch(() => {}); // toast 在测试环境里没有 DOM
  assert.deepEqual(S.lists.get('a|INBOX').map(m => m.seen), [true, true]);

  onMailEvent({ type: 'flags', accountId: 'a', minUid: 1, items: [{ uid: 1, seen: false, flagged: true }, { uid: 2, seen: true, flagged: false }] });
  const result = S.results.get('a|INBOX').items.find(m => m.uid === 1);
  assert.equal(result.seen, false);
  assert.equal(result.flagged, true);
});
