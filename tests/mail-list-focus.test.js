const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// A list replacement removes its focused DOM node, as innerHTML does in a browser.
function page() {
  const body = {}, document = { activeElement: body, addEventListener() {} };
  let nodes = [], markup = '';
  const rows = { scrollTop:0, clientHeight:600, focus() { document.activeElement = this; }, closest(selector) { return selector === '#list' ? list : null; }, matches(selector) { return selector === '.rows'; } };
  const list = {
    contains(node) { return node === rows || nodes.includes(node); },
    get innerHTML() { return markup; },
    set innerHTML(html) {
      if (this.contains(document.activeElement)) document.activeElement = body;
      markup = html;
      nodes = [...html.matchAll(/<button\b([^>]*)>/g)].map((match) => {
        const dataset = {};
        for (const [_, name, value] of match[1].matchAll(/data-([\w-]+)="([^"]*)"/g)) dataset[name.replace(/-([a-z])/g, (_, c) => c.toUpperCase())] = value.replaceAll('&quot;', '"').replaceAll('&amp;', '&');
        return { dataset, focus() { document.activeElement = this; }, closest(selector) { return selector === '#list' ? list : null; } };
      });
    },
    querySelector(selector) { return selector === '.rows' ? rows : null; },
    querySelectorAll(selector) {
      if (selector === '[data-select-mail]') return nodes.filter(n => n.dataset.selectMail);
      if (selector === '[data-open]') return nodes.filter(n => n.dataset.open);
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
