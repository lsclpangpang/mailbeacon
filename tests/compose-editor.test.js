const test = require('node:test');
const assert = require('node:assert/strict');
const editor = require('../ui/compose-editor.js');

test('toolbar actions map to editing commands', () => {
  assert.deepEqual(editor.commandFor('bold'), ['bold', null]);
  assert.deepEqual(editor.commandFor('ul'), ['insertUnorderedList', null]);
  assert.deepEqual(editor.commandFor('align-center'), ['justifyCenter', null]);
  assert.deepEqual(editor.commandFor('size', 'large'), ['fontSize', '5']);
  assert.deepEqual(editor.commandFor('font', 'SimSun'), ['fontName', 'SimSun']);
  assert.deepEqual(editor.commandFor('color', '#c00000'), ['foreColor', '#c00000']);
  assert.deepEqual(editor.commandFor('clear'), ['removeFormat', null]);
  assert.equal(editor.commandFor('size', 'gigantic'), null, '未知字号不执行');
  assert.equal(editor.commandFor('color', 'red;background:url(x)'), null, '颜色只接受 #rrggbb');
  assert.equal(editor.commandFor('nope'), null);
});

test('links are normalized and unsafe schemes are refused', () => {
  assert.equal(editor.normalizeLink('example.com/a'), 'https://example.com/a');
  assert.equal(editor.normalizeLink('  https://example.com  '), 'https://example.com');
  assert.equal(editor.normalizeLink('me@example.com'), 'mailto:me@example.com');
  assert.equal(editor.normalizeLink('mailto:me@example.com'), 'mailto:me@example.com');
  assert.equal(editor.normalizeLink('tel:+8613812345678'), 'tel:+8613812345678');
  assert.equal(editor.normalizeLink('https://'), null);
  assert.equal(editor.normalizeLink('mailto:'), null);
  assert.equal(editor.normalizeLink('tel:abc'), null);
  assert.equal(editor.normalizeLink('javascript:alert(1)'), null);
  assert.equal(editor.normalizeLink('data:text/html,x'), null);
  assert.equal(editor.normalizeLink(''), null);
});

test('tables are bounded and empty cells stay editable', () => {
  const html = editor.tableHtml(2, 3);
  assert.equal((html.match(/<tr>/g) || []).length, 2);
  assert.equal((html.match(/<td[ >]/g) || []).length, 6);
  assert.match(html, /border-collapse/);
  const huge = editor.tableHtml(500, 500);
  assert.ok((huge.match(/<tr>/g) || []).length <= 20);
  assert.ok((huge.match(/<td[ >]/g) || []).length <= 20 * 10);
});

test('the signature is a separate block that switches only while untouched', () => {
  const alice = editor.signatureBlock('Alice <a@x>\n电话 1');
  assert.equal(alice, '<div class="mp-signature" data-mp-signature="">Alice &lt;a@x&gt;<br>电话 1</div>');
  assert.equal(editor.signatureBlock(''), '');
  const start = editor.initialHtml('', 'Alice');
  assert.ok(start.endsWith(editor.signatureBlock('Alice')));
  // 未改动的签名：换发件人时替换
  let next = editor.switchSignature(start + '<div>正文</div>', editor.signatureBlock('Alice'), 'Bob');
  assert.ok(next.html.includes(editor.signatureBlock('Bob')));
  assert.ok(!next.html.includes('Alice'));
  assert.ok(next.html.includes('<div>正文</div>'), '正文保持不变');
  // 用户改过签名：不再替换，也不再管理
  const edited = start.replace('Alice', 'Alice（已改）');
  next = editor.switchSignature(edited, editor.signatureBlock('Alice'), 'Bob');
  assert.equal(next.html, edited);
  assert.equal(next.block, null);
  // 原来没有签名：切换到有签名的发件人时追加
  next = editor.switchSignature('<div>正文</div>', '', 'Bob');
  assert.equal(next.html, '<div>正文</div>' + editor.signatureBlock('Bob'));
  // 换成没有签名的发件人：去掉自动签名
  next = editor.switchSignature(start, editor.signatureBlock('Alice'), '');
  assert.ok(!next.html.includes('mp-signature'));
});

test('html becomes readable plain text for the text/plain alternative', () => {
  const html = '<div>第一行</div><div>第二行<br>第三行</div><ul><li>甲</li><li>乙</li></ul>' +
    '<table><tr><td>a</td><td>b</td></tr></table><p>&lt;tag&gt; &amp; &nbsp;x</p><div><br></div>' +
    '<a href="https://e.com">链接</a><style>p{}</style><script>bad()</script>';
  const text = editor.htmlToText(html);
  assert.equal(text, '第一行\n第二行\n第三行\n- 甲\n- 乙\na\tb\n\n<tag> &  x\n\n链接 <https://e.com>');
  assert.equal(editor.htmlToText(editor.textToHtml('a\n\nb')), 'a\n\nb');
});

test('htmlToText handles quoted angle brackets, pre blocks, numbered lists and paragraphs', () => {
  assert.equal(editor.htmlToText('<img alt="x > y" src="data:image/png;base64,AA">文字'), '文字');
  assert.equal(editor.htmlToText('<pre>a\n  b</pre>'), 'a\n  b');
  assert.equal(editor.htmlToText('<ol><li>一</li><li>二</li></ol><ul><li>甲</li></ul>'), '1. 一\n2. 二\n- 甲');
  assert.equal(editor.htmlToText('<p>段一</p><p>段二</p>'), '段一\n\n段二');
});

test('htmlToText keeps telephone destinations in the plain-text alternative', () => {
  assert.equal(editor.htmlToText('<p>联系<a href="tel:+8613812345678">客服</a></p>'), '联系客服 <+8613812345678>');
  assert.equal(editor.htmlToText('<a href="tel:+8613812345678">+8613812345678</a>'), '+8613812345678');
});

test('snapshots fingerprint inline image data instead of carrying it', () => {
  const big = 'A'.repeat(100000);
  const one = editor.fingerprintHtml(`<p>x</p><img src="data:image/png;base64,${big}">`);
  assert.ok(one.length < 300, '快照里不带整段 base64');
  assert.notEqual(one, editor.fingerprintHtml(`<p>x</p><img src="data:image/png;base64,${big}B">`), '图片变了快照也要变');
  assert.equal(editor.fingerprintHtml('<p>无图</p>'), '<p>无图</p>');
});

test('reopening strips only a single wrapper div around the whole body', () => {
  assert.equal(editor.unwrapOuterDiv('<div><div>a</div><div>b</div></div>'), '<div>a</div><div>b</div>');
  assert.equal(editor.unwrapOuterDiv('<div style="font:14px">x<br>y</div>'), 'x<br>y');
  assert.equal(editor.unwrapOuterDiv('<div>a</div><div>b</div>'), '<div>a</div><div>b</div>', '两个并列的 div 不是外层包装');
  assert.equal(editor.unwrapOuterDiv('  <div>a</div>\n'), 'a');
  assert.equal(editor.unwrapOuterDiv('<p>a</p>'), '<p>a</p>');
});
