const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

function app(invoke = async (cmd,args) => cmd==='execute_mail_operation' ? {items:args.request.items.map(source=>({source,status:'succeeded'}))} : undefined, {skipListLoads=false}={}) {
  const listeners = {};
  const overlay = { innerHTML: '', querySelectorAll: () => [] };
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: { MailBeaconSearch: require('../ui/mail-search.js'), MailBeaconSelection: require('../ui/mail-selection.js'), MailBeaconOperations: require('../ui/mail-operations.js'), MailBeaconViews: require('../ui/views.js'), MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'), MailBeaconRules: require('../ui/rules.js'), MailBeaconThreads: require('../ui/threads.js'), MailBeaconExtras: require('../ui/compose-extras.js'), __TAURI__: { core: { invoke }, event: { listen: async () => {} } } },
    document: {
      activeElement: null,
      querySelector: selector => selector === '#overlay' ? overlay : null,
      addEventListener: (name, handler) => { listeners[name] = handler; },
    },
    console,
    setTimeout,
    clearTimeout,
    listeners, skipListLoads,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    let renderCount = 0;
    renderList = () => { renderCount++; };
    render = renderReader = updateBadge = toast = showOperationOutcome = () => {};
    if (skipListLoads) loadList = async () => {};
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

test('sync events during a search do not replace results or drop the cached list', () => {
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
  // 首屏只有最新 100 封：已经从本地索引导入的更早邮件要保留，
  // 重连不能把用户已经看到的列表打回第一页
  const list = S.lists.get('a|INBOX');
  assert.equal(list.length, 201);
  assert.equal(list[0].uid, 301, '新首屏排在最前');
  assert.ok(list.some((m) => m.uid === 101), '更早的邮件还在');
});

test('a finished background summary index reloads the folder from the local list', async () => {
  const calls = [];
  const messages = Array.from({ length: 3 }, (_, i) => (
    { accountId: 'a', folder: 'Arch', uid: i + 1, fromName: '', fromAddr: '', to: '', subject: 's', date: i + 1, seen: true }));
  const search = app(async (cmd, input) => {
    calls.push({ cmd, input });
    if (cmd === 'list_messages') return { items: messages, complete: true };
    return [];
  });
  search.S.accounts = [{ id: 'a', email: 'me@example.com' }];
  search.S.acct = 'a';
  search.S.folder = 'Arch';
  // 别的账号的索引补完不该动当前列表
  search.onMailEvent({ type: 'listIndex', accountId: 'b', folder: 'Arch' });
  assert.equal(calls.length, 0);
  // 当前账号、当前文件夹：重新读一遍，换成一次显示全部
  search.onMailEvent({ type: 'listIndex', accountId: 'a', folder: 'Arch' });
  await sleep(5);
  assert.equal(calls.filter((c) => c.cmd === 'list_messages').length, 1);
  assert.equal(search.S.lists.get('a|Arch')?.length, 3);
  assert.ok(search.S.exhausted.has('a|Arch'), '整个文件夹都在本地，不再显示加载更早');
});

test('mark-all-read and flag sync update both search results and the folder list', async () => {
  const { S, onMailEvent, markAllRead } = app(undefined,{skipListLoads:true});
  S.accounts = [{ id: 'a', email: 'me@example.com' }];
  S.acct = 'a';
  S.folder = 'INBOX';
  const msg = (uid) => ({ accountId: 'a', folder: 'INBOX', uid, receiveEpoch: 1, uidValidity: 7, fromName: '', fromAddr: '', to: '', subject: 's', date: uid, seen: false, flagged: false });
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

const msg = (uid, extra={}) => ({accountId:'a',folder:'INBOX',uid,receiveEpoch:1,uidValidity:7,date:uid,subject:'match',seen:true,...extra});
const page = (items=[],extra={}) => ({items,coverage:[{accountId:'a',folder:'INBOX',receiveEpoch:1,uidValidity:7,indexed:20,total:30,full:4,prefix:12,skipped:2,snapshotAt:Date.now()}],catalogIncomplete:[],hasMore:false,nextCursor:null,revision:1,...extra});
function configure(t) { Object.assign(t.S,{accounts:[{id:'a',receiveEpoch:1,email:'a@example.test'}],acct:'a',folder:'INBOX',query:'needle',searching:true}); }
test('scoped search returns body-only matches and always exposes incomplete coverage', async()=>{
 const calls=[]; const t=app(async(cmd,input)=>{if(cmd==='search_mail'){calls.push(input);return page([msg(9,{subject:'no keyword in header'})]);}});configure(t);t.S.searchScope='all';t.S.searchFilters={attachmentOnly:true,sender:'amy'};
 t.requestServerSearch();await sleep(230);
 assert.equal(calls[0].input.scope,'all');assert.equal(calls[0].input.attachmentOnly,true);assert.equal(calls[0].input.sender,'amy');assert.deepEqual(t.currentList().map(m=>m.uid),[9]);assert.match(t.searchNotice(),/摘要 20\/30/);assert.match(t.searchNotice(),/未覆盖/);assert.match(t.searchNotice(),/完整正文 4/);
});
test('one failed or unsynchronized account stays visible beside successful local results',async()=>{
 const t=app(async()=>page([msg(9)],{catalogIncomplete:['b@example.test：文件夹未同步'],coverage:[{accountId:'a',folder:'INBOX',indexed:1,total:1,full:1,prefix:0,skipped:0,error:null},{accountId:'b',folder:'INBOX',indexed:0,total:null,full:0,error:'连接失败'}]}));configure(t);t.S.acct='all';t.requestServerSearch();await sleep(230);
 assert.deepEqual(t.currentList().map(m=>m.uid),[9]);assert.match(t.searchNotice(),/b@example.test/);assert.match(t.searchNotice(),/连接失败/);
});
test('search pages carry cursor and invalidated cursor keeps results with an explicit refresh action',async()=>{
 const calls=[];const t=app(async(cmd,args)=>{if(cmd!=='search_mail')return;calls.push(args.input);if(calls.length===1)return page([msg(9)],{hasMore:true,nextCursor:'cursor1'});throw '搜索结果已有变化，请刷新后继续浏览';});configure(t);t.requestServerSearch();await sleep(230);t.requestServerSearch({more:true});await sleep(230);
 assert.equal(calls[1].cursor,'cursor1');assert.deepEqual(t.currentList().map(m=>m.uid),[9]);assert.match(t.searchNotice(),/刷新搜索/);
});
test('late results cannot cross a receiving endpoint change',async()=>{
 let finish;const t=app(async()=>new Promise(r=>finish=r));configure(t);t.requestServerSearch();await sleep(200);t.S.accounts[0].receiveEpoch=2;finish(page([msg(9)]));await sleep(5);assert.equal(t.S.results.get('__search')?.items.length,0);
});
test('index growth in another folder refreshes an all-folder search exactly once',async()=>{
 let calls=0;const t=app(async(cmd)=>cmd==='search_mail'?(calls++,page(calls===1?[]:[msg(7)])):undefined);configure(t);t.S.searchScope='account';t.requestServerSearch();await sleep(230);
 const event={type:'searchIndex',accountId:'a',folder:'Sent',indexing:{running:true,complete:false,indexed:80,total:100,bodyIndexed:4}};t.onMailEvent(event);await sleep(380);assert.equal(calls,2);assert.deepEqual(t.currentList().map(m=>m.uid),[7]);t.onMailEvent(event);await sleep(380);assert.equal(calls,2);
});
test('clearing the query clears coverage and previously paged results',async()=>{
 const t=app(async()=>page([msg(9)]));configure(t);t.requestServerSearch();await sleep(230);t.S.query='';t.requestServerSearch();assert.equal(t.S.results.size,0);assert.equal(t.S.searchPage,null);assert.equal(t.searchNotice(),'');
});

test('a failed changed scope never presents the previous scope results',async()=>{
 let calls=0;const t=app(async()=>{if(++calls===1)return page([msg(9)]);throw '离线';});configure(t);t.requestServerSearch();await sleep(230);
 assert.deepEqual(t.currentList().map(m=>m.uid),[9]);t.S.searchScope='all';t.requestServerSearch();await sleep(230);
 assert.deepEqual(t.currentList(),[]);assert.match(t.searchNotice(),/离线/);
});
