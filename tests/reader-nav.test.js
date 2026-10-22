const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const N = require('../ui/reader-nav.js');
const T = require('../ui/threads.js');

const entry = (uid, unread = false, extra = {}) => ({ target: { accountId: 'a', folder: 'INBOX', uid, seen: !unread, ...extra }, unread });
const selection = (uid, extra = {}) => ({ accountId: 'a', folder: 'INBOX', uid, ...extra });
const metrics = (scrollTop, clientHeight, scrollHeight) => ({ scrollTop, clientHeight, scrollHeight });

test('next target prefers the first unread row after the current row', () => {
  const entries = [entry(5, true), entry(4, true), entry(3), entry(2, true), entry(1, true)];
  assert.deepEqual(N.nextMailTarget(entries, 1), selection(2));
  assert.deepEqual(N.nextMailTarget(entries, 1, { preferUnread: false }), selection(3));
});

test('next target falls back to the adjacent row without looking backward', () => {
  const entries = [entry(5, true), entry(4), entry(3), entry(2)];
  assert.deepEqual(N.nextMailTarget(entries, 1), selection(3));
  assert.equal(N.nextMailTarget(entries, 3), null);
});

test('empty, missing and invalid current positions never wrap to the first row', () => {
  const entries = [entry(2, true), entry(1, true)];
  for (const index of [-1, -2, 2, 10, 0.5, NaN, undefined]) {
    assert.equal(N.nextMailTarget(entries, index), null, String(index));
  }
  assert.equal(N.nextMailTarget([], 0), null);
});

test('the target is a fresh selection with account and folder identity only', () => {
  const target = Object.freeze({ accountId: 'b', folder: 'Sent|2026', uid: 9, seen: true, subject: 'subject' });
  const entries = Object.freeze([Object.freeze(entry(10)), Object.freeze({ target, unread: true })]);
  const result = N.nextMailTarget(entries, 0);
  assert.deepEqual(result, selection(9, { accountId: 'b', folder: 'Sent|2026' }));
  assert.notEqual(result, target);
  result.uid = 0;
  assert.equal(target.uid, 9);
});

test('thread navigation uses row unread state even when its latest message is read', () => {
  const messages = [
    { ...entry(10).target, date: 10, messageId: 'current' },
    { ...entry(9).target, date: 9, messageId: 'adjacent' },
    { ...entry(8).target, date: 8, messageId: 'reply', references: ['root'] },
    { ...entry(7, true).target, date: 7, messageId: 'root' },
  ];
  const entries = T.groupThreads(messages, () => false).map((g) => ({ target: g.latest, unread: g.unread > 0 }));
  assert.equal(entries[2].target.seen, true);
  assert.equal(entries[2].unread, true);
  assert.deepEqual(N.nextMailTarget(entries, 0), selection(8));
});

test('a resolved row index remains usable when the selected thread member is not its representative', () => {
  const entries = [entry(9), entry(8), entry(7)];
  // The caller resolved row 0 from an older or Sent message in that conversation.
  assert.deepEqual(N.nextMailTarget(entries, 0), selection(8));
  entries[0] = entry(99);
  assert.deepEqual(N.nextMailTarget(entries, 0), selection(8));
});

test('scroll-first treats one remaining pixel as the bottom', () => {
  assert.equal(N.shouldSpaceScrollFirst(metrics(0, 600, 400)), false);
  assert.equal(N.shouldSpaceScrollFirst(metrics(399, 600, 1000)), false);
  assert.equal(N.shouldSpaceScrollFirst(metrics(398.9, 600, 1000)), true);
  assert.equal(N.shouldSpaceScrollFirst(metrics(400, 600, 1000)), false);
  assert.equal(N.shouldSpaceScrollFirst(metrics(420, 600, 1000)), false);
});

test('hint labels omit unavailable archive while preserving the other shortcuts', () => {
  assert.deepEqual(N.readerHintLabels({ canArchive: true, canReply: true }), [
    { key: 'Space', label: '滚动 / 下一封', disabled: false },
    { key: 'E', label: '归档', disabled: false },
    { key: 'S', label: '星标', disabled: false },
    { key: 'R', label: '回复', disabled: false },
  ]);
  assert.deepEqual(N.readerHintLabels({ canArchive: false, canReply: false }), [
    { key: 'Space', label: '滚动 / 下一封', disabled: false },
    { key: 'S', label: '星标', disabled: false },
    { key: 'R', label: '暂不可回复', disabled: true },
  ]);
  const mixed = N.readerHintLabels({ canArchive: false, canReply: true });
  assert.deepEqual(mixed.map(item => item.key), ['Space', 'S', 'R']);
  assert.equal(mixed[2].disabled, false);
});

test('plain mail scrolls by 90 percent of the reader viewport and clamps the last step', () => {
  assert.deepEqual(N.spaceScrollPlan({ outer: metrics(0, 600, 1500) }), { target: 'outer', top: 540 });
  assert.deepEqual(N.spaceScrollPlan({ outer: metrics(800, 600, 1500) }), { target: 'outer', top: 100 });
  assert.equal(N.spaceScrollPlan({ outer: metrics(899, 600, 1500) }), null);
  assert.equal(N.spaceScrollPlan({ outer: metrics(0, 600, 500) }), null);
});

test('an iframe without its own overflow uses only the outer reader', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(0, 600, 1500), inner: metrics(0, 800, 800), frameBottom: 800, viewportBottom: 600,
  }), { target: 'outer', top: 540 });
});

test('before an overflowing iframe bottom is visible the outer step stops at that edge', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7000, 600, 8400), inner: metrics(0, 8000, 16000), frameBottom: 900, viewportBottom: 600,
  }), { target: 'outer', top: 300 });
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(0, 600, 8400), inner: metrics(0, 8000, 16000), frameBottom: 8100, viewportBottom: 600,
  }), { target: 'outer', top: 540 });
});

test('inner scroll steps use the reader viewport rather than the eight-thousand-pixel iframe', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7500, 600, 8400), inner: metrics(0, 8000, 16000),
    frameTop: -7400, frameBottom: 600, viewportTop: 0, viewportBottom: 600,
  }), { target: 'inner', top: 540 });
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7500, 600, 8400), inner: metrics(7900, 8000, 16000),
    frameTop: -7400, frameBottom: 600, viewportTop: 0, viewportBottom: 600,
  }), { target: 'inner', top: 100 });
});

test('a partly visible iframe scrolls by its visible body height without skipping text', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7800, 600, 8400), inner: metrics(0, 8000, 16000),
    frameTop: -7700, frameBottom: 300, viewportTop: 0, viewportBottom: 600,
  }), { target: 'inner', top: 270 });
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7800, 600, 8400), inner: metrics(0, 8000, 16000),
    frameTop: -7648, frameBottom: 352, viewportTop: 52, viewportBottom: 652,
  }), { target: 'inner', top: 270 }, 'viewport coordinates include the toolbar offset');
});

test('an entirely visible short iframe uses its own visible height for inner paging', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(0, 600, 600), inner: metrics(0, 120, 1200),
    frameTop: 200, frameBottom: 320, viewportTop: 52, viewportBottom: 652,
  }), { target: 'inner', top: 108 });
});

test('an overflowing iframe above the viewport is revealed before any inner scrolling', () => {
  const outer = metrics(8500, 600, 9100);
  const inner = metrics(0, 8000, 16000);
  let frameTop = -8100, frameBottom = -100;
  const plan = N.spaceScrollPlan({ outer, inner, frameTop, frameBottom, viewportTop: 52, viewportBottom: 652 });
  assert.deepEqual(plan, { target: 'outer', top: -752 }, 'restore the iframe bottom to the viewport bottom');
  outer.scrollTop += plan.top;
  frameTop -= plan.top;
  frameBottom -= plan.top;
  assert.equal(inner.scrollTop, 0, 'invisible body content was not consumed');
  assert.deepEqual(N.spaceScrollPlan({ outer, inner, frameTop, frameBottom, viewportTop: 52, viewportBottom: 652 }),
    { target: 'inner', top: 540 }, 'the following press can page the now-visible inner body');
});

test('revealing an offscreen iframe never scrolls above the outer start', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(200, 600, 1500), inner: metrics(0, 8000, 16000),
    frameTop: -8100, frameBottom: -100, viewportTop: 52, viewportBottom: 652,
  }), { target: 'outer', top: -200 });
});

test('callers without full geometry retain the reader-height fallback', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7800, 600, 8400), inner: metrics(0, 8000, 16000), frameBottom: 300, viewportBottom: 600,
  }), { target: 'inner', top: 540 });
});

test('after the inner body ends scrolling continues through the outer reader footer', () => {
  assert.deepEqual(N.spaceScrollPlan({
    outer: metrics(7500, 600, 8400), inner: metrics(8000, 8000, 16000), frameBottom: 600, viewportBottom: 600,
  }), { target: 'outer', top: 300 });
  assert.equal(N.spaceScrollPlan({
    outer: metrics(7800, 600, 8400), inner: metrics(8000, 8000, 16000), frameBottom: 300, viewportBottom: 600,
  }), null);
});

test('both scroll layers apply the one-pixel tolerance and minimum page increment', () => {
  assert.equal(N.spaceScrollPlan({
    outer: metrics(99, 600, 700), inner: metrics(7999, 8000, 16000), frameBottom: 600, viewportBottom: 600,
  }), null);
  assert.deepEqual(N.spaceScrollPlan({ outer: metrics(0, 1, 20) }), { target: 'outer', top: 1 });
});

test('repeated Space plans traverse a very long nested body before the footer', () => {
  const outer = metrics(0, 600, 8500);
  const inner = metrics(0, 8000, 24000);
  let steps = 0;
  let innerStarted = false;
  for (;;) {
    const frameBottom = 8200 - outer.scrollTop;
    const plan = N.spaceScrollPlan({ outer, inner, frameTop: frameBottom - 8000, frameBottom, viewportTop: 0, viewportBottom: 600 });
    if (!plan) break;
    assert.ok(++steps < 100, 'navigation must finish');
    assert.ok(plan.top > 0 && plan.top <= 540, 'no step skips more than one reading viewport');
    if (plan.target === 'inner') {
      innerStarted = true;
      assert.ok(frameBottom <= 601, 'outer reader first reveals the iframe bottom');
      inner.scrollTop += plan.top;
    } else {
      if (innerStarted) assert.ok(inner.scrollHeight - inner.clientHeight - inner.scrollTop <= 1, 'footer comes after all body content');
      else assert.ok(outer.scrollTop + plan.top <= 7600, 'outer scrolling must not skip across the iframe boundary');
      outer.scrollTop += plan.top;
    }
  }
  assert.ok(innerStarted);
  assert.equal(inner.scrollTop, 16000);
  assert.equal(outer.scrollTop, 7900);
});

test('browser script exposes the same public helpers without CommonJS', () => {
  const context = vm.createContext({ window: {} });
  vm.runInContext(fs.readFileSync(require.resolve('../ui/reader-nav.js'), 'utf8'), context);
  assert.deepEqual(Object.keys(context.window.MailBeaconReaderNav).sort(), Object.keys(N).sort());
});
