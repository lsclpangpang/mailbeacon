const test = require('node:test');
const assert = require('node:assert/strict');
const { createQuote, composeBody, previewDocument, sanitizeCss, safeUrl } = require('../ui/compose-quote.js');

test('a plain-text reply preserves the original text with quoted lines', () => {
  const quote = createQuote({ text: '原文第一行\r\n\r\n第二行', header: '小李写道：' });
  assert.equal(quote.rich, false);
  assert.equal(composeBody('谢谢\n会按时到场。', quote).body, '谢谢\n会按时到场。\n\n小李写道：\n> 原文第一行\n> \n> 第二行');
});

test('an absent quote keeps ordinary new mail plain text', () => {
  assert.deepEqual(composeBody('新邮件\n<hello>', null), { body: '新邮件\n<hello>', htmlBody: null });
});

test('HTML-only messages fail closed to an escaped plain fallback without a browser parser', () => {
  const quote = createQuote({ html: '<script>bad()</script><b>原文</b>', text: '原文', header: 'A <a@example.test> 写道：' });
  assert.equal(quote.rich, false);
  assert.doesNotMatch(quote.html, /<script|<b>/);
  assert.match(quote.html, /原文/);
});

test('rich replies escape typed content and preserve complete original HTML separately', () => {
  const quote = { rich: true, html: '<div class="mailpulse-original"><table><tr><td style="color:red">原邮件</td></tr></table></div>', text: '原邮件', header: 'A <a@example.test> 写道：' };
  const result = composeBody('收到 <不是标签>\n第二行 & 确认', quote);
  assert.equal(result.body, '收到 <不是标签>\n第二行 & 确认\n\nA <a@example.test> 写道：\n> 原邮件');
  assert.match(result.htmlBody, /收到 &lt;不是标签&gt;<br>第二行 &amp; 确认/);
  assert.match(result.htmlBody, /A &lt;a@example.test&gt; 写道：/);
  assert.match(result.htmlBody, /<table><tr><td style="color:red">原邮件<\/td><\/tr><\/table>/);
  assert.doesNotMatch(result.htmlBody, /&gt; 原邮件/);
});

test('quote preview denies scripts, forms and remote content by default', () => {
  const quote = { html: '<div class="mailpulse-original">内容</div>' };
  const preview = previewDocument(quote);
  assert.match(preview, /default-src 'none'/);
  assert.match(preview, /img-src data: cid:/);
  assert.doesNotMatch(preview, /img-src[^;]*https:/);
  assert.match(preview, /form-action 'none'/);
  assert.match(previewDocument(quote, { allowRemoteImages: true }), /img-src data: cid: https: http:/);
});

test('safe URLs retain original links and embedded image formats but reject executable URLs', () => {
  assert.equal(safeUrl('https://example.test/path?a=1&b=2', false), true);
  assert.equal(safeUrl('mailto:hello@example.test', false), true);
  assert.equal(safeUrl('data:image/png;base64,iVBORw0KGgo=', true), true);
  assert.equal(safeUrl('cid:original-logo', true), true);
  for (const value of ['javascript:alert(1)', 'java\nscript:alert(1)', 'data:text/html,hi', 'file:///tmp/private', 'blob:secret']) assert.equal(safeUrl(value, false), false, value);
  assert.equal(safeUrl('data:image/svg+xml,<svg onload="bad()"/>', true), false);
});

test('safe CSS keeps table layout, text formatting and media queries', () => {
  const css = 'table { border-collapse: collapse; width: 100% } .notice, td b { color: #234; font-weight: bold } @media (max-width: 600px) { .notice { padding: 8px } }';
  const result = sanitizeCss(css);
  assert.match(result, /border-collapse: collapse/);
  assert.match(result, /font-weight: bold/);
  assert.match(result, /@media \(max-width: 600px\)/);
});

test('CSS removes imports, executable legacy properties and unsafe URL schemes including escapes', () => {
  const css = '@import "https://tracking.example/style.css"; p { color:red; width:expression(alert(1)); behavior:url(x.htc); background:url(javascript:alert(1)); }';
  const result = sanitizeCss(css);
  assert.doesNotMatch(result, /@import|expression|behavior|javascript/i);
  assert.match(result, /color:red/);
  assert.equal(sanitizeCss('background: u\\72l(javascript:alert(1))', true), '');
  assert.equal(sanitizeCss('-moz-binding: url(https://example.test/x.xml)', true), '');
});

test('CSS scopes original body and document selectors away from the new reply', () => {
  const result = sanitizeCss('body { color:red } html body .item, h1 { font-size: 20px } @media screen { table { width: 300px } }');
  assert.match(result, /\.mailpulse-original\s*\{ color:red \}/);
  assert.match(result, /\.mailpulse-original \.item, \.mailpulse-original h1/);
  assert.match(result, /@media screen\s*\{\s*\.mailpulse-original table/);
  assert.doesNotMatch(result, /(?:^|\})\s*body\s*\{/);
});

test('quoted plain text and headers are escaped in preview', () => {
  const quote = createQuote({ text: '<img src=x onerror=bad()> & 原文', header: 'Someone' });
  assert.match(previewDocument(quote), /&lt;img src=x onerror=bad\(\)&gt; &amp; 原文/);
});

test('CID collection reads image URLs, decodes URI IDs, and ignores ordinary text', () => {
  const { cssCids } = require('../ui/compose-quote.js');
  assert.deepEqual(cssCids('background: url("cid:logo%40example.test"); background-image:url(cid:second); content:"cid:not-an-image"'), ['logo@example.test', 'second']);
  assert.deepEqual(cssCids('content: "cid:ignored"; background: url(https://example.test/a.png)'), []);
});

test('CSS preserves punctuation inside strings and safely quoted image URLs', () => {
  const result = sanitizeCss('font-family: "Some; Font"; color: red; background: url("https://example.test/a.png?v=1;2")', true);
  assert.match(result, /font-family: "Some; Font"/);
  assert.match(result, /background: url\("https:\/\/example.test\/a.png\?v=1;2"\)/);
});

test('image URLs cannot inject quotes, markup or CSS delimiters into wrapper backgrounds', () => {
  for (const url of ['https://example.test/x";color:red', 'https://example.test/<x>', 'https://example.test/\\bad']) assert.equal(safeUrl(url, true), false);
});

test('inert parsing preserves body attributes without rewriting quoted text or CSS', () => {
  const { inertSource } = require('../ui/compose-quote.js');
  const input = '<html><head><style>p:before{content:"<body>"}</style></head><body style="color:red" title="<body>quoted"><p>正文</p></body></html>';
  assert.equal(inertSource(input), '<html><head><style>p:before{content:"<body>"}</style></head><mailpulse-body style="color:red" title="<body>quoted"><p>正文</p></mailpulse-body></html>');
});

test('CID-looking text inside a CSS content string is not an image reference', () => {
  const { cssCids } = require('../ui/compose-quote.js');
  assert.deepEqual(cssCids('content: "url(cid:not-an-image)"; background:url(cid:actual)'), ['actual']);
});

test('escaped CSS quotes do not turn content text into CID references', () => {
  const { cssCids } = require('../ui/compose-quote.js');
  assert.deepEqual(cssCids(String.raw`content: "escaped \" url(cid:fake)"; background:url(cid:actual)`), ['actual']);
  assert.deepEqual(cssCids(String.raw`background:u\72l("cid:escaped-function")`), ['escaped-function']);
});


test('deeply nested mail CSS is bounded without losing adjacent ordinary rules', () => {
  const css = 'p { color:green }' + '@media screen {'.repeat(6000) + 'p { color:red }' + '}'.repeat(6000) + 'td { padding:8px }';
  const result = sanitizeCss(css);
  assert.match(result, /color:green/);
  assert.match(result, /padding:8px/);
  assert.doesNotMatch(result, /color:red/);
});

test('oversized stylesheets and declarations are discarded without partial CSS', () => {
  const large = 'a'.repeat(1024 * 1024 + 1);
  assert.equal(sanitizeCss('p { font-family:' + large + ' }'), '');
  assert.equal(sanitizeCss('font-family:' + large, true), '');
});

test('an unexpected HTML parser failure preserves the plain quote and allows composing', () => {
  const previous = globalThis.DOMParser;
  globalThis.DOMParser = class { parseFromString() { throw new RangeError('parser depth exceeded'); } };
  try {
    const quote = createQuote({ html: '<p>原文</p>', text: '原文', header: '对方写道：' });
    assert.equal(quote.rich, false);
    assert.equal(quote.text, '原文');
    assert.match(composeBody('收到', quote).body, /> 原文/);
  } finally {
    if (previous === undefined) delete globalThis.DOMParser; else globalThis.DOMParser = previous;
  }
});

test('rich reply HTML is used as the HTML body, with or without a quote', () => {
  const reply = '<div><b>粗体</b>回复</div>';
  const alone = composeBody('粗体回复', null, reply);
  assert.equal(alone.body, '粗体回复');
  assert.match(alone.htmlBody, /<b>粗体<\/b>回复/);
  assert.match(alone.htmlBody, /^<!doctype html>/);

  // 测试环境没有浏览器 HTML 解析器，直接构造已清理过的引用
  const rich = { rich: true, html: '<p>原文</p>', text: '原文', header: '在某时，A 写道：' };
  const withRich = composeBody('粗体回复', rich, reply);
  assert.match(withRich.htmlBody, /<b>粗体<\/b>回复[\s\S]*<blockquote[^>]*>[\s\S]*原文/);
  assert.match(withRich.body, /粗体回复\n\n在某时，A 写道：\n> 原文/);

  const plain = createQuote({ html: '', text: '纯文本 <原文>', header: 'A 写道：' });
  const withPlain = composeBody('回复', plain, reply);
  assert.match(withPlain.htmlBody, /<b>粗体<\/b>回复/);
  assert.match(withPlain.htmlBody, /纯文本 &lt;原文&gt;/, '纯文本引用在 HTML 里必须转义');
  assert.doesNotMatch(withPlain.htmlBody, /<原文>/);
});
