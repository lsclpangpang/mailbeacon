const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// A list replacement removes its focused DOM node, as innerHTML does in a browser.
function page() {
  const body = {}, document = { activeElement: body, addEventListener() {} };
  let nodes = [], markup = '', advanced = {open:false};
  const rows = { scrollTop:0, clientHeight:600, focus() { document.activeElement = this; }, closest(selector) { return selector === '#list' ? list : null; }, matches(selector) { return selector === '.rows'; } };
  const list = {
    contains(node) { return node === rows || nodes.includes(node); },
    get innerHTML() { return markup; },
    set innerHTML(html) {
      if (this.contains(document.activeElement)) document.activeElement = body;
      markup = html; advanced = {open:false};
      nodes = [...html.matchAll(/<(?:button|input|select)\b([^>]*)>/g)].map((match) => {
        const dataset = {};
        for (const [_, name, value] of match[1].matchAll(/data-([\w-]+)="([^"]*)"/g)) dataset[name.replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value.replaceAll('&quot;', '"').replaceAll('&amp;', '&');
        return { dataset, className: /class="([^"]*)"/.exec(match[1])?.[1] || '', id: /id="([^"]*)"/.exec(match[1])?.[1], selectionStart: 0, selectionEnd: 0, setSelectionRange(start,end) { this.selectionStart=start; this.selectionEnd=end; }, focus() { document.activeElement = this; }, closest(selector) { return selector === '#list' ? list : null; } };
      });
    },
    querySelector(selector) { return selector === '.rows' ? rows : selector === '.row.sel' ? nodes.find(n => n.className.split(/\s+/).includes('sel')) || null : selector === '.search-controls details' ? advanced : nodes.find(n => n.id && selector === '#'+n.id) || null; },
    querySelectorAll(selector) {
      if (selector === '[data-select-mail]') return nodes.filter(n => n.dataset.selectMail);
      if (selector === '[data-open]') return nodes.filter(n => n.dataset.open);
      if (selector === '[data-search-filter]') return nodes.filter(n => n.dataset.searchFilter);
      if (selector === '[data-action]') return nodes.filter(n => n.dataset.action);
      return [];
    },
  };
  document.querySelector = selector => selector === '#list' ? list : selector === '.rows' ? rows : null;
  document.querySelectorAll = selector => list.querySelectorAll(selector);
  const context = vm.createContext({ document, console, setTimeout, clearTimeout, window:{
    MailBeaconSelection:require('../ui/mail-selection.js'), MailBeaconOperations:require('../ui/mail-operations.js'), MailBeaconViews:require('../ui/views.js'), MailBeaconThreads:require('../ui/threads.js'), MailBeaconPrefs:require('../ui/prefs.js'),
    __TAURI__:{core:{invoke:async()=>{}},event:{listen:async()=>{}}},
  }});
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  vm.runInContext(source.slice(0,source.indexOf('(async function boot()'))+ '\nglobalThis.api={S,renderList};',context);
  const {S,renderList}=context.api;
  S.acct='a'; S.accounts=[{id:'a',email:'demo@example.test'}]; S.folders.a=[{path:'INBOX',name:'收件箱',role:'inbox'}];
  S.lists.set('a|INBOX',[{accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid:1,date:1,seen:true,subject:'Demo'}]);
  renderList();
  S.batchMode = true; renderList();
  const selected = list.querySelectorAll('[data-select-mail]')[0].dataset.selectMail;
  S.batch.add(selected); renderList();
  return { S, document, list, rows, renderList, selected };
}

for (const control of ['checkbox','toolbar','rows']) test(`list refresh keeps ${control} keyboard focus inside the batch`, () => {
  const t=page();
  const before = control === 'checkbox' ? t.list.querySelectorAll('[data-select-mail]')[0] : control === 'toolbar' ? t.list.querySelectorAll('[data-action]').find(n=>n.dataset.action === 'bulk-trash') : t.rows;
  before.focus();
  t.renderList();
  assert.equal(t.document.activeElement.closest?.('#list'),t.list);
  if (control === 'checkbox') assert.equal(t.document.activeElement.dataset.selectMail,t.selected);
  if (control === 'toolbar') assert.equal(t.document.activeElement.dataset.action,'bulk-trash');
});

test('when a focused selected mail disappears, list focus stays on the remaining list', () => {
  const t=page();
  t.list.querySelectorAll('[data-select-mail]')[0].focus();
  t.S.lists.set('a|INBOX',[]); t.renderList();
  assert.equal(t.document.activeElement,t.rows);
});

test('refreshing a focused list container restores the selected mail without moving the viewport', () => {
  const t=page();
  t.S.batchMode=false; t.S.batch.clear();
  t.S.sel={accountId:'a',folder:'INBOX',uid:1};
  t.renderList();
  t.rows.scrollTop=47;
  t.rows.focus();
  t.renderList();
  assert.equal(t.document.activeElement.dataset?.open,'a|1|INBOX');
  assert.equal(t.rows.scrollTop,47);
});

test('refreshing an ordinary focused mail row retains that row', () => {
  const t=page();
  t.S.batchMode=false; t.S.batch.clear();
  t.S.sel={accountId:'a',folder:'INBOX',uid:1};
  t.renderList();
  t.list.querySelectorAll('[data-open]')[0].focus();
  t.renderList();
  assert.equal(t.document.activeElement.dataset.open,'a|1|INBOX');
});

test('search result lists retain the multi-select menu entry point', () => {
  const t=page(); t.S.searching=true; t.renderList();
  assert.match(t.list.innerHTML,/data-action="list-menu"/);
});

test('updating search conditions preserves the advanced input focus and caret', () => {
  const t=page(); t.S.searching=true; t.renderList();
  t.list.querySelector('.search-controls details').open=true;
  const sender=t.list.querySelectorAll('[data-search-filter]').find(n=>n.dataset.searchFilter==='sender');
  sender.focus(); sender.setSelectionRange(2,4); t.S.searchFilters.sender='alice'; t.renderList();
  assert.equal(t.document.activeElement.dataset.searchFilter,'sender');
  assert.equal(t.document.activeElement.selectionStart,2); assert.equal(t.document.activeElement.selectionEnd,4);
  assert.equal(t.list.querySelector('.search-controls details').open,true);
});
