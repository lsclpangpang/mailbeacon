const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');

// 读信页头部的收件人/抄送：名字要能点开看地址，这里只测纯函数，不碰 DOM。
function api() {
  const source = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const context = vm.createContext({
    window: {
      MailBeaconViews: require('../ui/views.js'),
      MailBeaconPrefs: require('../ui/prefs.js'), MailBeaconBodyCache: require('../ui/body-cache.js'),
      MailBeaconRules: require('../ui/rules.js'),
      MailBeaconThreads: require('../ui/threads.js'),
      MailBeaconExtras: require('../ui/compose-extras.js'),
      __TAURI__: { core: { invoke: async () => {} }, event: { listen: async () => {} } },
    },
    document: { querySelector: () => null, addEventListener() {} },
    navigator: {},
    console,
    setTimeout,
    clearTimeout,
  });
  const beforeBoot = source.slice(0, source.indexOf('(async function boot()'));
  vm.runInContext(`${beforeBoot}
    globalThis.api = { recipientChips, personMenuEntries };
  `, context);
  return context.api;
}

test('recipient chips expose the email address for 收件人 and 抄送', () => {
  const { recipientChips } = api();
  const html = recipientChips([
    { name: '李雷', addr: 'lilei@example.com' },
    { name: '', addr: 'hanmeimei@example.com' },
  ], 'me@example.com');

  assert.match(html, />李雷</);
  assert.match(html, />hanmeimei@example\.com</, '没有名字时显示地址');
  assert.match(html, /data-addr="lilei@example\.com"/);
  assert.match(html, /title="李雷 &lt;lilei@example\.com&gt;"/, '悬停能看到完整地址');
  assert.match(html, /data-action="show-person"/);
  assert.equal(html.split('、').length, 2, '多个收件人用顿号分隔');
});

test('recipient chips show self as 我 but keep the real address', () => {
  const { recipientChips } = api();
  const html = recipientChips([{ name: '张三', addr: 'me@example.com' }], 'me@example.com');
  assert.match(html, />我</);
  assert.match(html, /data-addr="me@example\.com"/);
  assert.match(html, /title="张三 &lt;me@example\.com&gt;"/);
});

test('recipient chips escape names and addresses from the message header', () => {
  const { recipientChips } = api();
  const html = recipientChips([{ name: '<img src=x onerror=alert(1)>', addr: 'a&b@example.com' }], '');
  assert.doesNotMatch(html, /<img/);
  assert.match(html, /&lt;img src=x onerror=alert\(1\)&gt;/);
  assert.match(html, /data-addr="a&amp;b@example\.com"/);
});

test('person menu shows the full address and offers copy and compose', () => {
  const { personMenuEntries } = api();
  const entries = personMenuEntries('李雷', 'lilei@example.com');
  assert.equal(entries[0].label, '李雷 <lilei@example.com>');
  assert.equal(entries[0].disabled, true, '地址本身只是展示，不触发动作');
  assert.ok(entries.some((e) => e.label === '复制邮箱地址'));
  assert.ok(entries.some((e) => e.label === '写邮件'));
  assert.equal(personMenuEntries('', ''), null, '没有地址时不弹菜单');
});
