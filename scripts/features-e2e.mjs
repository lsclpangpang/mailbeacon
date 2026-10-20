// 在真实 Chrome 里端到端检查收信规则、通知设置和会话视图（用 ui/mock.js 的假数据，不连邮箱）。
// 用法：npm run test:e2e   （需要本机安装 google-chrome）
import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const UI = process.argv[2] || resolve(here, "../ui");
const port = 9334;
const chrome = spawn("google-chrome", [
  "--headless=new", "--no-sandbox", "--disable-gpu", `--remote-debugging-port=${port}`,
  `--user-data-dir=${mkdtempSync(join(tmpdir(), "mb-chrome-"))}`, "--window-size=1280,900", "about:blank",
], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let target;
for (let i = 0; i < 50 && !target; i++) {
  await sleep(200);
  try { target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT" })).json(); } catch {}
}
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => { const msg = JSON.parse(m.data); if (pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); } };
const send = (method, params = {}) => new Promise((r) => { const i = ++id; pending.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const run = async (expr) => {
  const res = await send("Runtime.evaluate", { expression: `(async () => { const sleep = (ms) => new Promise(r => setTimeout(r, ms)); const inv = window.__TAURI__.core.invoke; ${expr} })()`, awaitPromise: true, returnByValue: true });
  if (res.result.exceptionDetails) throw new Error(JSON.stringify(res.result.exceptionDetails, null, 1));
  return res.result.result.value;
};
const results = [];
const check = (name, ok, detail = "") => { results.push(ok); console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? " — " + detail : ""}`); };
const fresh = async () => { await send("Page.navigate", { url: `file://${UI}/index.html` }); await sleep(1200); };

await send("Page.enable");
try {
  // 新建规则：换账号后文件夹跟着换，勾「移到文件夹」默认选用户自己的文件夹
  await fresh();
  const created = await run(`
    document.querySelector('[data-action="rules"]').click(); await sleep(200);
    document.querySelector('[data-action="rule-new"]').click(); await sleep(200);
    const form = () => document.querySelector('.rule-form');
    const focus = document.activeElement?.id;
    form().querySelector('[name=name]').value = '公司通知';
    const acc = form().querySelector('[name=accountId]'); acc.value = 'a2'; acc.dispatchEvent(new Event('change', { bubbles: true })); await sleep(100);
    form().querySelector('[name=value]').value = 'company.com';
    form().querySelector('[data-action="rule-cond-add"]').click(); await sleep(50);
    const f2 = form().querySelectorAll('.rule-cond')[1].querySelector('[name=field]'); f2.value = 'attachment'; f2.dispatchEvent(new Event('change', { bubbles: true })); await sleep(50);
    const attachValue = !!form().querySelectorAll('.rule-cond')[1].querySelector('[name=value]');
    const nameKept = form().querySelector('[name=name]').value;
    form().querySelector('[type=submit]').click(); await sleep(100);
    const problem = form().querySelector('.rule-problem').textContent;
    form().querySelector('[name=move]').click(); await sleep(100);
    const target = form().querySelector('[name=moveTo]').value;
    const offered = [...form().querySelectorAll('[name=moveTo] option')].map(o => o.value);
    form().querySelector('[type=submit]').click(); await sleep(200);
    return { focus, attachValue, nameKept, problem, target, offered, saved: await inv('get_rules'), rows: document.querySelectorAll('.rule-row').length };
  `);
  check("new rule focuses the name field", created.focus === "rule-name", created.focus);
  check("attachment condition has no text box and editing keeps typed name", !created.attachValue && created.nameKept === "公司通知");
  check("saving without an action explains why", created.problem === "请至少选择一个动作", created.problem);
  check("move target defaults to the user's folder and skips drafts/sent", created.target === "&UXZO1mWHTvZZOQ-" && !created.offered.includes("Drafts") && !created.offered.includes("Sent"), JSON.stringify(created.offered));
  check("rule is saved with both conditions", created.rows === 1 && created.saved[0]?.conditions.length === 2 && created.saved[0].actions.moveTo === "&UXZO1mWHTvZZOQ-", JSON.stringify(created.saved));

  // 先勾「移到文件夹」再换账号：动作不能悄悄丢掉，目标换成新账号的文件夹
  await fresh();
  const switched = await run(`
    document.querySelector('[data-action="rules"]').click(); await sleep(200);
    document.querySelector('[data-action="rule-new"]').click(); await sleep(200);
    const form = () => document.querySelector('.rule-form');
    form().querySelector('[name=value]').value = 'bank';
    form().querySelector('[name=move]').click(); await sleep(100);
    form().querySelector('[name=markRead]').click();
    const acc = form().querySelector('[name=accountId]'); acc.value = 'a2'; acc.dispatchEvent(new Event('change', { bubbles: true })); await sleep(150);
    const state = { move: form().querySelector('[name=move]').checked, target: form().querySelector('[name=moveTo]').value, markRead: form().querySelector('[name=markRead]').checked };
    form().querySelector('[type=submit]').click(); await sleep(200);
    return { ...state, saved: (await inv('get_rules'))[0]?.actions };
  `);
  check("switching account keeps the move action and picks the new account's folder", switched.move && switched.target === "&UXZO1mWHTvZZOQ-" && switched.markRead && switched.saved?.moveTo === "&UXZO1mWHTvZZOQ-" && switched.saved?.markRead, JSON.stringify(switched));

  // 键盘：打开规则窗口焦点进去；排序后焦点留在同一行
  await fresh();
  const keyboard = await run(`
    await inv('save_rules', { rules: ['甲', '乙'].map((name) => ({ id: '', name, enabled: true, accountId: 'a1', matchAll: true, conditions: [{ field: 'subject', op: 'contains', value: name }], actions: { moveTo: null, markRead: true, flag: false } })) });
    document.querySelector('[data-action="rules"]').click(); await sleep(300);
    const opened = !!document.activeElement?.closest('.rules-sheet');
    const up = document.querySelector('[data-rule-index="1"] [data-action="rule-up"]'); up.focus(); up.click(); await sleep(250);
    const a = document.activeElement;
    return { opened, focusRow: a?.closest('[data-rule-index]')?.dataset.ruleIndex, focusAction: a?.dataset.action || (a?.matches('[data-rule-toggle]') ? 'toggle' : a?.tagName) };
  `);
  check("rules dialog takes focus and reordering keeps focus on the moved rule", keyboard.opened && keyboard.focusRow === "0" && keyboard.focusAction === "rule-down", JSON.stringify(keyboard));

  // 文件夹读不出来时给出错误和重试，而不是一直「正在读取」
  await fresh();
  const folderFail = await run(`
    window.__mockFail = { list_folders: '连接失败' };
    delete S.folders.a2;
    document.querySelector('[data-action="rules"]').click(); await sleep(200);
    document.querySelector('[data-action="rule-new"]').click(); await sleep(100);
    const acc = document.querySelector('.rule-form [name=accountId]'); acc.value = 'a2'; acc.dispatchEvent(new Event('change', { bubbles: true })); await sleep(200);
    const shown = document.querySelector('.rule-form .rule-folder-error')?.textContent || '';
    window.__mockFail = {};
    document.querySelector('.rule-form [data-action="rule-folders-retry"]')?.click(); await sleep(200);
    return { shown, after: [...document.querySelectorAll('.rule-form [name=moveTo] option')].map(o => o.value) };
  `);
  check("folder load failure in the rule editor offers a retry", /读取文件夹失败/.test(folderFail.shown) && folderFail.after.includes("Archive"), JSON.stringify(folderFail));

  // 右键邮件「按此发件人新建规则」
  await fresh();
  const fromMail = await run(`
    const row = [...document.querySelectorAll('#list [data-open]')].find(r => /GitHub/.test(r.textContent));
    row.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, clientX: 300, clientY: 200 })); await sleep(150);
    [...document.querySelectorAll('.menu [role=menuitem]')].find(b => /新建规则/.test(b.textContent))?.click(); await sleep(300);
    const f = document.querySelector('.rule-form');
    return { name: f?.querySelector('[name=name]').value, op: f?.querySelector('[name=op]').value, value: f?.querySelector('[name=value]').value };
  `);
  check("rule from a message prefills the exact sender", fromMail.op === "equals" && fromMail.value === "noreply@github.com" && /GitHub/.test(fromMail.name), JSON.stringify(fromMail));

  // 立即执行：命中的邮件移走，列表刷新
  await fresh();
  const ran = await run(`
    await inv('save_rules', { rules: [{ id: '', name: '证券', enabled: true, accountId: 'a1', matchAll: true, conditions: [{ field: 'from', op: 'contains', value: 'longbridge' }], actions: { moveTo: 'Junk', markRead: true, flag: false } }] });
    const shown = () => document.querySelector('#list').textContent.includes('长桥证券');
    const before = shown();
    document.querySelector('[data-action="rules"]').click(); await sleep(200);
    document.querySelector('#rules-run-account').value = 'a1';
    document.querySelector('[data-action="rules-run"]').click(); await sleep(500);
    return { before, after: shown(), toast: document.querySelector('#toast')?.textContent || '' };
  `);
  check("running rules now moves matching inbox mail and refreshes the list", ran.before && !ran.after && /已处理 1 封/.test(ran.toast), JSON.stringify(ran));

  // 开关、排序、删除都会保存
  await fresh();
  const list = await run(`
    await inv('save_rules', { rules: ['甲', '乙'].map((name) => ({ id: '', name, enabled: true, accountId: 'a1', matchAll: true, conditions: [{ field: 'subject', op: 'contains', value: name }], actions: { moveTo: null, markRead: true, flag: false } })) });
    document.querySelector('[data-action="rules"]').click(); await sleep(300);
    document.querySelector('[data-rule-index="1"] [data-action="rule-up"]').click(); await sleep(200);
    const order = (await inv('get_rules')).map(r => r.name);
    const toggle = document.querySelector('[data-rule-index="0"] [data-rule-toggle]'); toggle.click(); await sleep(200);
    const enabled = (await inv('get_rules'))[0].enabled;
    window.confirm = () => true;
    document.querySelector('[data-rule-index="0"] [data-action="rule-delete"]').click(); await sleep(200);
    return { order, enabled, left: (await inv('get_rules')).map(r => r.name), rows: document.querySelectorAll('.rule-row').length };
  `);
  check("reorder, enable toggle and delete are saved", list.order.join() === "乙,甲" && list.enabled === false && list.left.join() === "甲" && list.rows === 1, JSON.stringify(list));

  // 通知设置：连续改两处都能保存
  await fresh();
  const prefs = await run(`
    document.querySelector('[data-action="settings"]').click(); await sleep(300);
    const sec = document.querySelector('[data-notify-account="a2"]');
    sec.querySelector('[name=notifyFolder]').click();
    document.querySelector('.prefs-form [name=tray]').click();
    await sleep(300);
    const p = await inv('get_prefs');
    return { tray: p.tray, folders: p.notifyFolders };
  `);
  check("quick changes in two settings sections are both saved", prefs.tray === false && prefs.folders.a2?.length === 1, JSON.stringify(prefs));

  // 会话视图：列表一行一个会话；阅读区列出整个会话，包括已发送里自己的回复
  await fresh();
  const conv = await run(`
    document.querySelector('[data-action="list-menu"]').click(); await sleep(150);
    [...document.querySelectorAll('.menu [role=menuitem]')].find(b => /按会话显示/.test(b.textContent)).click(); await sleep(200);
    const rows = [...document.querySelectorAll('#list .row')];
    const row = rows.find(r => /路线图/.test(r.textContent));
    const count = row?.querySelector('.thread-count')?.textContent;
    row.click(); await sleep(500);
    const strip = [...document.querySelectorAll('.thread-item')].map(b => b.textContent.replace(/\\s+/g, ' ').trim());
    [...document.querySelectorAll('.thread-item')].find(b => /已发送/.test(b.textContent)).click(); await sleep(400);
    const current = document.querySelector('.thread-item[aria-current]')?.textContent || '';
    const stillSelected = !!document.querySelector('#list .row.sel');
    // J：下一个会话
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'j', bubbles: true })); await sleep(300);
    const next = document.querySelector('#list .row.sel')?.textContent.replace(/\\s+/g, ' ').trim() || '';
    return { rows: rows.length, count, strip, current, stillSelected, next, remembered: localStorage.getItem('mb.threads') };
  `);
  check("conversation mode groups the inbox and shows the conversation size", conv.count === "2" && conv.rows === 12, JSON.stringify({ rows: conv.rows, count: conv.count }));
  check("the reader lists the conversation including my reply from Sent", conv.strip.length === 3 && /已发送/.test(conv.strip[1]), JSON.stringify(conv.strip));
  check("opening my reply keeps the conversation row selected", /已发送|我/.test(conv.current) && conv.stillSelected, conv.current);
  check("J moves to the next conversation and the choice is remembered", /GitHub/.test(conv.next) && conv.remembered === "1", conv.next);

  // 模板：把写好的邮件存成模板，新邮件里一键插入（签名只留一份）
  await fresh();
  const tpl = await run(`
    const compose = async () => { document.querySelector('[data-action="compose"]').click(); await sleep(300); };
    await compose();
    document.querySelector('#c-subject').value = '报价回复';
    const body = document.querySelector('#c-body');
    body.insertAdjacentHTML('afterbegin', '<div>您好，报价如下：</div>');
    document.querySelector('[data-action="compose-templates"]').click(); await sleep(150);
    document.querySelector('#c-template-name').value = '报价';
    document.querySelector('[data-action="template-save"]').click(); await sleep(200);
    const saved = await inv('templates_list');
    window.confirm = () => true;
    document.querySelector('[data-action="compose-close"]').click(); await sleep(200);
    document.querySelector('[data-action="compose-discard"]')?.click(); await sleep(200);
    await compose();
    document.querySelector('[data-action="compose-templates"]').click(); await sleep(200);
    document.querySelector('[data-template-id]').click(); await sleep(150);
    const html = document.querySelector('#c-body').innerHTML;
    return { saved, subject: document.querySelector('#c-subject').value, html, signatures: (html.match(/data-mp-signature/g) || []).length };
  `);
  check("a template is saved without the signature", tpl.saved.length === 1 && /报价如下/.test(tpl.saved[0].html) && !/mp-signature/.test(tpl.saved[0].html) && tpl.saved[0].subject === "报价回复", JSON.stringify(tpl.saved));
  check("inserting a template fills subject and body and keeps one signature", tpl.subject === "报价回复" && /报价如下/.test(tpl.html) && tpl.signatures === 1, JSON.stringify({ subject: tpl.subject, signatures: tpl.signatures }));

  // 键盘：模板名称框、定时时间框里按回车
  await fresh();
  const keys = await run(`
    const enter = (el) => el.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    document.querySelector('[data-action="compose"]').click(); await sleep(300);
    document.querySelector('#c-to').value = 'boss@x.com';
    document.querySelector('#c-subject').value = '键盘';
    document.querySelector('#c-body').insertAdjacentHTML('afterbegin', '<div>正文</div>');
    document.querySelector('[data-action="compose-templates"]').click(); await sleep(150);
    const name = document.querySelector('#c-template-name'); name.value = '键盘模板'; enter(name); await sleep(250);
    const saved = (await inv('templates_list')).length;
    document.querySelector('[data-action="compose-schedule"]').click(); await sleep(150);
    const at = document.querySelector('#c-send-at');
    const d = new Date(Date.now() + 2 * 3600e3), pad = (n) => String(n).padStart(2, '0');
    at.value = d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + 'T' + pad(d.getHours()) + ':' + pad(d.getMinutes());
    enter(at); await sleep(400);
    return { saved, scheduled: (await inv('outbox_list', {})).length, closed: !document.querySelector('.compose-sheet') };
  `);
  check("Enter saves a template and confirms a schedule", keys.saved === 1 && keys.scheduled === 1 && keys.closed, JSON.stringify(keys));

  // 无主题的定时邮件也要确认，确认后按原来选的时间定时
  await fresh();
  const noSubject = await run(`
    document.querySelector('[data-action="compose"]').click(); await sleep(300);
    document.querySelector('#c-to').value = 'boss@x.com';
    document.querySelector('[data-action="compose-schedule"]').click(); await sleep(150);
    document.querySelector('[data-send-at]').click(); await sleep(200);
    const asked = !document.querySelector('.compose-confirm').hidden;
    const want = +document.querySelector('[data-send-at]')?.dataset.sendAt || null;
    document.querySelector('[data-action="compose-send-confirm"]').click(); await sleep(400);
    const list = await inv('outbox_list', {});
    return { asked, count: list.length, sendAt: list[0]?.sendAt, sent: !!window.__lastSent };
  `);
  // 确认框还开着时按 ⌘↩：不能把它变成立即发送
  await fresh();
  const cmdEnter = await run(`
    document.querySelector('[data-action="compose"]').click(); await sleep(300);
    document.querySelector('#c-to').value = 'boss@x.com';
    document.querySelector('[data-action="compose-schedule"]').click(); await sleep(150);
    document.querySelector('[data-send-at]').click(); await sleep(200);
    const message = document.querySelector('.compose-confirm-message').textContent;
    document.querySelector('#c-to').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', metaKey: true, bubbles: true })); await sleep(200);
    document.querySelector('[data-action="compose-send-confirm"]').click(); await sleep(400);
    return { message, scheduled: (await inv('outbox_list', {})).length, sent: !!window.__lastSent };
  `);
  check("cmd+Enter during the no-subject prompt still schedules instead of sending now", cmdEnter.scheduled === 1 && !cmdEnter.sent && /定时/.test(cmdEnter.message), JSON.stringify(cmdEnter));
  check("a scheduled mail without subject asks first and keeps the chosen time", noSubject.asked && noSubject.count === 1 && !noSubject.sent, JSON.stringify(noSubject));

  // 请求已读回执
  await fresh();
  const asked = await run(`
    document.querySelector('[data-action="compose"]').click(); await sleep(300);
    document.querySelector('#c-to').value = 'boss@x.com';
    document.querySelector('#c-subject').value = '周报';
    document.querySelector('#c-receipt').click();
    document.querySelector('[data-action="send"]').click(); await sleep(900);
    return window.__lastSent?.requestReceipt;
  `);
  check("asking for a read receipt is sent with the mail", asked === true, String(asked));

  // 定时发送：存进「定时发送」，可以立即发送或取消（转成草稿）
  await fresh();
  const sched = await run(`
    window.confirm = () => true;
    const schedule = async (subject) => {
      document.querySelector('[data-action="compose"]').click(); await sleep(300);
      document.querySelector('#c-to').value = 'boss@x.com';
      document.querySelector('#c-subject').value = subject;
      document.querySelector('[data-action="compose-schedule"]').click(); await sleep(150);
      const input = document.querySelector('#c-send-at');
      const quick = [...document.querySelectorAll('[data-send-at]')].map(b => b.textContent);
      const d = new Date(Date.now() + 3 * 3600e3);
      const pad = (n) => String(n).padStart(2, '0');
      input.value = d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) + 'T' + pad(d.getHours()) + ':' + pad(d.getMinutes());
      document.querySelector('[data-action="schedule-ok"]').click(); await sleep(400);
      return { quick, closed: !document.querySelector('.compose-sheet'), toast: document.querySelector('#toast')?.textContent || '' };
    };
    const first = await schedule('定时一');
    await sleep(300);
    const folder = document.querySelector('#sidebar [data-go="__outbox"]');
    folder?.click(); await sleep(400);
    const rows = [...document.querySelectorAll('#list .row')].map(r => r.textContent.replace(/\\s+/g, ' ').trim());
    document.querySelector('#list .row')?.click(); await sleep(400);
    const banner = document.querySelector('.outbox-bar')?.textContent || '';
    const hasReply = !!document.querySelector('#reader [data-action="reply"]');
    document.querySelector('[data-action="outbox-send-now"]').click(); await sleep(500);
    const afterSend = await inv('outbox_list', {});
    await schedule('定时二');
    await sleep(300);
    document.querySelector('#sidebar [data-go="__outbox"]')?.click(); await sleep(400);
    document.querySelector('#list .row')?.click(); await sleep(400);
    document.querySelector('[data-action="outbox-cancel"]').click(); await sleep(500);
    const drafts = await inv('list_messages', { accountId: 'a1', folder: 'Drafts' });
    const stillThere = !!document.querySelector('#sidebar [data-go="__outbox"]');
    return { first, rows, banner, hasReply, afterSend: afterSend.length, cancelledToDraft: drafts.some(d => d.subject === '定时二'), stillThere };
  `);
  check("scheduling closes the compose window and says when it will go", sched.first.closed && /将于/.test(sched.first.toast), JSON.stringify(sched.first));
  check("scheduled mail shows in its own folder with the send time", sched.rows.length === 1 && /定时一/.test(sched.rows[0]) && /将于/.test(sched.banner) && !sched.hasReply, JSON.stringify({ rows: sched.rows, banner: sched.banner }));
  check("send now and cancel-to-draft work and the folder goes away when empty", sched.afterSend === 0 && sched.cancelledToDraft && !sched.stillThere, JSON.stringify(sched));

  // 已读回执：对方请求时询问，绝不自动发送
  await fresh();
  const rcpt = await run(`
    const open = async () => { [...document.querySelectorAll('#list [data-open]')].find(r => /陈静/.test(r.textContent)).click(); await sleep(400); };
    await open();
    const bar = document.querySelector('.receipt-bar')?.textContent || '';
    const autoSent = (window.__receipts || []).length;
    document.querySelector('[data-action="receipt-send"]').click(); await sleep(300);
    const after = !!document.querySelector('.receipt-bar');
    [...document.querySelectorAll('#list [data-open]')].find(r => /GitHub/.test(r.textContent)).click(); await sleep(300);
    await open();
    return { bar, autoSent, after, again: !!document.querySelector('.receipt-bar'), receipts: window.__receipts };
  `);
  check("a requested read receipt is offered, never sent automatically", /回执/.test(rcpt.bar) && rcpt.autoSent === 0, JSON.stringify(rcpt));
  check("after answering, the receipt prompt doesn't come back", !rcpt.after && !rcpt.again && rcpt.receipts?.join() === "sent", JSON.stringify(rcpt));
} finally {
  chrome.kill();
}
const passed = results.filter(Boolean).length;
console.log(`${passed}/${results.length} passed`);
process.exit(passed === results.length ? 0 : 1);
