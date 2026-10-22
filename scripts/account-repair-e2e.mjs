// Real Chrome checks for account repair using mock data only; no credentials or mail are accessed.
// Uses ui/mock.js only. Run: node scripts/account-repair-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, ACCOUNT_REPAIR_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.ACCOUNT_REPAIR_CDP_PORT || 9345);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-account-repair-"));
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
  await send("Page.navigate", { url: pathToFileURL(join(ui, "index.html")).href });
  await waitFor("typeof S !== 'undefined' && S.accounts.length === 2 && S.folders.a1 && S.folders.a2 && S.loading.size === 0", "mock accounts and initial lists");
  await run(`
    for (const a of S.accounts) Object.assign(a, { receiveEpoch:1, connectionRevision:1, imapHost:'imap.example.test', imapPort:993, smtpHost:'smtp.example.test', smtpPort:465 });
    S.live.a1 = { state:'error', detail:'演示：授权码失效', unread:0 }; render();
  `);
  await run(`document.querySelector('[data-action="repair-account"]').focus();`);
  await key('Enter', 13);
  check('fault entry opens the affected account and focuses its new-secret field', await run(`return document.activeElement?.name === 'password' && document.activeElement.closest('form').dataset.accountRepair === 'a1';`));
  await run(`const form=document.querySelector('[data-account-repair="a1"]'); form.elements.password.value='wrong'; form.elements.smtpHost.value='smtp.fail.test';`);
  await click('[data-account-repair="a1"] [data-action="repair-test"]');
  await waitFor(`document.querySelector('[data-account-repair="a1"]').dataset.busy === 'false'`, 'probe completed');
  check('receive and send failures remain beside their account without changing identity', await run(`const f=document.querySelector('[data-account-repair="a1"]'); return f.querySelector('.connection-status').textContent.includes('收信：演示：授权码错误；发信：演示：SMTP 连接失败') && f.elements.password.value==='wrong' && account('a1').connectionRevision===1;`));
  await click('[data-account-repair="a1"] [data-action="repair-password"]');
  check('show-secret control only reveals the newly entered value', await run(`return document.querySelector('[data-account-repair="a1"] [name="password"]').type==='text';`));
  await run(`const form=document.querySelector('[data-account-repair="a1"]'); form.elements.password.value='storage-error'; form.elements.smtpHost.value='smtp.example.test';`);
  await click('[data-account-repair="a1"] [type="submit"]');
  await waitFor(`document.querySelector('[data-account-repair="a1"]').dataset.busy==='false'`, 'storage failure');
  check('storage failure retains typed values and account id', await run(`const f=document.querySelector('[data-account-repair="a1"]'); return f.querySelector('.connection-status').textContent.includes('无法保存') && f.elements.password.value==='storage-error' && !!account('a1');`));
  await run(`const form=document.querySelector('[data-account-repair="a1"]'); form.elements.password.value='new-demo-secret'; form.elements.imapHost.value='imap.example.test'; form.elements.imapPort.value='993'; form.elements.smtpHost.value='smtp.example.test'; form.elements.smtpPort.value='465';`);
  await click('[data-account-repair="a1"] [type="submit"]');
  await waitFor(`document.querySelector('[data-account-repair="a1"]').dataset.busy==='false' && S.live.a1.state==='online'`, 'successful reconnect');
  check('successful save clears new secret and preserves the existing account', await run(`const f=document.querySelector('[data-account-repair="a1"]'); return f.elements.password.value==='' && S.accounts.length===2 && account('a1').connectionRevision===2 && !('password' in account('a1'));`));
  await run(`onMailEvent({type:'status', accountId:'a1', state:'error', detail:'old callback', connectionRevision:1, receiveEpoch:1});`);
  check('late failure from the previous connection cannot overwrite recovery', await run(`return S.live.a1.state==='online';`));
  await run(`onMailEvent({type:'status', accountId:'a1', state:'online', detail:'new callback before command reply', connectionRevision:3, receiveEpoch:1});`);
  check('new connection events wait for their matching configuration snapshot', await run(`return account('a1').connectionRevision===2 && S.accountRepairEvents.get('a1')?.length===1;`));
  await run(`adoptRepairedAccount({...account('a1'), connectionRevision:3});`);
  check('adopting the new snapshot replays its already-arrived connection event', await run(`return S.live.a1.detail==='new callback before command reply' && !S.accountRepairEvents.has('a1');`));
  await run(`document.querySelector('[data-account-repair="a1"] [name="name"]').dispatchEvent(new Event('input',{bubbles:true}));`);
  check('editing invalidates the previous connection-test conclusion', await run(`return document.querySelector('[data-account-repair="a1"] .connection-status').textContent.includes('尚未保存');`));
  await run(`closeOverlay(); openSettings(true); formKind='custom'; formReceiveProtocol='pop3'; document.querySelector('#acct-form-wrap').innerHTML=accountForm(); const f=document.querySelector('#acct-form'); Object.assign(f.elements.email,{value:'demo@example.test'}); f.elements.password.value='mock-new'; f.elements.pop3Host.value='pop.example.test'; f.elements.pop3Port.value='110'; f.elements.pop3Tls.checked=false; f.elements.smtpHost.value='smtp.example.test'; f.elements.smtpPort.value='587';`);
  await click('#acct-form [type="submit"]');
  await waitFor(`!!document.querySelector('#acct-form .form-err')`, 'mock new-account login rejected');
  check('new-account failure preserves host, port, TLS and temporary new secret', await run(`const f=document.querySelector('#acct-form'); return f.elements.pop3Host.value==='pop.example.test' && f.elements.pop3Port.value==='110' && !f.elements.pop3Tls.checked && f.elements.smtpPort.value==='587' && f.elements.password.value==='mock-new';`));
  await run(`closeOverlay();`);
  check('closing the sheet removes its password fields', await run(`return !document.querySelector('input[name="password"]');`));
} catch (error) {
  check('browser test completes without an unexpected exception', false, error.stack || String(error));
} finally {
  await cleanup();
}

console.log(`\nAccount repair checks: ${results.filter(result => result.ok).length}/${results.length} passed. Owned Chrome closed and temporary profile removed.`);
if (results.some(result => !result.ok)) process.exitCode = 1;

async function key(value, code) {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: value, code: value, windowsVirtualKeyCode: code, ...(value === 'Enter' ? { text: '\r', unmodifiedText: '\r' } : {}) });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: value, code: value, windowsVirtualKeyCode: code });
}

async function click(selector) {
  const point = await run(`
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error('Missing element: ' + ${JSON.stringify(selector)});
    element.scrollIntoView({ block: 'nearest' });
    const rect = element.getBoundingClientRect();
    return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
  `);
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
