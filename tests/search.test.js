const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke = async () => {}) {
  const listeners = {};
  const overlay = { innerHTML: '', querySelectorAll: () => [] };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
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
    globalThis.searchApi = { S, currentList, onMailEvent, markAllRead, listeners, requestServerSearch, searchNotice, get renderCount() { return renderCount; } };
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

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

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

// 服务器拒绝搜索时，列表里留着的只是已加载邮件的本地匹配；必须明确告诉用户，
// 否则看起来就像“只能搜到最近的邮件”。
test('a failed server search keeps a visible notice instead of silently showing loaded mail', async () => {
  const search = app(async (cmd, input) => {
    if (cmd !== 'search_messages') return;
    if (input.accountId === 'bad') throw 'NO [BADCHARSET] unsupported';
    return { items: [{ accountId: 'good', folder: 'INBOX', uid: 9, subject: '2025 报价单', date: 1, seen: true }], indexing: null };
  });
  const { S } = search;
  S.accounts = [{ id: 'good', email: 'g@example.com' }, { id: 'bad', email: 'b@163.com' }];
  S.acct = 'all';
  S.folder = 'INBOX';
  S.lists.set('bad|INBOX', [{ accountId: 'bad', folder: 'INBOX', uid: 1, subject: '最近的报价单', date: 2, seen: true }]);
  S.query = '报价单';
  search.requestServerSearch();
  await new Promise(resolve => setTimeout(resolve, 250));

  assert.deepEqual(search.currentList().map(m => `${m.accountId}:${m.uid}`), ['bad:1', 'good:9'], '成功的账号照常显示服务器结果');
  const notice = search.searchNotice();
  assert.match(notice, /b@163\.com/);
  assert.match(notice, /BADCHARSET/);
  assert.match(notice, /已加载/);

  // 之前成功过的同一关键词结果不能留着，否则提示说“只列出已加载邮件”就不对了
  S.results.set('bad|INBOX', { query: '报价单', items: [{ accountId: 'bad', folder: 'INBOX', uid: 7, date: 3 }] });
  search.requestServerSearch();
  await new Promise(resolve => setTimeout(resolve, 250));
  assert.equal(S.results.has('bad|INBOX'), false);

  // 切到聚合视图搜同一个词时，不能留着收件箱那次的失败提示
  S.folder = '__unread';
  search.requestServerSearch();
  assert.equal(search.searchNotice(), '');

  S.indexing.set('good|INBOX', { query: '报价单', running: true, complete: false, indexed: 1, total: 2, error: null });
  S.folder = 'INBOX';
  S.query = '';
  search.requestServerSearch();
  assert.equal(search.searchNotice(), '', '退出搜索后提示消失');
  assert.equal(S.indexing.size, 0, '退出搜索后清掉索引进度');
});

const msg = (accountId, uid, extra = {}) => ({
  accountId, folder: 'INBOX', uid, fromName: '', fromAddr: '', to: '', subject: 's', date: uid, seen: true, ...extra,
});

const INDEX_RUNNING = { running: true, complete: false, indexed: 120, total: 2712, error: null };

test('an in-progress index is announced as progress, not as an error', async () => {
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    return { items: [msg('a', 5, { subject: '发票 2025' })], indexing: INDEX_RUNNING };
  });
  const { S } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '发票';
  search.requestServerSearch();
  await sleep(300);

  assert.deepEqual(search.currentList().map(m => m.uid), [5], '索引中的部分结果先显示出来');
  assert.deepEqual(
    { ...S.indexing.get('a|INBOX') },
    { query: '发票', ...INDEX_RUNNING },
    '进度跟着关键词存进 S.indexing',
  );
  const notice = search.searchNotice();
  assert.match(notice, /正在建立本地搜索索引/);
  assert.match(notice, /me@example\.com：120\/2712 封/);
  assert.match(notice, /完成后会显示全部匹配/);
  assert.doesNotMatch(notice, /失败/);
  assert.doesNotMatch(notice, /搜索失败/);
});

test('body indexing progress is shown alongside the summary progress', async () => {
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    return {
      items: [],
      indexing: { running: true, complete: false, indexed: 2712, total: 2712, bodyIndexed: 480, bodyTotal: 2712, error: null },
    };
  });
  const { S } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '合同';
  search.requestServerSearch();
  await sleep(300);

  const notice = search.searchNotice();
  assert.match(notice, /正在建立本地搜索索引/);
  assert.match(notice, /me@example\.com：摘要 2712\/2712 封，正文 480\/2712 封/);
});

test('an index that has not counted mail yet says it is checking for new mail', async () => {
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    return { items: [], indexing: { running: true, complete: false, indexed: 0, total: 0, error: null } };
  });
  const { S } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '周报';
  search.requestServerSearch();
  await sleep(300);

  const notice = search.searchNotice();
  assert.match(notice, /正在建立本地搜索索引/);
  assert.match(notice, /me@example\.com：正在检查新邮件…/);
  assert.doesNotMatch(notice, /0\/0/);
});

test('a completed index event triggers one more search and shows the final matches', async () => {
  const calls = [];
  const search = app(async (cmd, input) => {
    if (cmd !== 'search_messages') return;
    calls.push(input);
    if (calls.length === 1) {
      return { items: [], indexing: { running: true, complete: false, indexed: 1, total: 2, error: null } };
    }
    return { items: [msg('a', 9, { subject: '合同 终稿' })], indexing: { running: false, complete: true, indexed: 2, total: 2, error: null } };
  });
  const { S, onMailEvent, currentList } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '合同';
  S.searching = true;
  S.lists.set('a|INBOX', [msg('a', 1, { subject: '更早的合同' })]);
  search.requestServerSearch();
  await sleep(300);
  assert.equal(calls.length, 1);
  assert.deepEqual(currentList().map(m => m.uid), [], '索引结果到达前不能拿已加载邮件冒充');

  onMailEvent({
    type: 'searchIndex', accountId: 'a', folder: 'INBOX',
    indexing: { running: false, complete: true, indexed: 2, total: 2, error: null },
  });
  await sleep(500);
  assert.equal(calls.length, 2, '索引完成后再查一次本地库');
  assert.equal(calls[1].query, '合同');
  assert.deepEqual(currentList().map(m => m.uid), [9], '显示最终的搜索结果');
  assert.equal(search.searchNotice(), '', '完成后不再显示进度');

  // 同一个完成事件反复到达时不能无限重新搜索
  onMailEvent({
    type: 'searchIndex', accountId: 'a', folder: 'INBOX',
    indexing: { running: false, complete: true, indexed: 2, total: 2, error: null },
  });
  await sleep(400);
  assert.equal(calls.length, 2, '相同状态的事件不触发额外搜索');
});

test('growing index progress refreshes the visible matches while searching', async () => {
  let call = 0;
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    call += 1;
    if (call === 1) {
      return { items: [], indexing: { running: true, complete: false, indexed: 40, total: 100, bodyIndexed: 0, bodyTotal: 40, error: null } };
    }
    return {
      items: [msg('a', 7, { subject: '报价单 旧邮件' })],
      indexing: { running: true, complete: false, indexed: 80, total: 100, bodyIndexed: 0, bodyTotal: 80, error: null },
    };
  });
  const { S, onMailEvent, currentList } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '报价单';
  S.searching = true;
  search.requestServerSearch();
  await sleep(300);
  assert.deepEqual(currentList().map(m => m.uid), []);

  onMailEvent({
    type: 'searchIndex', accountId: 'a', folder: 'INBOX',
    indexing: { running: true, complete: false, indexed: 80, total: 100, bodyIndexed: 0, bodyTotal: 80, error: null },
  });
  await sleep(400);
  assert.equal(call, 2, '索引进度变化后要重新查一次本地库');
  assert.deepEqual(currentList().map(m => m.uid), [7], '刚索引到的旧邮件要出现在结果里');

  // 进度没变化的事件不要再触发请求
  onMailEvent({
    type: 'searchIndex', accountId: 'a', folder: 'INBOX',
    indexing: { running: true, complete: false, indexed: 80, total: 100, bodyIndexed: 0, bodyTotal: 80, error: null },
  });
  await sleep(300);
  assert.equal(call, 2);
});

test('an index error is shown as a failure notice and partial results stay visible', async () => {
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    return {
      items: [msg('a', 3, { subject: '发票' })],
      indexing: { running: false, complete: false, indexed: 40, total: 2712, error: '连接中断' },
    };
  });
  const { S } = search;
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  S.query = '发票';
  search.requestServerSearch();
  await sleep(300);

  assert.deepEqual(search.currentList().map(m => m.uid), [3]);
  const notice = search.searchNotice();
  assert.match(notice, /本地索引建立失败/);
  assert.match(notice, /me@example\.com：连接中断/);
  assert.match(notice, /下面只列出已索引邮件中的匹配/);
  assert.doesNotMatch(notice, /正在建立本地搜索索引/);
  assert.doesNotMatch(notice, /服务器搜索失败/);
});

test('a null indexing payload (POP3) behaves like a plain result and clears stale progress', async () => {
  const search = app(async (cmd) => {
    if (cmd !== 'search_messages') return;
    return { items: [msg('p', 11, { subject: '报价单' })], indexing: null };
  });
  const { S } = search;
  S.accounts = [{ id: 'p', email: 'p@163.com', receiveProtocol: 'pop3' }];
  S.acct = 'p';
  S.folder = 'INBOX';
  S.query = '报价单';
  S.indexing.set('p|INBOX', { query: '报价单', running: true, complete: false, indexed: 1, total: 2, error: null });
  search.requestServerSearch();
  await sleep(300);

  assert.deepEqual(search.currentList().map(m => m.uid), [11]);
  assert.equal(S.indexing.has('p|INBOX'), false, 'POP3 没有索引进度');
  assert.equal(search.searchNotice(), '');
});
