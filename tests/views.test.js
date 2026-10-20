const test = require('node:test');
const assert = require('node:assert/strict');
const views = require('../ui/views.js');

const folders = [
  { path: 'INBOX', name: '收件箱', role: 'inbox' },
  { path: 'Drafts', name: '草稿箱', role: 'drafts' },
  { path: '&XfJT0ZAB-', name: '已发送', role: 'sent' },
  { path: '__local_drafts', name: '本地草稿', role: 'local-drafts' },
];

test('virtual views resolve to each account\'s real folders', () => {
  assert.equal(views.isVirtual('__unread'), true);
  assert.equal(views.isVirtual('__local_drafts'), false, '本地草稿是真实（本地）文件夹');
  assert.equal(views.isVirtual('INBOX'), false);
  assert.deepEqual(views.sources('__unread', folders), [{ folder: 'INBOX', unreadOnly: true }]);
  assert.deepEqual(views.sources('__starred', folders), [{ folder: 'INBOX', flaggedOnly: true }]);
  assert.deepEqual(views.sources('__drafts', folders), [{ folder: 'Drafts' }, { folder: '__local_drafts' }]);
  assert.deepEqual(views.sources('__sent', folders), [{ folder: '&XfJT0ZAB-' }]);
  assert.deepEqual(views.sources('__sent', [{ path: 'INBOX', role: 'inbox' }]), [], 'POP3 账号没有已发送');
  assert.equal(views.label('__drafts'), '所有草稿');
});

test('filters: unread, flagged, attachment and mail addressed to me', () => {
  const mine = (accountId) => accountId === 'a' ? ['me@x.com'] : ['other@y.com'];
  const items = [
    { accountId: 'a', uid: 1, seen: false, flagged: false, attachment: false, recipients: ['me@x.com'] },
    { accountId: 'a', uid: 2, seen: true, flagged: true, attachment: true, recipients: ['list@x.com'] },
    { accountId: 'b', uid: 3, seen: true, flagged: false, attachment: false, recipients: ['OTHER@y.com', 'z@y.com'] },
  ];
  const uids = (filter) => views.filterMessages(items, filter, mine).map((m) => m.uid);
  assert.deepEqual(uids('all'), [1, 2, 3]);
  assert.deepEqual(uids('unread'), [1]);
  assert.deepEqual(uids('flagged'), [2]);
  assert.deepEqual(uids('attachment'), [2]);
  assert.deepEqual(uids('mentions'), [1, 3], '收件人或抄送里有自己（不区分大小写）');
  assert.deepEqual(views.FILTERS.map((f) => f.key), ['all', 'unread', 'flagged', 'attachment', 'mentions']);
});

test('the open message stays visible even if it no longer matches the filter', () => {
  const items = [{ accountId: 'a', uid: 1, seen: true, flagged: false, attachment: false, recipients: [] }];
  assert.deepEqual(views.filterMessages(items, 'unread', () => [], (m) => m.uid === 1).map((m) => m.uid), [1]);
});
