const test = require('node:test');
const assert = require('node:assert/strict');
const search = require('../ui/mail-search.js');

const mark = text => `<mark class="mail-search-hit" data-mailbeacon-search-hit>${text}</mark>`;
const hits = root => root.querySelectorAll('[data-mailbeacon-search-hit]');

// A small DOM tree supports native text-node operations and records mutations.
// No HTML parser is provided: a highlighter must create text safely, preserve
// existing elements and attributes, and never rewrite their innerHTML.
function tree() {
  const document = { mutations: 0 };
  class Node {
    constructor(type, name) { this.nodeType = type; this.nodeName = name; this.ownerDocument = document; this.parentNode = null; this.childNodes = []; }
    get parentElement() { return this.parentNode?.nodeType === 1 ? this.parentNode : null; }
    get firstChild() { return this.childNodes[0] || null; }
    get nextSibling() { return this.parentNode?.childNodes[this.parentNode.childNodes.indexOf(this) + 1] || null; }
    get textContent() { return this.childNodes.map(child => child.textContent).join(''); }
    set textContent(value) {
      for (const child of [...this.childNodes]) this.removeChild(child);
      if (value) this.appendChild(document.createTextNode(String(value)));
    }
    insertBefore(node, reference) {
      if (node.nodeType === 11) { for (const child of [...node.childNodes]) this.insertBefore(child, reference); return node; }
      if (node.parentNode) node.parentNode.removeChild(node);
      const index = reference == null ? this.childNodes.length : this.childNodes.indexOf(reference);
      assert.ok(index >= 0, 'insertBefore reference belongs to the parent');
      this.childNodes.splice(index, 0, node); node.parentNode = this; document.mutations++;
      return node;
    }
    appendChild(node) { return this.insertBefore(node, null); }
    append(...nodes) { for (const node of nodes) this.appendChild(typeof node === 'string' ? document.createTextNode(node) : node); }
    removeChild(node) {
      const index = this.childNodes.indexOf(node); assert.ok(index >= 0);
      this.childNodes.splice(index, 1); node.parentNode = null; document.mutations++;
      return node;
    }
    replaceChild(node, old) { this.insertBefore(node, old); this.removeChild(old); return old; }
    replaceWith(...nodes) {
      const parent = this.parentNode;
      for (const node of nodes) parent.insertBefore(typeof node === 'string' ? document.createTextNode(node) : node, this);
      parent.removeChild(this);
    }
    remove() { this.parentNode?.removeChild(this); }
    normalize() {
      let previous;
      for (const child of [...this.childNodes]) {
        if (child.nodeType === 3 && !child.data) { this.removeChild(child); continue; }
        if (child.nodeType === 3 && previous?.nodeType === 3) { previous.data += child.data; this.removeChild(child); continue; }
        child.normalize(); previous = child;
      }
    }
    querySelectorAll(selector) {
      return this.childNodes.flatMap(child => [
        ...(child.nodeType === 1 && child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector),
      ]);
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  }
  class Text extends Node {
    constructor(value) { super(3, '#text'); this._data = String(value); }
    get data() { return this._data; }
    set data(value) { this._data = String(value); document.mutations++; }
    get nodeValue() { return this.data; }
    set nodeValue(value) { this.data = value; }
    get textContent() { return this.data; }
    set textContent(value) { this.data = value; }
  }
  class Element extends Node {
    constructor(tag) { super(1, tag.toUpperCase()); this.tagName = this.nodeName; this.attributes = new Map(); }
    setAttribute(name, value) { this.attributes.set(name, String(value)); document.mutations++; }
    getAttribute(name) { return this.attributes.has(name) ? this.attributes.get(name) : null; }
    hasAttribute(name) { return this.attributes.has(name); }
    removeAttribute(name) { this.attributes.delete(name); document.mutations++; }
    get className() { return this.getAttribute('class') || ''; }
    set className(value) { this.setAttribute('class', value); }
    get style() {
      const values = Object.fromEntries((this.getAttribute('style') || '').split(';').filter(part => part.includes(':')).map(part => {
        const colon = part.indexOf(':');
        return [part.slice(0, colon).trim(), part.slice(colon + 1).replace(/!important\s*$/i, '').trim()];
      }));
      return values;
    }
    set innerHTML(_) { throw new Error('Search highlighting must not replace HTML'); }
    matches(selector) {
      return selector.split(',').some(part => {
        part = part.trim();
        const tag = /^[a-z][\w-]*/i.exec(part)?.[0];
        if (tag && this.tagName !== tag.toUpperCase()) return false;
        for (const [, name] of part.matchAll(/\.([\w-]+)/g)) if (!this.className.split(/\s+/).includes(name)) return false;
        for (const [, name, value] of part.matchAll(/\[([\w-]+)(?:=["']?([^"'\]]+)["']?)?\]/g)) {
          if (!this.hasAttribute(name) || (value !== undefined && this.getAttribute(name) !== value)) return false;
        }
        return true;
      });
    }
    closest(selector) {
      for (let node = this; node; node = node.parentElement) if (node.matches(selector)) return node;
      return null;
    }
  }
  document.createElement = tag => new Element(tag);
  document.createTextNode = value => new Text(value);
  document.createDocumentFragment = () => new Node(11, '#document-fragment');
  document.createTreeWalker = (root, show, filter) => {
    assert.equal(show, 4, 'walk text nodes only');
    const all = [];
    const stack = [...root.childNodes].reverse();
    while (stack.length) {
      const node = stack.pop();
      if (node.nodeType === 3) all.push(node);
      for (let i = node.childNodes.length - 1; i >= 0; i--) stack.push(node.childNodes[i]);
    }
    let index = 0;
    return { nextNode() {
      while (index < all.length) {
        const node = all[index++], accept = typeof filter === 'function' ? filter(node) : filter?.acceptNode(node);
        if (accept === undefined || accept === 1) return node;
      }
      return null;
    } };
  };
  const el = (tag, attrs = {}, ...children) => {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, value);
    node.append(...children);
    return node;
  };
  return { document, el, text: document.createTextNode };
}

test('plain-text highlighting escapes markup and matches Chinese occurrences', () => {
  assert.equal(search.highlightText('合同 <报告> & "确认" \'原文\' 合同', '合同'), `${mark('合同')} &lt;报告&gt; &amp; &quot;确认&quot; &#39;原文&#39; ${mark('合同')}`);
});

test('query is a trimmed literal phrase and English matching preserves original case', () => {
  assert.equal(search.highlightText('Alpha beta ALPHA BETA alpha alone', ' alpha beta '), `${mark('Alpha beta')} ${mark('ALPHA BETA')} alpha alone`);
  assert.equal(search.highlightText('x [a+b].*? y aab', '[a+b].*?'), `x ${mark('[a+b].*?')} y aab`);
});

test('an empty query only escapes text and never marks empty matches', () => {
  assert.equal(search.highlightText('<img src=x onerror="bad()">&', '  '), '&lt;img src=x onerror=&quot;bad()&quot;&gt;&amp;');
  assert.equal(search.highlightText('', '合同'), '');
});

test('matching keeps correct offsets after Unicode characters with length-changing lowercase', () => {
  assert.equal(search.highlightText('İstanbul REPORT', 'report'), `İstanbul ${mark('REPORT')}`);
});

test('DOM highlights visible text while preserving link attributes and existing markup', () => {
  const { document, el } = tree();
  const link = el('a', { href: 'https://example.test/合同', 'data-link': 'https://example.test/合同' }, '合同下载');
  const originalMark = el('mark', { class: 'sender-mark' }, '合同原文');
  const root = el('div', {}, '合同正文 ', link, originalMark);
  const before = root.textContent;
  search.highlight(root, '合同');
  assert.equal(hits(root).length, 3);
  assert.equal(root.textContent, before);
  assert.equal(root.childNodes.includes(link), true);
  assert.equal(root.childNodes.includes(originalMark), true);
  assert.equal(link.getAttribute('href'), 'https://example.test/合同');
  assert.equal(link.getAttribute('data-link'), 'https://example.test/合同');
  for (const hit of hits(root)) assert.equal(hit.className, 'mail-search-hit');
  document.mutations = 0;
  search.highlight(root, '合同');
  assert.equal(document.mutations, 0, 'repeating the same query is a no-op');
});

test('changing and clearing DOM highlights removes only owned marks and rejoins text', () => {
  const { el } = tree();
  const originalMark = el('mark', { title: '邮件原有标记' }, '保留');
  const root = el('div', {}, '合同与报告合同', originalMark);
  search.highlight(root, '合同');
  assert.equal(hits(root).length, 2);
  search.highlight(root, '报告');
  assert.deepEqual(hits(root).map(hit => hit.textContent), ['报告']);
  search.highlight(root, '');
  assert.equal(hits(root).length, 0);
  assert.equal(root.textContent, '合同与报告合同保留');
  assert.equal(root.childNodes.length, 2, 'split plain text is merged back beside the original mark');
  assert.equal(root.firstChild.nodeType, 3);
  assert.equal(root.firstChild.textContent, '合同与报告合同');
  assert.equal(root.childNodes[1], originalMark);
  assert.equal(originalMark.getAttribute('title'), '邮件原有标记');
});

test('DOM matching treats HTML-looking mail content as text', () => {
  const { el } = tree();
  const payload = '<img src=x onerror="bad()">';
  const root = el('p', {}, payload);
  search.highlight(root, payload);
  assert.equal(hits(root).length, 1);
  assert.equal(hits(root)[0].firstChild.nodeType, 3);
  assert.equal(root.querySelectorAll('img').length, 0);
  assert.equal(root.textContent, payload);
});

test('DOM walker skips code, input controls and hidden ancestor text', () => {
  const { el } = tree();
  const skipped = ['script', 'style', 'head', 'noscript', 'textarea', 'select', 'option', 'input'].map(tag => el(tag, {}, '合同'));
  const svg = el('svg', {}, el('text', {}, '合同'));
  const math = el('math', {}, el('mi', {}, '合同'));
  svg.tagName = 'svg'; math.tagName = 'math'; // Foreign elements keep lowercase tagName in HTML documents.
  skipped.push(svg, math);
  skipped.push(el('div', { hidden: '' }, el('span', {}, '合同')));
  skipped.push(el('div', { 'aria-hidden': 'true' }, el('span', {}, '合同')));
  const visible = el('span', { 'aria-hidden': 'false' }, '合同');
  const root = el('div', {}, ...skipped, visible);
  search.highlight(root, '合同');
  assert.equal(hits(root).length, 1);
  assert.equal(hits(visible).length, 1);
  for (const node of skipped) { assert.equal(hits(node).length, 0); assert.equal(node.textContent, '合同'); }
});

test('a nonmatching query leaves the existing text nodes untouched', () => {
  const { document, el, text } = tree();
  const node = text('没有匹配的内容');
  const root = el('p', {}, node);
  document.mutations = 0;
  search.highlight(root, '合同');
  assert.equal(root.firstChild, node);
  assert.equal(document.mutations, 0);
});

test('clearing highlights preserves a sender mark even when its attribute resembles our marker', () => {
  const { el } = tree();
  const originalMark = el('mark', { 'data-mailbeacon-search-hit': '', class: 'sender-mark', title: '保留格式' }, '原有标记');
  const root = el('div', {}, '合同', originalMark);
  assert.equal(search.highlight(root, '合同'), 1);
  assert.equal(search.highlight(root, ''), 0);
  assert.equal(root.childNodes[1], originalMark);
  assert.equal(originalMark.textContent, '原有标记');
  assert.equal(originalMark.className, 'sender-mark');
  assert.equal(originalMark.getAttribute('title'), '保留格式');
  assert.equal(originalMark.hasAttribute('data-mailbeacon-search-hit'), true);
});

test('a keyword split across Word-style inline elements keeps each original element and link', () => {
  const { document, el } = tree();
  const bold = el('b', { style: 'font-weight:bold' }, '同');
  const link = el('a', { href: 'https://example.test/report', 'data-link': 'original' }, '告');
  const span = el('span', { class: 'word-run' }, '报', link);
  const root = el('p', {}, '合', bold, span);
  assert.equal(search.highlight(root, '合同报告'), 4);
  assert.deepEqual(hits(root).map(hit => hit.textContent), ['合', '同', '报', '告']);
  assert.equal(root.textContent, '合同报告');
  assert.equal(root.childNodes.includes(bold), true);
  assert.equal(root.childNodes.includes(span), true);
  assert.equal(span.childNodes.includes(link), true);
  assert.equal(link.getAttribute('href'), 'https://example.test/report');
  assert.equal(link.getAttribute('data-link'), 'original');
  document.mutations = 0;
  search.highlight(root, '合同报告');
  assert.equal(document.mutations, 0);
  search.highlight(root, '');
  assert.equal(root.textContent, '合同报告');
  assert.equal(root.childNodes[1], bold);
  assert.equal(root.childNodes[2], span);
  assert.equal(span.childNodes[1], link);
});

test('multiple inline-spanning matches retain the unmatched prefix, suffix and middle text', () => {
  const { el } = tree();
  const first = el('span', {}, '同与合');
  const second = el('b', {}, '同y');
  const root = el('p', {}, 'x合', first, second);
  search.highlight(root, '合同');
  assert.deepEqual(hits(root).map(hit => hit.textContent), ['合', '同', '合', '同']);
  assert.equal(root.textContent, 'x合同与合同y');
  search.highlight(root, '与');
  assert.deepEqual(hits(root).map(hit => hit.textContent), ['与']);
  search.highlight(root, '');
  assert.equal(root.firstChild.textContent, 'x合');
  assert.equal(first.textContent, '同与合');
  assert.equal(second.textContent, '同y');
});

test('text runs stop at paragraphs, table cells, line breaks and omitted or styled boundaries', () => {
  const builders = [
    el => el('div', {}, el('p', {}, '合'), el('p', {}, '同')),
    el => el('table', {}, el('tr', {}, el('td', {}, '合'), el('td', {}, '同'))),
    el => el('p', {}, '合', el('br'), '同'),
    el => el('div', {}, '合', el('hr'), '同'),
    el => el('p', {}, '合', el('span', { hidden: '' }, '略'), '同'),
    el => el('p', {}, '合', el('span', { 'aria-hidden': 'true' }, '略'), '同'),
    el => el('p', {}, '合', el('script', {}, '略'), '同'),
    el => el('p', {}, '合', el('svg', {}, el('text', {}, '略')), '同'),
    el => el('p', {}, '合', el('span', { style: 'display:none' }, '略'), '同'),
    el => el('p', {}, '合', el('span', { style: 'visibility:hidden' }, '略'), '同'),
    el => el('p', {}, '合', el('span', { style: 'display:inline-block' }, '同')),
    el => el('p', {}, '合', el('span', { style: 'display:block' }, '同')),
  ];
  for (const [index, build] of builders.entries()) {
    const { el } = tree(), root = build(el);
    assert.equal(search.highlight(root, '合同'), 0, `boundary fixture ${index}`);
    assert.equal(hits(root).length, 0);
  }
  const { el } = tree();
  const root = el('div', {}, el('p', {}, '合同'), el('p', {}, '合同'));
  assert.equal(search.highlight(root, '合同'), 2, 'a boundary does not hide text inside its block');
});

test('deeply nested inline email formatting is traversed without recursive stack growth', () => {
  const { el } = tree();
  const root = el('p', {}, '合');
  let leaf = root;
  for (let i = 0; i < 12000; i++) {
    const child = el('span'); leaf.appendChild(child); leaf = child;
  }
  leaf.append('同');
  assert.equal(search.highlight(root, '合同'), 2);
  assert.equal(leaf.firstChild.tagName, 'MARK');
  assert.equal(leaf.firstChild.textContent, '同');
});

test('large repeated bodies cap mark fragments while retaining all text and allowing a new query', () => {
  for (const inline of [false, true]) {
    const { document, el } = tree();
    const root = el('p');
    if (inline) {
      root.append('ab'); // An odd remaining budget ends within the last spanning match.
      for (let i = 0; i < 2600; i++) root.append(el('span', {}, 'a'), el('b', {}, 'b'));
    } else root.append('a '.repeat(5100));
    root.append(' finish');
    const original = root.textContent;
    assert.equal(search.highlight(root, inline ? 'ab' : 'a'), 5000);
    assert.equal(hits(root).length, 5000);
    assert.equal(root.textContent, original, 'unmarked remainder is still complete');
    document.mutations = 0;
    search.highlight(root, inline ? 'ab' : 'a');
    assert.equal(document.mutations, 0, 'the capped result is also idempotent');
    assert.equal(search.highlight(root, ''), 0);
    assert.equal(root.textContent, original);
    assert.equal(hits(root).length, 0);
    assert.equal(search.highlight(root, 'finish'), 1, 'a new query receives a fresh mark budget');
    assert.equal(root.textContent, original);
  }
});
