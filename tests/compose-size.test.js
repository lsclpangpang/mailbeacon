const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const root = path.join(__dirname, '..');
const css = fs.readFileSync(path.join(root, 'ui/style.css'), 'utf8');
const app = fs.readFileSync(path.join(root, 'ui/app.js'), 'utf8');

function blockFor(selector, source) {
  // Match only a rule whose selector list is exactly `selector` (not descendants like `.compose-sheet .compose-body`).
  const re = new RegExp(`(^|\\n)\\s*(${selector.replace('.', '\\.')})\\s*\\{([^}]*)\\}`, 'm');
  const m = source.match(re);
  assert.ok(m, `missing CSS rule for ${selector}`);
  return { selectors: m[2].trim(), body: m[3] };
}

test('compose sheet is wider than the shared sheet default (960px)', () => {
  const compose = blockFor('.compose-sheet', css);
  assert.match(compose.body, /width\s*:\s*min\(\s*960px\s*,\s*calc\(100vw\s*-\s*48px\)\s*\)/,
    'compose-sheet must use width:min(960px, calc(100vw - 48px))');
});

test('compose sheet is taller (820px capped by viewport)', () => {
  const compose = blockFor('.compose-sheet', css);
  const hasHeight = /height\s*:\s*min\(\s*820px\s*,\s*calc\(100vh\s*-\s*80px\)\s*\)/.test(compose.body);
  const hasMax = /max-height\s*:\s*min\(\s*820px\s*,\s*calc\(100vh\s*-\s*80px\)\s*\)/.test(compose.body)
    || /max-height\s*:\s*calc\(100vh\s*-\s*80px\)/.test(compose.body);
  assert.ok(hasHeight || (hasMax && /820px/.test(compose.body)),
    'compose-sheet must target height min(820px, calc(100vh - 80px))');
});

test('shared .sheet width stays 680px so other dialogs are unchanged', () => {
  // Prefer the standalone ".sheet {" rule, not ".compose-sheet".
  const m = css.match(/(^|\n)\.sheet\s*\{([^}]*)\}/m);
  assert.ok(m, 'missing .sheet rule');
  assert.match(m[2], /width\s*:\s*min\(\s*680px\s*,\s*calc\(100vw\s*-\s*48px\)\s*\)/);
  assert.doesNotMatch(m[2], /960px/);
});

test('openCompose no longer hardcodes 640px height inline', () => {
  const open = app.slice(app.indexOf('function openCompose'), app.indexOf('function openCompose') + 2500);
  assert.ok(open.includes('compose-sheet'), 'openCompose should still mount compose-sheet');
  assert.doesNotMatch(open, /height\s*:\s*min\(\s*640px/, 'remove inline height:min(640px, ...) from openCompose');
  assert.doesNotMatch(open, /640px/, 'openCompose template must not hardcode 640px');
});
