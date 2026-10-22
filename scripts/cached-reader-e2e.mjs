// Real Chrome regression checks for cached IMAP reading. Synthetic ui/mock.js data only.
// Run: node scripts/cached-reader-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, CACHED_READER_CDP_PORT. All HTTP(S) requests are blocked.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.CACHED_READER_CDP_PORT || 9339);
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

async function install(mode) {
  await run(`
    window.__cacheMode = ${JSON.stringify(mode)};
    window.__cacheControl = {};
    window.__cacheCalls = [];
    const source = { accountId: 'a1', receiveEpoch: 1, folder: 'INBOX', uidValidity: 7, uid: 100 };
    const summary = { ...source, fromName: '缓存验收', fromAddr: 'cache@example.invalid', to: '我',
      subject: '本地缓存 · 浏览器回归', date: 1700000000000, seen: false, flagged: false, attachment: false };
    const html = '<p id="cache-proof">合成缓存正文，未连接真实邮箱</p><img src="https://cache.example.invalid/tracker.png" alt="测试远程图片">'
      + Array.from({length: 120}, (_, i) => '<p style="height:120px;margin:0">缓存第 ' + (i + 1) + ' 段</p>').join('');
    window.__cacheFixture = { source, summary, message: { uidValidity: 7, subject: summary.subject,
      from: {name: summary.fromName, addr: summary.fromAddr}, to: [{name: '演示', addr: 'demo@example.invalid'}],
      cc: [], bcc: [], date: summary.date, html, text: '合成缓存正文', hasRemote: true,
      attachments: [], messageId: 'cache-proof@example.invalid', references: [], receiptTo: null } };
    S.acct = 'a1'; S.folder = 'INBOX'; S.query = ''; S.searching = false; S.filter = 'all';
    S.threads = false; S.thread = null; S.results.clear(); S.lists.clear(); S.loading.clear();
    S.indexing.clear(); S.removed.clear(); S.sel = null; S.msg = null; S.msgError = '';
    S.showImages.clear(); S.localSnapshots.clear(); S.live.a1.unread = 1;
    S.lists.set('a1|INBOX', [structuredClone(summary)]); S.exhausted.add('a1|INBOX');
    render();
  `);
}

async function clickMail() {
  await run(`
    const row = document.querySelector('[data-open="a1|100|INBOX"]');
    if (!row) throw new Error('Synthetic mail row missing');
    row.click();
  `);
}

async function readyFrame() {
  await waitFor(`document.querySelector('#mail-frame')?.contentDocument?.getElementById('cache-proof')
    && document.querySelector('#mail-frame').style.height === '8000px'`, 'cached HTML frame');
}

async function rememberScroll() {
  await run(`
    window.__keptFrame = document.querySelector('#mail-frame');
    window.__keptDocument = __keptFrame.contentDocument;
    window.__keptScroll = document.querySelector('#reader .read-scroll');
    __keptScroll.scrollTop = 420;
    __keptDocument.scrollingElement.scrollTop = 700;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    window.__keptPositions = { outer: __keptScroll.scrollTop, inner: __keptDocument.scrollingElement.scrollTop,
      frameTop: __keptFrame.getBoundingClientRect().top };
  `);
}

async function scrollState() {
  return run(`
    const frame = document.querySelector('#mail-frame');
    const outer = document.querySelector('#reader .read-scroll');
    return { sameFrame: frame === __keptFrame, sameDocument: frame?.contentDocument === __keptDocument,
      sameScroll: outer === __keptScroll, before: __keptPositions,
      outer: outer?.scrollTop, inner: frame?.contentDocument?.scrollingElement?.scrollTop,
      frameTop: frame?.getBoundingClientRect().top,
      localOnly: S.msg?.localOnly, syncing: S.msg?.syncing,
      seen: S.lists.get('a1|INBOX')[0].seen, unread: S.live.a1.unread,
      replyDisabled: document.querySelector('#reader [data-action="reply"]')?.disabled,
      error: S.msg?.syncError, banner: document.querySelector('.offline-bar')?.textContent };
  `);
}

function keptScroll(state) {
  return state.sameFrame && state.sameDocument && state.sameScroll
    && state.outer > 0 && Math.abs(state.inner - state.before.inner) <= 1
    // The browser may adjust outer scrollTop when the status bar changes height.
    // Keeping the frame's viewport position proves the visible text stays put.
    && Math.abs(state.frameTop - state.before.frameTop) <= 1;
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-cached-reader-"));
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
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    let tauri;
    Object.defineProperty(window, '__TAURI__', { configurable: true, get: () => tauri, set: value => {
      const original = value.core.invoke;
      value.core.invoke = async (command, args = {}) => {
        const fixture = window.__cacheFixture;
        if (!fixture) return original(command, args);
        window.__cacheCalls.push({ command, args: structuredClone(args) });
        if (command === 'get_message_ref') {
          if (!['accountId','receiveEpoch','folder','uidValidity','uid'].every(key => args.source?.[key] === fixture.source[key]))
            throw new Error('Unexpected synthetic message identity');
          const result = { source: structuredClone(fixture.source), message: structuredClone(fixture.message),
            localOnly: !!args.localOnly, readMarked: !args.localOnly, snapshotAt: args.localOnly ? 1700000000000 : null };
          if (args.localOnly && window.__cacheMode === 'slow-local') return new Promise((resolve, reject) => {
            window.__cacheControl.local = () => resolve(result); window.__cacheControl.localError = reject;
          });
          if (!args.localOnly && window.__cacheMode !== 'slow-local') return new Promise((resolve, reject) => {
            window.__cacheControl.online = () => resolve(result); window.__cacheControl.onlineError = reject;
            window.__cacheControl.readOnly = () => resolve({...result, localOnly:true, readMarked:false, syncError:'合成校验暂未完成'});
          });
          return result;
        }
        if (command === 'prefetch_message') return false;
        if (command === 'thread_messages') return [];
        if (command === 'get_previews') return args.uids.map(uid => ({uid, text:'合成缓存正文'}));
        if (command === 'list_messages') return {items:[structuredClone(fixture.summary)],complete:true};
        if (command === 'list_saved_messages') return {items:[structuredClone(fixture.summary)],coverage:[],hasMore:false};
        return original(command, args);
      };
      tauri = value;
    } });
  ` });
  await send("Page.navigate", { url: pathToFileURL(join(ui, "index.html")).href });
  await waitFor(`typeof S !== 'undefined' && S.accounts.length === 2 && S.folders.a1 && S.folders.a2 && S.loading.size === 0`, 'synthetic app startup');

  await install('cached');
  await clickMail();
  await waitFor(`S.msg?.localOnly && S.msg?.syncing && !!window.__cacheControl.online`, 'local body before online completion');
  await readyFrame();
  const cached = await run(`return {
    visible: document.querySelector('#mail-frame').contentDocument.getElementById('cache-proof').textContent.includes('合成缓存正文'),
    localOnly: S.msg.localOnly, syncing: S.msg.syncing, unread: S.live.a1.unread,
    seen: S.lists.get('a1|INBOX')[0].seen,
    replyDisabled: document.querySelector('#reader [data-action="reply"]').disabled,
    calls: __cacheCalls.filter(c => c.command === 'get_message_ref').map(c => c.args.localOnly)
  };`);
  check('cached HTML is readable while online validation hangs, still read-only and unread',
    cached.visible && cached.localOnly && cached.syncing && cached.replyDisabled && !cached.seen && cached.unread === 1
      && cached.calls.includes(true) && cached.calls.includes(false), cached);
  await rememberScroll();
  await run(`__cacheControl.online();`);
  await waitFor(`S.msg && !S.msg.localOnly && !S.msg.syncing`, 'online upgrade');
  const upgraded = await scrollState();
  check('online success keeps the live iframe, inner scroll and reader scroll container',
    keptScroll(upgraded) && !upgraded.replyDisabled && upgraded.seen && upgraded.unread === 0, upgraded);

  await install('cached');
  await clickMail();
  await waitFor(`S.msg?.syncing && !!window.__cacheControl.online`, 'second local open');
  await readyFrame();
  await rememberScroll();
  await run(`__cacheControl.onlineError(new Error('合成服务器离线'));`);
  await waitFor(`S.msg?.syncError && !S.msg.syncing`, 'non-blocking online error');
  const failed = await scrollState();
  check('online failure keeps cached HTML and both scroll positions with a read-only error banner',
    keptScroll(failed) && failed.localOnly && failed.replyDisabled && !failed.seen && failed.unread === 1
      && failed.banner.includes('合成服务器离线'), failed);

  await install('cached');
  await clickMail();
  await waitFor(`S.msg?.syncing && !!window.__cacheControl.readOnly`, 'read-only server result');
  await run(`__cacheControl.readOnly();`);
  await waitFor(`S.msg?.localOnly && !S.msg.syncing && !!document.querySelector('#reader [data-action="retry"]')`, 'reconnect control');
  const reconnecting = await run(`
    const previous = __cacheCalls.filter(c => c.command === 'get_message_ref' && !c.args.localOnly).length;
    document.querySelector('#reader [data-action="retry"]').click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    return {previous, current: __cacheCalls.filter(c => c.command === 'get_message_ref' && !c.args.localOnly).length};
  `);
  check('reconnecting a server-fallback snapshot starts a fresh online request', reconnecting.current === reconnecting.previous + 1, reconnecting);
  await run(`__cacheControl.online();`);
  await waitFor(`S.msg && !S.msg.localOnly && !S.msg.syncing`, 'reconnected verified body');
  const reconnected = await run(`return {enabled: !document.querySelector('#reader [data-action="reply"]').disabled,
    banner: !!document.querySelector('.offline-bar'), seen:S.lists.get('a1|INBOX')[0].seen};`);
  check('successful reconnect restores actions and removes the offline banner', reconnected.enabled && !reconnected.banner && reconnected.seen, reconnected);

  await install('slow-local');
  await clickMail();
  await waitFor(`S.msg && !S.msg.localOnly && !!window.__cacheControl.local`, 'fast online result without waiting for local probe');
  await readyFrame();
  await rememberScroll();
  await run(`__cacheControl.local(); await new Promise(resolve => requestAnimationFrame(resolve));`);
  const late = await scrollState();
  check('a hanging cache probe cannot delay online mail or downgrade it after completing late',
    keptScroll(late) && !late.localOnly && !late.replyDisabled && late.seen && late.unread === 0, late);

  await install('cached');
  await clickMail();
  await waitFor(`S.msg?.syncing && !!window.__cacheControl.online`, 'image-policy local open');
  await readyFrame();
  const blocked = await run(`return document.querySelector('#mail-frame').contentDocument.querySelector('meta[http-equiv="Content-Security-Policy"]').content;`);
  await run(`document.querySelector('#reader [data-action="show-images"]').click();`);
  await waitFor(`document.querySelector('#mail-frame')?.contentDocument?.querySelector('meta[http-equiv="Content-Security-Policy"]')?.content.includes('img-src data: cid: https: http:')`, 'explicit image permission');
  await readyFrame();
  await rememberScroll();
  await run(`__cacheControl.online();`);
  await waitFor(`S.msg && !S.msg.localOnly`, 'image-enabled online upgrade');
  const imageState = await scrollState();
  const permitted = await run(`return document.querySelector('#mail-frame').contentDocument.querySelector('meta[http-equiv="Content-Security-Policy"]').content;`);
  check('show-images updates CSP explicitly and online upgrade preserves that permission and frame',
    blocked.includes('img-src data: cid:') && !blocked.includes('https:') && permitted.includes('img-src data: cid: https: http:')
      && keptScroll(imageState), {blocked, permitted, ...imageState});

  const failures = results.filter(item => !item.ok);
  console.log(`\n${results.length - failures.length}/${results.length} cached-reader browser checks passed`);
  process.exitCode = failures.length ? 1 : 0;
} catch (error) {
  console.error(error);
  process.exitCode = 1;
} finally {
  await cleanup();
}
