const test = require('node:test');
const assert = require('node:assert/strict');
const r = require('../ui/recipients.js');

test('addresses are formatted with quotes only when the name needs them', () => {
  assert.equal(r.formatAddress('张三', 'z@example.com'), '张三 <z@example.com>');
  assert.equal(r.formatAddress('', 'z@example.com'), 'z@example.com');
  assert.equal(r.formatAddress('Zhang, San', 'z@example.com'), '"Zhang, San" <z@example.com>');
  assert.equal(r.formatAddress('a "b"', 'z@example.com'), '"a \\"b\\"" <z@example.com>');
});

test('the token under the caret respects quotes and Chinese separators', () => {
  const value = '"Zhang, San" <z@example.com>， li';
  assert.deepEqual(r.currentToken(value, value.length), { start: value.indexOf('li') - 1, end: value.length, text: 'li' });
  assert.equal(r.currentToken(value, 5).text, '"Zhang, San" <z@example.com>');
  assert.equal(r.currentToken('', 0).text, '');
});

test('applying a suggestion replaces only the current token and adds a separator', () => {
  const value = 'a@example.com, zh';
  const next = r.applySuggestion(value, value.length, '张三 <zhang@example.com>');
  assert.equal(next.value, 'a@example.com, 张三 <zhang@example.com>, ');
  assert.equal(next.caret, next.value.length);
  const middle = r.applySuggestion('zh, b@example.com', 2, '张三 <zhang@example.com>');
  assert.equal(middle.value, '张三 <zhang@example.com>, b@example.com');
});

test('group suggestions expand to all members and duplicates are skipped', () => {
  assert.equal(r.suggestionText({ kind: 'group', members: ['A <a@x.com>', 'b@x.com'] }), 'A <a@x.com>, b@x.com');
  assert.equal(r.suggestionText({ kind: 'contact', name: '张三', email: 'z@x.com' }), '张三 <z@x.com>');
  const added = r.addRecipients('A <a@x.com>, ', ['a@X.com', '李四 <l@x.com>', 'l@x.com']);
  assert.equal(added, 'A <a@x.com>, 李四 <l@x.com>');
  assert.equal(r.addRecipients('', ['c@x.com']), 'c@x.com');
  assert.equal(r.emailOf('"A, B" <ab@x.com>'), 'ab@x.com');
  assert.equal(r.emailOf('plain@x.com'), 'plain@x.com');
});

test('dedupe removes repeated addresses but keeps a trailing separator for typing', () => {
  assert.equal(r.dedupe('A <a@x.com>, b@x.com, a@X.com, '), 'A <a@x.com>, b@x.com, ');
  assert.equal(r.dedupe('a@x.com, a@x.com'), 'a@x.com');
});
