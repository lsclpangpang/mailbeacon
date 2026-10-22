// Real Chrome checks for local send records and recovery actions.
// Uses ui/mock.js only. Run: node scripts/send-records-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, SEND_RECORDS_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.SEND_RECORDS_CDP_PORT || 9343);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-send-records-"));
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

  await send("Page.navigate", { url: pathToFileURL(join(ui,"index.html")).href });
  await waitFor("typeof S !== 'undefined' && S.accounts.length > 0", "demo accounts");
  await run(`document.querySelector('[data-action="send-records"]').click();`);
  await waitFor("document.querySelector('.send-records-empty')", "empty records");
  check("sidebar opens an honest empty state", await run(`return document.querySelector('.send-records-empty').textContent.includes('还没有');`));
  await run(`closeOverlay(); await invoke('send_mail', { draft:{accountId:'a1',to:'demo@example.invalid',subject:'发送记录 · 测试',body:'完整原文内容'}, sendId:'browser-one' }); await openSendRecords();`);
  await waitFor("document.querySelector('[data-send-record=browser-one]')", "persisted preview record");
  check("normal send becomes a visible record", await run(`return document.querySelector('.send-record-status').textContent === '已发送';`));
  await run(`document.querySelector('[data-record-action=view]').click();`);
  await waitFor("!document.querySelector('.send-records-preview').hidden", "raw preview");
  check("original content can be read", await run(`return document.querySelector('.send-records-preview').textContent.includes('完整原文内容');`));
  await run(`document.querySelector('[data-record-action=export]').click();`);
  await waitFor("document.querySelector('.send-records-notice').textContent.includes('.eml')", "export feedback");
  check("export identifies saved EML", true);
  await run(`
    window.__recordCalls=[];
    window.__recordFixture=[
      {id:'pending',accountId:'a1',subject:'已发送，副本待同步',recipients:['demo@example.invalid'],createdAt:Date.now(),status:'accepted',copyStatus:'failed',copyError:'服务器暂不可用，原文已保留'},
      {id:'unknown',accountId:'a1',subject:'需要确认的提交',recipients:['demo@example.invalid'],createdAt:Date.now(),status:'unknown',copyStatus:'pending',error:'连接在提交后中断，不会自动重新发送'},
      {id:'rejected',accountId:'a1',subject:'未发送的邮件',recipients:['demo@example.invalid'],createdAt:Date.now(),status:'rejected',copyStatus:'pending',error:'收件地址被服务器拒绝'}
    ];
    await MailBeaconSendRecords.open({overlay:document.querySelector('#overlay'),invoke:async (cmd,args)=>{
      window.__recordCalls.push({cmd,args});
      if(cmd==='list_send_records') return structuredClone(window.__recordFixture);
      if(cmd==='retry_send_copy') throw '网络断开，请恢复连接后重试';
      if(cmd==='resend_record') { await new Promise(r=>setTimeout(r,200)); return {status:'accepted',copyStatus:'pending'}; }
    }});
  `);
  check("accepted copy failure never offers SMTP resend", await run(`return !document.querySelector('[data-send-record=pending] [data-record-action=resend]') && !!document.querySelector('[data-send-record=pending] [data-record-action=copy]');`));
  await run(`document.querySelector('[data-record-action=copy]').click();`);
  await waitFor("!document.querySelector('.send-records-error').hidden", "persistent error");
  check("copy retry failure remains visible", await run(`return document.querySelector('.send-records-error').textContent.includes('网络断开');`));
  await run(`window.confirm=()=>false; document.querySelector('[data-send-record=unknown] [data-record-action=resend]').click();`);
  check("cancelled resend makes zero submit calls", await run(`return window.__recordCalls.filter(c=>c.cmd==='resend_record').length===0;`));
  await run(`window.confirm=()=>true; const b=document.querySelector('[data-send-record=unknown] [data-record-action=resend]'); b.click(); b.click();`);
  await sleep(300);
  check("confirmed double click submits once", await run(`return window.__recordCalls.filter(c=>c.cmd==='resend_record').length===1;`));
  await send("Emulation.setDeviceMetricsOverride", { width:700,height:760,deviceScaleFactor:1,mobile:false });
  check("record sheet fits a narrow viewport", await run(`const s=document.querySelector('.send-records-sheet'); return s.getBoundingClientRect().right<=innerWidth && s.scrollWidth<=s.clientWidth+1;`));
  const shot=await send("Page.captureScreenshot", {format:"png",captureBeyondViewport:false});
  writeFileSync('/tmp/mailbeacon-send-records.png',Buffer.from(shot.data,'base64'));
  await send("Input.dispatchKeyEvent",{type:"keyDown",key:"Escape",code:"Escape",windowsVirtualKeyCode:27});
  check("Escape closes records", await run(`return !document.querySelector('.send-records-sheet');`));
} catch(error) { check("browser test completed",false,error.stack || String(error)); }
finally { await cleanup(); }
const passed=results.filter(r=>r.ok).length;
console.log(`${passed}/${results.length} send-record checks passed; fixture Chrome closed.`);
if(passed!==results.length) process.exitCode=1;
