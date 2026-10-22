const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const { createScrollMotion } = require('../ui/reader-motion.js');

function clock(reducedMotion = false) {
  let time = 0, nextId = 0;
  const frames = new Map(), cancelled = [];
  const motion = createScrollMotion({
    requestFrame(callback) { const id = ++nextId; frames.set(id, callback); return id; },
    cancelFrame(id) { cancelled.push(id); frames.delete(id); },
    now: () => time,
    reducedMotion,
  });
  return {
    motion, frames, cancelled,
    tick(at) {
      time = at;
      const queued = [...frames.values()];
      frames.clear();
      for (const callback of queued) callback(time);
    },
  };
}

function element(top = 0, height = 500, total = 2000) {
  let position = top;
  const writes = [], by = [];
  return {
    isConnected: true, clientHeight: height, scrollHeight: total, writes, by,
    get scrollTop() { return position; },
    set scrollTop(value) { writes.push(value); position = Math.max(0, Math.min(value, this.scrollHeight - this.clientHeight)); },
    scrollBy(options) { by.push(options); this.scrollTop += options.top; },
  };
}

test('scroll animates for 180ms with an actual ease-out cubic midpoint', () => {
  const t = clock(), el = element(10);
  assert.equal(t.motion.scroll(el, 100), true);
  assert.equal(t.motion.active, true);
  assert.equal(el.scrollTop, 10, 'starting does not jump to the destination');
  t.tick(90);
  assert.equal(el.scrollTop, 97.5, 'half-time covers 87.5 percent of the distance');
  assert.equal(t.motion.active, true);
  assert.equal(t.frames.size, 1);
  t.tick(180);
  assert.equal(el.scrollTop, 110);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0, 'completion does not queue more work');
});

test('a delayed animation frame finishes exactly at the target', () => {
  const t = clock(), el = element(12);
  t.motion.scroll(el, 123);
  t.tick(500);
  assert.equal(el.scrollTop, 135);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
});

test('rapid requests while active are rejected rather than queued', () => {
  const t = clock(), first = element(), other = element();
  assert.equal(t.motion.scroll(first, 100), true);
  assert.equal(t.motion.scroll(first, 100), false);
  assert.equal(t.motion.scroll(other, 200), false);
  assert.equal(t.frames.size, 1);
  t.tick(180);
  assert.equal(first.scrollTop, 100);
  assert.equal(other.scrollTop, 0);
  assert.equal(t.frames.size, 0);
  assert.equal(t.motion.scroll(other, 200), true, 'a new deliberate request may start after completion');
});

test('forward and backward scrolling clamp the destination before interpolation', () => {
  const down = clock(), downEl = element(950, 200, 1200);
  down.motion.scroll(downEl, 300);
  down.tick(90);
  assert.equal(downEl.scrollTop, 993.75);
  down.tick(180);
  assert.equal(downEl.scrollTop, 1000);
  assert.ok(downEl.writes.every(value => value <= 1000));

  const up = clock(), upEl = element(100);
  up.motion.scroll(upEl, -500);
  up.tick(90);
  assert.equal(upEl.scrollTop, 12.5);
  up.tick(180);
  assert.equal(upEl.scrollTop, 0);
  assert.ok(upEl.writes.every(value => value >= 0));
});

test('missing elements, zero room and invalid deltas do not start an animation', () => {
  const t = clock();
  for (const [el, delta] of [
    [null, 100], [undefined, 100], [element(0, 500, 400), 100],
    [element(1500), 100], [element(0), -100], [element(), 0],
    [element(), NaN], [element(), Infinity], [{ ...element(), isConnected: false }, 100],
  ]) assert.equal(t.motion.scroll(el, delta), false);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
});

test('cancel stops at the current position and ignores an already-dispatched stale frame', () => {
  const t = clock(), el = element();
  t.motion.scroll(el, 100);
  t.tick(90);
  const callback = [...t.frames.values()][0];
  const top = el.scrollTop;
  t.motion.cancel();
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
  assert.equal(t.cancelled.length, 1);
  callback(180);
  assert.equal(el.scrollTop, top);
  assert.equal(t.frames.size, 0);
});

test('an old callback cannot overwrite a newly started animation after cancellation', () => {
  const t = clock(), el = element();
  t.motion.scroll(el, 100);
  const stale = [...t.frames.values()][0];
  t.motion.cancel();
  assert.equal(t.motion.scroll(el, 200), true);
  stale(180);
  assert.equal(el.scrollTop, 0);
  assert.equal(t.motion.active, true);
  assert.equal(t.frames.size, 1);
  t.tick(90);
  assert.equal(el.scrollTop, 175);
  t.tick(180);
  assert.equal(el.scrollTop, 200);
});

test('completion prevents a duplicate old callback from writing again', () => {
  const t = clock(), el = element();
  t.motion.scroll(el, 100);
  const stale = [...t.frames.values()][0];
  t.tick(180);
  const count = el.writes.length;
  stale(190);
  assert.equal(el.writes.length, count);
  assert.equal(t.motion.active, false);
});

test('detaching the element during animation cancels future writes and frames', () => {
  const t = clock(), el = element();
  t.motion.scroll(el, 100);
  t.tick(60);
  const top = el.scrollTop;
  el.isConnected = false;
  t.tick(90);
  assert.equal(el.scrollTop, top);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
});

test('reduced motion uses immediate native scrolling and schedules no frames', () => {
  const t = clock(true), el = element(100);
  assert.equal(t.motion.scroll(el, 300), true);
  assert.equal(el.scrollTop, 400);
  assert.deepEqual(el.by, [{ top: 300, behavior: 'auto' }]);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
});

test('reduced-motion preference is read again for every request', () => {
  let reduce = false;
  const t = clock(() => reduce), el = element();
  t.motion.scroll(el, 100);
  assert.equal(t.motion.active, true);
  t.tick(180);
  reduce = true;
  t.motion.scroll(el, 100);
  assert.equal(el.scrollTop, 200);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
  reduce = false;
  t.motion.scroll(el, 100);
  assert.equal(t.motion.active, true);
  assert.equal(el.scrollTop, 200);
});

test('enabling reduced motion during animation finishes the remaining scroll at the next frame', () => {
  let reduce = false;
  const t = clock(() => reduce), el = element();
  t.motion.scroll(el, 100);
  t.tick(90);
  reduce = true;
  t.tick(100);
  assert.equal(el.scrollTop, 100);
  assert.deepEqual(el.by, [{ top: 12.5, behavior: 'auto' }]);
  assert.equal(t.motion.active, false);
  assert.equal(t.frames.size, 0);
});

test('a layout that shrinks while scrolling never receives an out-of-range position', () => {
  const t = clock(), el = element();
  t.motion.scroll(el, 1000);
  t.tick(40);
  el.scrollHeight = 800;
  el.writes.length = 0;
  t.tick(90);
  t.tick(180);
  assert.ok(el.writes.every(value => value >= 0 && value <= 300));
  assert.equal(el.scrollTop, 300);
  assert.equal(t.motion.active, false);
});

test('cancel is safe while idle and invalidates a finished frame before an immediate scroll', () => {
  let reduce = false;
  const t = clock(() => reduce), el = element();
  t.motion.cancel();
  t.motion.scroll(el, 100);
  const stale = [...t.frames.values()][0];
  t.tick(180);
  reduce = true;
  t.motion.scroll(el, 50);
  stale(190);
  assert.equal(el.scrollTop, 150);
  assert.equal(t.motion.active, false);
  t.motion.cancel();
  t.motion.cancel();
  assert.equal(t.frames.size, 0);
});

test('browser script exports the controller without CommonJS or DOM dependencies', () => {
  const context = vm.createContext({ window: {} });
  vm.runInContext(fs.readFileSync(require.resolve('../ui/reader-motion.js'), 'utf8'), context);
  assert.equal(typeof context.window.MailBeaconReaderMotion.createScrollMotion, 'function');
});
