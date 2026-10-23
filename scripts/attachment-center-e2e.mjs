// Real Chrome checks for attachment discovery and refresh continuity.
// Uses synthetic attachment fixtures only. Run: node scripts/attachment-center-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, ATTACHMENT_CENTER_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.ATTACHMENT_CENTER_CDP_PORT || 9351);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const results = [];
let chrome, profile, socket, exited = false, spawnError, exitPromise;
let sequence = 0;
const pending = new Map();

function chromePath() {
  const candidates = [
    process.env.CHROME_BIN,
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    ...String(process.env.PATH || "").split(delimiter).flatMap((dir) =>
      ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map((name) => join(dir, name))),
  ];
  const found = candidates.find((path) => path && existsSync(path));
  if (!found) throw new Error("Chrome was not found; set CHROME_BIN to its executable path.");
  return found;
}

async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  await new Promise((resolve) => server.close(resolve));
}

function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++sequence;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timed out: ${method}`)); }, 15000);
    pending.set(id, { resolve, reject, timer });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function run(source) {
  const response = await send("Runtime.evaluate", {
    expression: `(async () => { ${source} })()`, awaitPromise: true, returnByValue: true,
  });
  if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails));
  return response.result?.value;
}

async function waitFor(expression, label, timeout = 5000) {
  const start = Date.now();
  while (Date.now() - start < timeout) {
    if (await run(`return !!(${expression});`)) return;
    await sleep(25);
  }
  throw new Error(`Timed out waiting for ${label}`);
}

function check(name, ok, detail = "") {
  results.push({ name, ok: !!ok });
  console.log(`${ok ? "PASS" : "FAIL"} ${name}${detail ? ` — ${typeof detail === "string" ? detail : JSON.stringify(detail)}` : ""}`);
}

async function cleanup() {
  for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error("Browser closed")); }
  pending.clear();
  socket?.close();
  if (chrome && !exited) {
    chrome.kill("SIGTERM");
    await Promise.race([exitPromise, sleep(3000)]);
    if (!exited) { chrome.kill("SIGKILL"); await Promise.race([exitPromise, sleep(2000)]); }
  }
  if (profile) rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}

try {
  await freePort(); // Never connect to or terminate somebody else's browser.
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-attachments-"));
  chrome = spawn(chromePath(), [
    "--headless=new", "--no-sandbox", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
    `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`, "--window-size=1280,900", "about:blank",
  ], { stdio: "ignore" });
  exitPromise = new Promise((resolve) => {
    chrome.once("exit", () => { exited = true; resolve(); });
    chrome.once("error", (error) => { spawnError = error; exited = true; resolve(); });
  });
  let target;
  for (let i = 0; i < 80 && !target; i++) {
    if (spawnError) throw spawnError;
    if (exited) throw new Error("Chrome exited before its debugging endpoint was ready.");
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT", signal: AbortSignal.timeout(500) });
      if (response.ok) target = await response.json();
    } catch {}
    if (!target) await sleep(100);
  }
  if (!target?.webSocketDebuggerUrl) throw new Error("Chrome debugging endpoint did not start.");
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  socket.onmessage = (event) => {
    const message = JSON.parse(event.data), item = pending.get(message.id);
    if (!item) return;
    clearTimeout(item.timer); pending.delete(message.id);
    if (message.error) item.reject(new Error(JSON.stringify(message.error)));
    else item.resolve(message.result);
  };
  await send("Page.enable");
  await send("Network.enable");
  await send("Network.setBlockedURLs", { urls: ["http://*", "https://*"] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "no-preference" }] });
  const fixture = join(profile, 'attachments.html');
  writeFileSync(fixture, `<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="${pathToFileURL(join(ui, 'style.css'))}"><link rel="stylesheet" href="${pathToFileURL(join(ui, 'productivity.css'))}"><script src="${pathToFileURL(join(ui, 'attachment-center.js'))}"></script><div id="list" style="position:absolute;inset:0;width:720px;height:900px;display:flex;flex-direction:column"></div>`);
  await send('Page.navigate', { url: pathToFileURL(fixture).href });
  await waitFor('window.MailBeaconAttachments', 'attachment module');
  await run(`
    window.__calls=[];window.__fail=false;
    window.__item=uid=>({source:{accountId:'a',receiveEpoch:1,folder:'INBOX',uidValidity:7,uid},partId:0,rawHash:'hash-'+uid,parserVersion:1,filename:'file-'+uid+'.pdf',sender:'sender@example.test',mime:'application/pdf',messageDate:uid,size:128});
    window.__rows=Array.from({length:350},(_,i)=>__item(i+1));
    window.__center=MailBeaconAttachments.mount({container:document.querySelector('#list'),targets:[{accountId:'a',receiveEpoch:1,folder:'INBOX'}],accounts:[{id:'a',email:'a@example.test',receiveProtocol:'imap'},{id:'pop',email:'pop@example.test',receiveProtocol:'pop3'}],formatDate:String,formatSize:n=>n+' B',accountName:String,
      invoke:async(cmd,{input})=>{__calls.push(structuredClone(input));if(__fail)throw Error('synthetic refresh failure');const start=Number(input.cursor||0),end=start+100;return {items:__rows.slice(start,end),coverage:[{indexedMessages:350,parsedMessages:350,attachmentCount:350}],hasMore:end<__rows.length,nextCursor:end<__rows.length?String(end):null,catalogIncomplete:[]};}});
  `);
  await waitFor('__center.items.length===100', 'first page');
  await click('[data-center-filter="query"]');
  await send('Input.insertText', { text: 'file' });
  await waitFor('__calls.at(-1).query==="file"', 'name filter');
  await click('[data-center-action="more"]');
  await waitFor('__center.items.length===200', 'second page');
  await click('[data-center-action="more"]');
  await waitFor('__center.items.length===300', 'third page');
  await click('[data-center-index="149"] [data-center-action="select"]');
  await key('ArrowDown', 40); await key('ArrowUp', 38);
  const before = await run(`return {scroll:document.querySelector('.attachment-items').scrollTop,calls:__calls.length,focused:document.activeElement.closest('[data-center-index]')?.dataset.centerIndex};`);
  check('trusted keyboard navigation returns to the selected attachment', before.focused === '149', before);
  await run(`__rows.unshift(__item(999));__center.scheduleRefresh();__center.scheduleRefresh();__center.scheduleRefresh();`);
  await waitFor('__center.items[0]?.source.uid===999', 'debounced index refresh');
  const after = await run(`return {count:__center.items.length,calls:__calls.length,query:document.querySelector('[data-center-filter="query"]').value,selected:document.querySelector('.attachment-item.selected')?.dataset.centerIndex,focused:document.activeElement.closest('[data-center-index]')?.dataset.centerIndex,scroll:document.querySelector('.attachment-items').scrollTop,popOption:!!document.querySelector('option[value="pop"]'),coverage:document.querySelector('.attachment-coverage').textContent};`);
  check('index refresh preserves three pages, filter, selected attachment, keyboard focus and scroll', after.count===300 && after.calls-before.calls===3 && after.query==='file' && after.selected==='150' && after.focused==='150' && after.scroll===before.scroll, after);
  check('center describes discovered attachments and omits unsupported POP3 filtering', !after.popOption && after.coverage.includes('当前显示 300 个附件') && after.coverage.includes('其他邮件可能还有附件'), after.coverage);
  const previewFocus=await run(`
    window.__originalRows=__rows.slice();
    const trigger=document.querySelector('[data-center-index="150"] [data-center-action="preview"]');
    MailBeaconAttachments.showPreview(__center.items,150,{invoke:async()=>({kind:'unsupported'})},trigger);
    [__rows[150],__rows[151]]=[__rows[151],__rows[150]];
    await __center.refresh();await MailBeaconAttachments.closePreview();
    return {focused:document.activeElement.closest('[data-center-index]')?.dataset.centerIndex,action:document.activeElement.dataset.centerAction,uid:__center.items[151].source.uid};
  `);
  check('closing preview restores the same attachment after rows reorder',previewFocus.focused==='151' && previewFocus.action==='preview' && previewFocus.uid===150,previewFocus);
  const changedIdentity=await run(`
    const trigger=document.querySelector('[data-center-index="151"] [data-center-action="preview"]');
    MailBeaconAttachments.showPreview(__center.items,151,{invoke:async()=>({kind:'unsupported'})},trigger);
    __rows[151]={...__rows[151],source:{...__rows[151].source,uidValidity:8}};
    await __center.refresh();await MailBeaconAttachments.closePreview();
    const fallback=document.activeElement===document.querySelector('.attachment-items');
    __rows=__originalRows;await __center.refresh();return {fallback};
  `);
  check('a replaced source identity returns preview focus to the list instead of another attachment',changedIdentity.fallback,changedIdentity);
  await click('[data-center-index="150"] [data-center-action="select"]');
  const shot=await send('Page.captureScreenshot',{format:'png'});
  writeFileSync('/tmp/mailbeacon-attachment-center-refresh.png',Buffer.from(shot.data,'base64'));
  await run(`__fail=true;await __center.refresh();`);
  const failed=await run(`return {count:__center.items.length,selected:document.querySelector('.attachment-item.selected')?.dataset.centerIndex,error:document.querySelector('.attachment-center-error').textContent,coverage:document.querySelector('.attachment-coverage').textContent};`);
  check('failed refresh leaves the known list visible and reports its age honestly',failed.count===300 && failed.selected==='150' && failed.error.includes('synthetic refresh failure') && failed.coverage.includes('仍显示上次读取的 300 个附件'),failed);
  const disposedFocus=await run(`
    const trigger=document.querySelector('[data-center-index="150"] [data-center-action="preview"]');
    MailBeaconAttachments.showPreview(__center.items,150,{invoke:async()=>({kind:'unsupported'})},trigger);
    __center.dispose();__center=MailBeaconAttachments.mount({container:document.querySelector('#list'),targets:[],accounts:[{id:'pop',email:'pop@example.test',receiveProtocol:'pop3'}],invoke:()=>{throw Error('must not widen the query');}});
    const next=document.querySelector('[data-center-filter="query"]');next.focus();
    await MailBeaconAttachments.closePreview();return document.activeElement===next;
  `);
  check('closing an old preview never steals focus after its center is disposed',disposedFocus);
  await waitFor('document.querySelector(".attachment-items").textContent.includes("暂不支持 POP3")','POP3-only explanation');
  check('POP3-only selection explains the supported scope instead of claiming an empty mailbox',await run(`return document.querySelector('.attachment-items').textContent.includes('暂不支持 POP3');`));
} catch (error) {
  check('browser test completes without an unexpected exception', false, error.stack || String(error));
} finally {
  await cleanup();
}
console.log(`\nAttachment center checks: ${results.filter(result=>result.ok).length}/${results.length} passed. Owned Chrome closed and temporary profile removed.`);
if(results.some(result=>!result.ok))process.exitCode=1;

async function click(selector) {
  const point=await run(`const el=document.querySelector(${JSON.stringify(selector)});el.scrollIntoView({block:'center'});const r=el.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2};`);
  await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...point});
  await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...point});
}
async function key(value,code) {
  await send('Input.dispatchKeyEvent',{type:'keyDown',key:value,code:value,windowsVirtualKeyCode:code});
  await send('Input.dispatchKeyEvent',{type:'keyUp',key:value,code:value,windowsVirtualKeyCode:code});
}
