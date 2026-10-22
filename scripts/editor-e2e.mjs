// 在真实 Chrome 里端到端检查富文本写信（用 ui/mock.js 的假数据，不连邮箱）。
// 用法：npm run test:e2e   （需要本机安装 google-chrome；不依赖 npm 包，直接走 DevTools 协议）
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
const here = dirname(fileURLToPath(import.meta.url));
const UI = process.argv[2] || resolve(here, "../ui");
const OUT = process.argv[3] || join(tmpdir(), "mailbeacon-compose.png");
const port = 9333;
const profile = mkdtempSync(join(tmpdir(), "mb-chrome-"));
const chrome = spawn("google-chrome", [
  "--headless=new", "--no-sandbox", "--disable-gpu", `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`, "--window-size=1280,900", "about:blank",
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
  const res = await send("Runtime.evaluate", { expression: `(async () => { ${expr} })()`, awaitPromise: true, returnByValue: true });
  if (res.result.exceptionDetails) throw new Error(JSON.stringify(res.result.exceptionDetails, null, 1));
  return res.result.result.value;
};

// 页面脚本运行前包装 invoke（app.js 启动时就解构拿走了它）
await send("Page.enable");
await send("Page.addScriptToEvaluateOnNewDocument", { source: `
  window.__sent = [];
  let tauri;
  Object.defineProperty(window, "__TAURI__", { configurable: true, get: () => tauri, set: (v) => {
    const orig = v.core.invoke;
    v.core.invoke = async (cmd, args) => { if (cmd === "send_mail") window.__sent.push(structuredClone(args)); return orig(cmd, args); };
    tauri = v;
  } });
` });
await send("Page.navigate", { url: `file://${UI}/index.html` });
const results = [];
const check = (name, ok, detail = "") => { results.push({ name, ok, detail }); console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? " — " + detail : ""}`); };

try {
  await sleep(1500);
  await run(`
    document.querySelector('[data-action="compose"]').click();
    await new Promise(r => setTimeout(r, 300));
  `);
  const setup = await run(`
    const ed = document.querySelector('#c-body');
    return { editable: ed.isContentEditable, toolbar: !!document.querySelector('.compose-toolbar'), html: ed.innerHTML };
  `);
  check("editor is contenteditable with toolbar", setup.editable && setup.toolbar, setup.html.slice(0, 120));
  check("signature block inserted", /mp-signature/.test(setup.html), setup.html.slice(0, 160));

  // 还没点过编辑器就选字体：光标放进编辑器，样式作用于接下来的输入
  const early = await run(`
    const f = document.querySelector('[data-fmt="font"]');
    f.focus(); f.value = f.options[2].value;
    f.dispatchEvent(new Event('change', { bubbles: true }));
    const focusedEditor = document.activeElement === document.querySelector('#c-body');
    document.execCommand('insertText', false, '宋');
    const html = document.querySelector('#c-body').innerHTML;
    document.execCommand('undo');
    return { focusedEditor, html };
  `);
  check("choosing a font before focusing the editor styles the next typing", early.focusedEditor && /font-family[^"]*SimSun|face="SimSun/.test(early.html), JSON.stringify(early).slice(0, 220));
  await run(`document.querySelector('#c-body').innerHTML = '<div><br></div><div><br></div>' + (document.querySelector('#c-body .mp-signature')?.outerHTML || '');`);

  // 切换发件人：只替换签名节点，已输入的文字节点不被重建（光标、撤销记录都在）
  const switchCheck = await run(`
    const ed = document.querySelector('#c-body');
    ed.focus();
    const r = document.createRange(); r.setStart(ed.firstChild, 0); r.collapse(true);
    getSelection().removeAllRanges(); getSelection().addRange(r);
    document.execCommand('insertText', false, 'hello');
    const typed = [...ed.querySelectorAll('div')].map(d => d.firstChild).find(n => n && n.nodeType === 3 && n.data === 'hello');
    const from = document.querySelector('#c-from');
    const before = ed.querySelector('.mp-signature').textContent;
    from.value = from.options[from.selectedIndex === 0 ? 1 : 0].value;
    from.dispatchEvent(new Event('change', { bubbles: true }));
    const after = ed.querySelector('.mp-signature')?.textContent || '(none)';
    const still = typed && typed.isConnected;
    document.execCommand('undo');
    return { before, after, still };
  `);
  check("sender switch swaps the untouched signature", switchCheck.before !== switchCheck.after && switchCheck.after !== "(none)", `${switchCheck.before} → ${switchCheck.after}`);
  check("sender switch keeps the typed text nodes (caret/undo survive)", switchCheck.still === true);
  await run(`document.querySelector('#c-body').innerHTML = '<div><br></div>';`);

  // Type, select a word, bold it via the toolbar (mousedown is prevented so focus stays)
  const bold = await run(`
    const ed = document.querySelector('#c-body');
    ed.focus();
    const first = ed.firstChild; const r = document.createRange(); r.setStart(first, 0); r.collapse(true);
    getSelection().removeAllRanges(); getSelection().addRange(r);
    document.execCommand('insertText', false, '你好 world');
    const text = [...ed.querySelectorAll('*')].map(n => n.firstChild).find(n => n && n.nodeType === 3 && n.data.includes('world'));
    const sel = document.createRange(); sel.setStart(text, text.data.indexOf('world')); sel.setEnd(text, text.data.indexOf('world') + 5);
    getSelection().removeAllRanges(); getSelection().addRange(sel);
    const btn = document.querySelector('[data-action="fmt"][data-cmd="bold"]');
    btn.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    btn.click();
    return ed.innerHTML;
  `);
  check("bold applies to the selected text", /font-weight:\s*bold|<b>world<\/b>/.test(bold), bold.slice(0, 160));

  const size = await run(`
    const s = document.querySelector('[data-fmt="size"]'); s.value = 'large';
    s.dispatchEvent(new Event('change', { bubbles: true }));
    return { html: document.querySelector('#c-body').innerHTML, reset: s.value };
  `);
  check("font size applies and the picker keeps its value", /font-size:\s*x-large|size="5"/.test(size.html) && size.reset === "large", size.html.slice(0, 200));

  const color = await run(`
    const c = document.querySelector('[data-fmt="color"]'); c.value = '#c00000';
    c.dispatchEvent(new Event('change', { bubbles: true }));
    return document.querySelector('#c-body').innerHTML;
  `);
  check("text color applies", /color:\s*rgb\(192, 0, 0\)|#c00000/i.test(color), color.slice(0, 200));

  const link = await run(`
    document.querySelector('[data-action="fmt"][data-cmd="link"]').click();
    await new Promise(r => setTimeout(r, 50));
    const input = document.querySelector('#c-link');
    if (!input) return 'no input';
    input.value = 'example.com/report';
    document.querySelector('[data-action="popover-ok"]').click();
    return document.querySelector('#c-body').innerHTML;
  `);
  check("link wraps the selection after the popover steals focus", /<a href="https:\/\/example.com\/report">/.test(link), link.slice(0, 220));

  const table = await run(`
    const ed = document.querySelector('#c-body');
    const r = document.createRange(); r.selectNodeContents(ed); r.collapse(false);
    getSelection().removeAllRanges(); getSelection().addRange(r);
    document.querySelector('[data-action="fmt"][data-cmd="table"]').click();
    await new Promise(r => setTimeout(r, 50));
    document.querySelector('#c-rows').value = '2'; document.querySelector('#c-cols').value = '2';
    document.querySelector('[data-action="popover-ok"]').click();
    return ed.querySelectorAll('table tr').length + '/' + ed.querySelectorAll('table td').length;
  `);
  check("table inserted at the caret", table === "2/4", table);

  const image = await run(`
    document.querySelector('[data-action="fmt"][data-cmd="image"]').click();
    await new Promise(r => setTimeout(r, 200));
    return document.querySelectorAll('#c-body img[src^="data:image/png"]').length;
  `);
  check("image inserted from picker", image === 1, String(image));

  const paste = await run(`
    const ed = document.querySelector('#c-body');
    ed.focus();
    const dt = new DataTransfer();
    dt.setData('text/html', '<p onclick="evil()">粘贴<b>内容</b><script>bad()<\/script></p>');
    dt.setData('text/plain', '粘贴内容');
    ed.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    await new Promise(r => setTimeout(r, 200));
    return ed.innerHTML;
  `);
  check("pasted HTML is cleaned (mock cleaner) and inserted without style noise", /粘贴<b>内容<\/b>/.test(paste) && !/onclick|script|initial/.test(paste), paste.slice(-200));


  // 自绘字号下拉：真实键盘展开、移动后回车提交，期间保留编辑器选区。
  await run(`
    const ed = document.querySelector('#c-body');
    const word = [...ed.querySelectorAll('*')].map(n => n.firstChild).find(n => n && n.nodeType === 3 && n.data.includes('内容'));
    ed.focus();
    const r = document.createRange(); r.selectNodeContents(word);
    getSelection().removeAllRanges(); getSelection().addRange(r);
    rememberSelection();
    const s = document.querySelector('[data-fmt="size"]');
    s.value = ''; window.MailBeaconDropdown.refresh(s); s.nextElementSibling.focus();
  `);
  await sleep(50); // 选区变化事件是异步的；真实操作中用户选完文字再按方向键远不止这点时间
  for (const [key, keyCode] of [["Enter", 13], ["Home", 36], ["ArrowDown", 40], ["ArrowDown", 40], ["Enter", 13]]) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code: key, windowsVirtualKeyCode: keyCode, ...(key === 'Enter' ? { text: '\r', unmodifiedText: '\r' } : {}) });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode: keyCode });
    await sleep(100);
  }
  const arrows = await run(`
    const s = document.querySelector('[data-fmt="size"]');
    const word = [...document.querySelector('#c-body').querySelectorAll('*')].map(n => n.firstChild).find(n => n && n.nodeType === 3 && n.data.includes('内容'));
    return { focused: document.activeElement === s.nextElementSibling, value: s.value, closed: !document.querySelector('.select-popover'), size: getComputedStyle(word.parentElement).fontSize, html: document.querySelector('#c-body').innerHTML.slice(-160) };
  `);
  check("arrow keys and Enter commit the size while keeping trigger focus and text selection", arrows.focused && arrows.value === "normal" && arrows.closed && parseFloat(arrows.size) === 16, JSON.stringify(arrows));

  const esc = await run(`
    document.querySelector('[data-action="fmt"][data-cmd="link"]').click();
    await new Promise(r => setTimeout(r, 50));
    const input = document.querySelector('#c-link');
    input.focus();
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    await new Promise(r => setTimeout(r, 50));
    return { compose: !!document.querySelector('#c-body'), popover: document.querySelector('.compose-popover').hidden };
  `);
  check("Escape in the link field closes only the field", esc.compose && esc.popover, JSON.stringify(esc));

  const blurred = await run(`
    const s = document.querySelector('[data-fmt="size"]');
    document.querySelector('#c-body').focus();
    return { value: s.value, label: s.nextElementSibling.querySelector('.select-title').textContent, editorFocused: document.activeElement === document.querySelector('#c-body') };
  `);
  check("size picker and trigger label reset after losing focus", blurred.value === "" && blurred.label === "字号" && blurred.editorFocused, JSON.stringify(blurred));

  // 拖入的内容插到放下的位置
  const atPoint = await run(`
    const ed = document.querySelector('#c-body');
    const saved = ed.innerHTML;
    ed.innerHTML = '<div>START middle END</div>';
    const text = ed.firstChild.firstChild;
    const caret = document.createRange(); caret.setStart(text, text.data.length); caret.collapse(true);
    getSelection().removeAllRanges(); getSelection().addRange(caret);
    await new Promise(r => setTimeout(r, 30));
    const target = document.createRange(); target.setStart(text, 1); target.setEnd(text, 2);
    const box = target.getBoundingClientRect();
    const dt = new DataTransfer(); dt.setData('text/plain', 'DROP');
    ed.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true, clientX: box.left + 1, clientY: box.top + box.height / 2 }));
    await new Promise(r => setTimeout(r, 100));
    const result = ed.textContent;
    ed.innerHTML = saved;
    return result;
  `);
  check("dropped content lands where it is dropped", /^S.?DROP/.test(atPoint) && atPoint.endsWith("END"), atPoint);

  const dropped = await run(`
    const ed = document.querySelector('#c-body');
    const dt = new DataTransfer();
    dt.setData('text/html', '<p onclick="evil()">拖入<script>bad()<\/script></p>');
    ed.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
    await new Promise(r => setTimeout(r, 200));
    return ed.innerHTML;
  `);
  check("dropped HTML goes through the cleaner", /拖入/.test(dropped) && !/onclick|script/.test(dropped), dropped.slice(-120));

  await send("Page.captureScreenshot", { format: "png" }).then((r) => writeFileSync(OUT, Buffer.from(r.result.data, "base64")));

  // 收件人联想：输入后出现建议，回车选中第一项
  await run(`
    const to = document.querySelector('#c-to');
    to.focus(); to.value = '张'; to.setSelectionRange(1, 1);
    to.dispatchEvent(new Event('input', { bubbles: true }));
  `);
  await sleep(400);
  const listShown = await run(`return document.querySelector('.recipient-suggest')?.hidden === false && document.querySelector('.recipient-suggest').textContent`);
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 });
  await sleep(100);
  const picked = await run(`return { to: document.querySelector('#c-to').value, open: !!document.querySelector('#c-body') }`);
  check("typing a name suggests contacts", typeof listShown === "string" && listShown.includes("张磊"), String(listShown).slice(0, 80));
  check("Enter inserts the suggestion without sending", picked.open && picked.to === "张磊 <zhanglei@company.com>, ", JSON.stringify(picked));

  const groupPick = await run(`
    document.querySelector('[data-action="show-cc"]')?.click();
    const cc = document.querySelector('#c-cc');
    cc.focus(); cc.value = '项目'; cc.setSelectionRange(2, 2);
    cc.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise(r => setTimeout(r, 400));
    document.querySelector('.recipient-suggest [data-action="suggest-pick"]').dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    document.querySelector('.recipient-suggest [data-action="suggest-pick"]').click();
    return cc.value;
  `);
  check("choosing a group adds all members", groupPick === "张磊 <zhanglei@company.com>, 王芳 <wangfang@company.com>, ", groupPick);

  const picker = await run(`
    document.querySelector('[data-action="pick-contacts"]').click();
    await new Promise(r => setTimeout(r, 200));
    const rows = document.querySelectorAll('.contact-picker .picker-row').length;
    document.querySelector('.contact-picker [data-pick-address*="wangfang"]').click();
    document.querySelector('.contact-picker [data-action="picker-add"][data-target="bcc"]').click();
    return { rows, bcc: document.querySelector('#c-bcc').value, closed: document.querySelector('.contact-picker').hidden };
  `);
  check("contact picker adds checked contacts", picker.rows === 3 && picker.bcc === "王芳 <wangfang@company.com>" && picker.closed, JSON.stringify(picker));

  const sent = await run(`
    document.querySelector('#c-to').value = 'reader@example.com';
    document.querySelector('#c-subject').value = '富文本测试';
    document.querySelector('[data-action="send"]').click();
    await new Promise(r => setTimeout(r, 900));
    return window.__sent[0]?.draft;
  `);
  check("send carries HTML with formatting", !!sent && /<a href="https:\/\/example.com\/report">/.test(sent.htmlBody) && /<table/.test(sent.htmlBody), (sent?.htmlBody || "").slice(0, 120));
  check("send carries readable plain text", !!sent && /你好/.test(sent.body) && /example.com\/report/.test(sent.body), JSON.stringify(sent?.body || "").slice(0, 160));

  // 聚合视图和筛选
  const views = await run(`
    await new Promise(r => setTimeout(r, 300));
    const go = (v) => document.querySelector('[data-go="' + v + '"]').click();
    go('__unread');
    await new Promise(r => setTimeout(r, 300));
    const unreadRows = [...document.querySelectorAll('.rows .row')];
    const allUnread = unreadRows.length > 0 && unreadRows.every(r => r.classList.contains('unread'));
    go('INBOX');
    await new Promise(r => setTimeout(r, 300));
    const total = document.querySelectorAll('.rows .row').length;
    document.querySelector('[data-action="filter"][data-filter="attachment"]').click();
    const withClip = document.querySelectorAll('.rows .row').length;
    const chip = document.querySelector('.filter-chip.on')?.textContent;
    document.querySelector('[data-action="filter"][data-filter="all"]').click();
    return { title: document.querySelector('.list-head h1').textContent, unread: unreadRows.length, allUnread, total, withClip, chip };
  `);
  check("unread view lists only unread mail", views.unread > 0 && views.allUnread, JSON.stringify(views));
  check("attachment filter narrows the list", views.withClip > 0 && views.withClip < views.total && views.chip === "附件", JSON.stringify(views));

  // 通讯录管理：新建联系人后出现在列表里
  const manager = await run(`
    await new Promise(r => setTimeout(r, 300));
    document.querySelector('[data-action="contacts"]').click();
    await new Promise(r => setTimeout(r, 200));
    const before = document.querySelectorAll('.contacts-list [data-contact-id]').length;
    document.querySelector('[data-action="contact-new"]').click();
    document.querySelector('#contact-name').value = '赵敏';
    document.querySelector('#contact-emails').value = 'zhaomin@example.com';
    document.querySelector('[data-action="contact-save"]').click();
    await new Promise(r => setTimeout(r, 300));
    return { before, after: document.querySelectorAll('.contacts-list [data-contact-id]').length, text: document.querySelector('.contacts-list').textContent };
  `);
  check("contacts manager creates a contact", manager.after === manager.before + 1 && manager.text.includes("赵敏"), JSON.stringify(manager).slice(0, 160));
  await send("Page.captureScreenshot", { format: "png" }).then((r) => writeFileSync(OUT.replace(/\.png$/, "-contacts.png"), Buffer.from(r.result.data, "base64")));
} catch (e) {
  check("script error", false, String(e).slice(0, 400));
} finally {
  ws.close();
  chrome.kill();
  const failed = results.filter((r) => !r.ok).length;
  console.log(`\n${results.length - failed}/${results.length} passed`);
  process.exit(failed ? 1 : 0);
}
