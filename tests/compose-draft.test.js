const test = require('node:test');
const assert = require('node:assert/strict');
const drafts = require('../ui/compose-draft.js');

function fakeTimers() {
  let now = 0, seq = 0;
  const pending = new Map();
  return {
    setTimeout(fn, ms) { const id = ++seq; pending.set(id, { fn, at: now + ms }); return id; },
    clearTimeout(id) { pending.delete(id); },
    advance(ms) {
      now += ms;
      for (const [id, t] of [...pending].sort((a, b) => a[1].at - b[1].at)) {
        if (t.at <= now) { pending.delete(id); t.fn(); }
      }
    },
    get size() { return pending.size; },
  };
}
const tick = () => new Promise((resolve) => setImmediate(resolve));

test('autosave waits for the user to pause, then saves the latest snapshot once', async () => {
  const timers = fakeTimers();
  const saved = [];
  const saver = drafts.createAutosaver({
    delay: 30000, timers,
    save: async (snapshot, previous) => { saved.push({ snapshot, previous }); return { uid: saved.length }; },
  });
  saver.change({ body: 'a' });
  timers.advance(20000);
  saver.change({ body: 'ab' });
  timers.advance(20000);
  assert.equal(saved.length, 0, '仍在输入时不保存');
  timers.advance(10000);
  await tick();
  assert.deepEqual(saved, [{ snapshot: { body: 'ab' }, previous: null }]);
  assert.deepEqual(saver.ref, { uid: 1 });
  saver.change({ body: 'ab' });
  timers.advance(30000);
  await tick();
  assert.equal(saved.length, 1, '内容没变不重复保存');
});

test('saves never overlap and each new save replaces the previous version', async () => {
  const timers = fakeTimers();
  const calls = [];
  let release;
  const saver = drafts.createAutosaver({
    delay: 1000, timers,
    save: (snapshot, previous) => new Promise((resolve) => {
      calls.push({ snapshot, previous });
      release = () => resolve({ uid: calls.length });
    }),
  });
  saver.change({ body: 'v1' });
  timers.advance(1000);
  await tick();
  saver.change({ body: 'v2' });
  timers.advance(1000);
  await tick();
  assert.equal(calls.length, 1, '上一次保存还没完成时不能并发保存');
  release();
  await tick(); await tick();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1], { snapshot: { body: 'v2' }, previous: { uid: 1 } });
  release();
  await tick();
  assert.deepEqual(saver.ref, { uid: 2 });
});

test('flush saves immediately and a failed save stays dirty for the next attempt', async () => {
  const timers = fakeTimers();
  let fail = true;
  const errors = [];
  const saver = drafts.createAutosaver({
    delay: 30000, timers,
    save: async () => { if (fail) throw new Error('offline'); return { uid: 9 }; },
    onError: (e) => errors.push(String(e)),
  });
  saver.change({ body: 'x' });
  await assert.rejects(saver.flush());
  assert.equal(errors.length, 1);
  assert.equal(saver.dirty, true);
  fail = false;
  assert.deepEqual(await saver.flush(), { uid: 9 });
  assert.equal(saver.dirty, false);
  assert.equal(timers.size, 0, 'flush 取消了待执行的定时保存');
});

test('attachment list dedupes, sums sizes, and reports the size limit', () => {
  let list = drafts.addAttachments([], [{ token: 't1', name: 'a.pdf', size: 10 }, { token: 't1', name: 'a.pdf', size: 10 }]);
  list = drafts.addAttachments(list, [{ token: 't2', name: 'b.png', size: 20 }]);
  assert.deepEqual(list.map((a) => a.name), ['a.pdf', 'b.png']);
  assert.equal(drafts.totalSize(list), 30);
  assert.equal(drafts.limitMessage(list, 100), '');
  assert.match(drafts.limitMessage(list, 25), /超过/);
  list = drafts.removeAttachment(list, drafts.attachmentKey(list[0]));
  assert.deepEqual(list.map((a) => a.name), ['b.png']);
  assert.deepEqual(drafts.toRefs(list), [{ kind: 'file', token: 't2' }]);
});

test('forwarding carries the original attachments as message references', () => {
  const view = { attachments: [{ index: 0, name: '合同.pdf', size: 5 }, { index: 2, name: 'x.zip', size: 7 }] };
  const items = drafts.messageAttachments(view, { accountId: 'a', folder: 'INBOX', uid: 42, uidValidity: 9 });
  assert.deepEqual(drafts.toRefs(items), [
    { kind: 'message', receiveEpoch: 1, accountId: 'a', folder: 'INBOX', uid: 42, uidValidity: 9, index: 0 },
    { kind: 'message', receiveEpoch: 1, accountId: 'a', folder: 'INBOX', uid: 42, uidValidity: 9, index: 2 },
  ]);
  assert.equal(drafts.totalSize(items), 12);
});

test('a reopened draft restores recipients, body, attachments and its own location', () => {
  const person = (addr, name = '') => ({ addr, name });
  const view = {
    subject: '季度报价', text: '正文\n第二行',
    to: [person('z@example.com', '张三')], cc: [person('c@example.com')], bcc: [person('b@example.com')],
    attachments: [{ index: 0, name: 'a.pdf', size: 3 }], messageId: 'draft-1@example.com', uidValidity: 5,
  };
  const d = drafts.draftFromView(view, { accountId: 'acct', folder: 'Drafts', uid: 11 });
  assert.equal(d.to, '张三 <z@example.com>');
  assert.equal(d.cc, 'c@example.com');
  assert.equal(d.bcc, 'b@example.com');
  assert.equal(d.subject, '季度报价');
  assert.equal(d.body, '正文\n第二行');
  assert.deepEqual(d.draftRef, { accountId: 'acct', folder: 'Drafts', uid: 11, uidValidity: 5, messageId: '<draft-1@example.com>' });
  assert.deepEqual(drafts.toRefs(d.attachments), [{ kind: 'message', receiveEpoch: 1, accountId: 'acct', folder: 'Drafts', uid: 11, uidValidity: 5, index: 0 }]);
});

test('settle waits for an in-flight save without starting a new one', async () => {
  const timers = fakeTimers();
  let release, calls = 0;
  const saver = drafts.createAutosaver({
    delay: 1000, timers,
    save: () => new Promise((resolve) => { calls++; release = () => resolve({ uid: calls }); }),
  });
  saver.change({ body: 'v1' });
  timers.advance(1000);
  await tick();
  saver.change({ body: 'v2' });
  const settled = saver.settle();
  release();
  assert.deepEqual(await settled, { uid: 1 });
  timers.advance(5000);
  await tick();
  assert.equal(calls, 1, 'settle 之后不再保存，避免发送后又冒出新草稿');
});

test('a reopened draft can use extracted attachment tokens and keeps reply threading', () => {
  const view = { subject: 's', text: 't', to: [], cc: [], bcc: [], attachments: [{ index: 0, name: 'a.pdf', size: 3 }], messageId: 'd@x', inReplyTo: 'orig@x', references: ['root@x', 'orig@x'] };
  const d = drafts.draftFromView(view, { accountId: 'acct', folder: 'Drafts', uid: 1, uidValidity: 2 }, [{ token: 'mem', name: 'a.pdf', size: 3 }]);
  assert.deepEqual(drafts.toRefs(d.attachments), [{ kind: 'file', token: 'mem' }]);
  assert.equal(d.inReplyTo, '<orig@x>');
  assert.deepEqual(d.references, ['<root@x>', '<orig@x>']);
});
