const test = require('node:test');
const assert = require('node:assert/strict');
const { createSignatureDraft, editSignatureDraft, switchSignature } = require('../ui/compose-signature.js');

test('signature appears before a reply or forwarded original', () => {
  const quote = '\n\n> 原邮件正文';
  const draft = createSignatureDraft(quote, '李嘉文\n产品部');
  assert.equal(draft.text, '\n\n李嘉文\n产品部' + quote);
});

test('switching sender replaces only the automatic signature and preserves typed text', () => {
  let draft = createSignatureDraft('\n\n> 原邮件正文', '个人签名');
  draft = editSignatureDraft(draft, '收到，谢谢。' + draft.text);
  draft = switchSignature(draft, '工作签名');
  assert.equal(draft.text, '收到，谢谢。\n\n工作签名\n\n> 原邮件正文');
  assert.equal(switchSignature(draft, '工作签名').text, draft.text);
});

test('editing the signature protects it when switching sender', () => {
  let draft = createSignatureDraft('', '工作签名');
  draft = editSignatureDraft(draft, draft.text.replace('工作签名', '我的手写落款'));
  assert.equal(switchSignature(draft, '个人签名').text, '\n\n我的手写落款');
});

test('switching to an empty signature removes only the automatic signature', () => {
  let draft = createSignatureDraft('\n\n> 工作签名', '工作签名');
  draft = editSignatureDraft(draft, '正文' + draft.text);
  assert.equal(switchSignature(draft, '').text, '正文\n\n> 工作签名');
});

test('a sender without a signature can gain one after typed body and before quoted content', () => {
  let draft = createSignatureDraft('\n\n> 引用', '');
  draft = editSignatureDraft(draft, '回复内容' + draft.text);
  draft = switchSignature(draft, '新签名');
  assert.equal(draft.text, '回复内容\n\n新签名\n\n> 引用');
  draft = switchSignature(draft, '');
  draft = switchSignature(draft, '另一个签名');
  assert.equal(draft.text, '回复内容\n\n另一个签名\n\n> 引用');
});

test('quote edits and text appended after a signature survive sender switching', () => {
  let draft = createSignatureDraft('\n\n> 原文', '签名');
  draft = editSignatureDraft(draft, draft.text.replace('原文', '保留的引用'));
  assert.equal(switchSignature(draft, '新签名').text, '\n\n新签名\n\n> 保留的引用');
});

test('deleting a signature is a deliberate edit and does not re-add it', () => {
  const draft = editSignatureDraft(createSignatureDraft('', '签名'), '');
  assert.equal(switchSignature(draft, '另一签名').text, '');
});

test('adding a blank line before the signature still permits sender replacement', () => {
  let draft = createSignatureDraft('', '旧签名');
  draft = editSignatureDraft(draft, '\n' + draft.text, { start: 0, end: 0 });
  assert.equal(switchSignature(draft, '新签名').text, '\n\n\n新签名');
});

test('extending the signature at its end protects the user edit', () => {
  let draft = createSignatureDraft('', '李嘉文');
  draft = editSignatureDraft(draft, draft.text + '，产品部');
  assert.equal(switchSignature(draft, 'Jarvis').text, '\n\n李嘉文，产品部');
});
