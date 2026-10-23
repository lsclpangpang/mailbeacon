// Real Chrome checks for reader Space navigation and its shortcut hint.
// Uses ui/mock.js only. Run: node scripts/reader-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, READER_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.READER_CDP_PORT || 9336);
const screenshots = ["/tmp/mailbeacon-reader-hud.png", "/tmp/mailbeacon-reader-hud-narrow.png", "/tmp/mailbeacon-reader-hud-narrow-disabled.png", "/tmp/mailbeacon-list-star-focus.png", "/tmp/mailbeacon-list-tab-focus.png"];
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

async function waitForReaderScroll() {
  await waitFor(`typeof readerScrollMotion === 'undefined' || !readerScrollMotion?.active`, "reader scroll animation to finish");
}

async function waitForReaderEntry() {
  await waitFor(`[...document.querySelectorAll('#reader .read-scroll, #reader .reader-hint')].every(el =>
    el.getAnimations().every(animation => animation.playState === 'finished' || animation.playState === 'idle'))`, "reader entry animations to finish");
}

async function pressSpace({ waitForMotion = true, delay = 80 } = {}) {
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: " ", code: "Space", windowsVirtualKeyCode: 32, text: " ", unmodifiedText: " " });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: " ", code: "Space", windowsVirtualKeyCode: 32 });
  if (delay) await sleep(delay);
  if (waitForMotion) await waitForReaderScroll();
}

async function screenshot(path) {
  const image = await send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
  writeFileSync(path, Buffer.from(image.data, "base64"));
}

function fixture(kind = "short") {
  const titles = ["阅读快捷键 · 验证邮件", "已读的相邻邮件", "优先阅读这封未读邮件", "列表中的最后一封"];
  const summaries = [100, 90, 80, 70].map((uid, index) => ({
    accountId: "a1", receiveEpoch: 1, folder: "INBOX", uidValidity: 1, uid, fromName: "MailBeacon 测试", fromAddr: "reader@example.invalid",
    to: "我", subject: titles[index], date: Date.now() - index * 60000, seen: index === 1 || index === 3,
    flagged: false, attachment: false, recipients: ["jarvis@gmail.com"],
  }));
  const body = "你好，\n\n这是一封模拟邮件。使用空格滚动阅读，到底后继续阅读下一封。\n\nMailBeacon";
  const bodies = Object.fromEntries(summaries.map((mail) => [mail.uid, {
    subject: mail.subject, from: { name: mail.fromName, addr: mail.fromAddr }, to: [{ name: "Jarvis", addr: "jarvis@gmail.com" }],
    cc: [], bcc: [], date: mail.date, html: null, text: body, attachments: [], hasRemote: false,
    messageId: `${mail.uid}@reader.invalid`, references: [], uidValidity: 1, receiptTo: null,
  }]));
  if (kind === "long") bodies[100].text = Array.from({ length: 110 }, (_, i) => `第 ${i + 1} 段：逐页阅读长邮件，每次空格都应保留当前邮件，不跳过尚未阅读的正文。`).join("\n\n");
  if (kind === "iframe") bodies[100].html = Array.from({ length: 150 }, (_, i) =>
    `<p style="height:160px;margin:0;border-bottom:1px solid #eee">第 ${i + 1} 段 · 超长 HTML 邮件的滚动验证</p>`).join("");
  return { summaries, bodies };
}

async function installFixture(kind) {
  const data = fixture(kind);
  await run(`
    window.__readerFixture = ${JSON.stringify(data)};
    window.__readerOnlineGate = null; window.__readerFlagGate = null;
    S.flagOperations.clear(); S.confirmedFlags.clear();
    window.__readerCalls = [];
    S.acct = 'a1'; S.folder = 'INBOX'; S.query = ''; S.searching = false; S.filter = 'all';
    S.threads = false; S.thread = null; S.results.clear(); S.lists.clear(); S.loading.clear();
    S.indexing.clear(); S.removed.clear(); S.sel = null; S.msg = null; S.msgError = '';
    visibleListSnapshot?.reset(); visibleListPending = 0;
    const oldToast = document.querySelector('#toast'); oldToast.hidden = true; oldToast.textContent = '';
    S.lists.set('a1|INBOX', structuredClone(window.__readerFixture.summaries));
    S.exhausted.add('a1|INBOX');
    render();
  `);
}

async function open(uid) {
  await run(`
    const row = document.querySelector('[data-open="a1|${uid}|INBOX"]');
    if (!row) throw new Error('Missing fixture row ${uid}');
    row.click();
  `);
  await waitFor(`S.sel?.uid === ${uid} && S.msg && !S.msg.partial`, `mail ${uid} body`);
  await waitForReaderEntry();
  await run(`document.activeElement?.blur();`);
}

// Capture in the first rendered frame, before CDP round trips can outlast the short entrance.
async function captureOpeningMotion(uid) {
  return run(`
    const row = document.querySelector('[data-open="a1|${uid}|INBOX"]');
    if (!row) throw new Error('Missing fixture row ${uid}');
    row.click();
    await new Promise(resolve => requestAnimationFrame(resolve));
    const sample = el => {
      const css = getComputedStyle(el);
      return { opacity: Number(css.opacity), transform: css.transform, animationName: css.animationName,
        transitionDuration: css.transitionDuration,
        animations: el.getAnimations().map(animation => ({ state: animation.playState, time: animation.currentTime })) };
    };
    return { hint: sample(document.querySelector('.reader-hint')), reader: sample(document.querySelector('.read-scroll')) };
  `);
}

async function state() {
  return run(`
    const outer = document.querySelector('#reader .read-scroll');
    const frame = document.querySelector('#mail-frame');
    const inner = frame?.contentDocument?.scrollingElement;
    const hint = document.querySelector('.reader-hint');
    const metrics = (el) => el ? ({ top: el.scrollTop, height: el.clientHeight, total: el.scrollHeight }) : null;
    return {
      uid: S.sel?.uid, outer: metrics(outer), inner: metrics(inner), hintCount: document.querySelectorAll('.reader-hint').length,
      hintText: hint?.textContent, hintOpacity: hint ? Number(getComputedStyle(hint).opacity) : null,
      hintHiding: hint?.classList.contains('is-hiding'), hintUntil: S.readerHintUntil,
      hintTransform: hint ? getComputedStyle(hint).transform : null,
      readerOpacity: outer ? Number(getComputedStyle(outer).opacity) : null,
      readerTransform: outer ? getComputedStyle(outer).transform : null,
      activeAnimations: [...document.querySelectorAll('#reader .read-scroll, #reader .reader-hint')]
        .flatMap(el => el.getAnimations()).filter(animation => animation.playState === 'running' || animation.playState === 'pending').length,
      scrollActive: typeof readerScrollMotion !== 'undefined' && !!readerScrollMotion?.active,
      now: Date.now(), frameBottom: frame?.getBoundingClientRect().bottom, viewportBottom: outer?.getBoundingClientRect().bottom,
      frameTop: frame?.getBoundingClientRect().top, viewportTop: outer?.getBoundingClientRect().top,
      lastKey: window.__readerLastKey,
    };
  `);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-reader-"));
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
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    let tauri;
    Object.defineProperty(window, '__TAURI__', { configurable: true, get: () => tauri, set: (value) => {
      const original = value.core.invoke;
      value.core.invoke = async (command, args = {}) => {
        const fixture = window.__readerFixture;
        if (!fixture) return original(command, args);
        window.__readerCalls.push({ command, args: structuredClone(args) });
        const fixtureMessage = source => {
          const keys = ['accountId', 'receiveEpoch', 'folder', 'uidValidity', 'uid'];
          const summary = fixture.summaries.find(mail => keys.every(key => mail[key] === source?.[key]));
          if (!summary) throw new Error('Unexpected fixture message identity');
          return summary;
        };
        if (command === 'get_message_ref') {
          const summary = fixtureMessage(args.source), body = fixture.bodies[args.source.uid];
          if (!body) throw new Error('No fixture body for ' + args.source.uid);
          if (!args.localOnly && window.__readerOnlineGate?.uid === args.source.uid) await window.__readerOnlineGate.promise;
          if (!args.localOnly) summary.seen = true;
          return { source: structuredClone(args.source), message: structuredClone(body),
            localOnly: !!args.localOnly, readMarked: !args.localOnly, snapshotAt: args.localOnly ? Date.now() : null };
        }
        if (command === 'get_message') {
          const body = fixture.bodies[args.uid];
          if (!body) throw new Error('No fixture body for ' + args.uid);
          return structuredClone(body);
        }
        if (command === 'prefetch_message') return false;
        if (command === 'thread_messages') return [];
        if (command === 'get_previews') return args.uids.map(uid => ({ uid, text: '模拟邮件，未连接真实邮箱' }));
        if (command === 'list_messages') return { items: structuredClone(fixture.summaries.filter(m =>
          m.accountId === args.accountId && m.folder === args.folder && (!args.before || m.uid < args.before) &&
          (!args.flaggedOnly || m.flagged) && (!args.unreadOnly || !m.seen))), complete: true, more: false };
        if (command === 'execute_mail_operation') {
          const request = args.request;
          if (window.__readerFlagGate) {
            const result = await window.__readerFlagGate.promise;
            if (result) return { operationId:request.operationId, items:request.items.map(source=>({source,status:result.status,message:result.message})) };
          }
          if (!['seen', 'flagged'].includes(request.action) || typeof request.value !== 'boolean')
            throw new Error('Unsupported reader fixture operation');
          const items = request.items.map(source => {
            fixtureMessage(source)[request.action] = request.value;
            return { source: structuredClone(source), status: 'succeeded', message: '已完成' };
          });
          return { operationId: request.operationId, items, undoToken: null, expiresAt: 0 };
        }
        if (command === 'set_flag') return null;
        return original(command, args);
      };
      tauri = value;
    } });
    document.addEventListener('keydown', e => {
      window.__readerLastKey = { key: e.key, trusted: e.isTrusted, source: 'main', tag: e.target.tagName };
    }, true);
  ` });
  await send("Page.navigate", { url: pathToFileURL(join(ui, "index.html")).href });
  await send("Page.bringToFront");
  await waitFor(`typeof S !== 'undefined' && S.accounts.length === 2 && S.folders.a1 && S.folders.a2 && S.loading.size === 0`, "mock app startup");
  check("the page uses mock accounts and the new navigation helper", await run(`return S.accounts[0].email === 'jarvis@gmail.com' && !!window.MailBeaconReaderNav;`));

  await installFixture("short");
  const pointerClick = async (position) => {
    await send("Input.dispatchMouseEvent", { type: "mousePressed", button: "left", clickCount: 1, ...position });
    await send("Input.dispatchMouseEvent", { type: "mouseReleased", button: "left", clickCount: 1, ...position });
  };
  const keyPress = async (key, code, windowsVirtualKeyCode, modifiers = 0) => {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key, code, windowsVirtualKeyCode, modifiers });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code, windowsVirtualKeyCode, modifiers });
  };
  await pointerClick(await run(`const r = document.querySelector('[data-open="a1|100|INBOX"]').getBoundingClientRect(); return { x: r.x + 80, y: r.y + 30 };`));
  await waitFor(`S.sel?.uid === 100 && S.msg && !S.msg.partial`, "pointer-selected focus fixture");
  await pointerClick(await run(`const r = document.querySelector('#list .rows').getBoundingClientRect(); return { x: r.x + 80, y: r.bottom - 30 };`));
  check("clicking the blank list area can focus the list container", await run(`return document.activeElement === document.querySelector('#list .rows');`));
  await keyPress("s", "KeyS", 83);
  await waitFor(`findMsg(S.sel)?.flagged && S.moving.size === 0 && S.loading.size === 0`, "keyboard star operation");
  const starFocus = await run(`const rows = document.querySelector('#list .rows'), css = getComputedStyle(rows); return {
    key: window.__readerLastKey, activeMail: document.activeElement.dataset.open,
    outlineStyle: css.outlineStyle, outlineWidth: css.outlineWidth, flagged: findMsg(S.sel).flagged,
    operations: window.__readerCalls.filter(c => c.command === 'execute_mail_operation').map(c => c.args.request.action),
  };`);
  check("real S stars the selected mail and restores its row without a list-wide focus ring",
    starFocus.key?.trusted && starFocus.key.key === "s" && starFocus.flagged && starFocus.activeMail === "a1|100|INBOX" &&
    (starFocus.outlineStyle === "none" || starFocus.outlineWidth === "0px") && starFocus.operations.join() === "flagged", starFocus);
  await screenshot(screenshots[3]);
  await keyPress("Tab", "Tab", 9);
  const tabFocus = await run(`const el = document.activeElement, css = getComputedStyle(el); return {
    key: window.__readerLastKey, activeMail: el.dataset.open, visible: el.matches(':focus-visible'),
    outlineStyle: css.outlineStyle, outlineWidth: css.outlineWidth,
  };`);
  check("real Tab moves to the next mail and retains a visible control focus indicator",
    tabFocus.key?.trusted && tabFocus.activeMail === "a1|90|INBOX" && tabFocus.visible &&
    tabFocus.outlineStyle !== "none" && Number.parseFloat(tabFocus.outlineWidth) > 0, tabFocus);
  await screenshot(screenshots[4]);
  await keyPress("a", "KeyA", 65, 2);
  const batchFocus = await run(`const rows = document.querySelector('#list .rows'), css = getComputedStyle(rows); return {
    batchMode: S.batchMode, count: S.batch.size, containerFocused: document.activeElement === rows,
    outlineStyle: css.outlineStyle, outlineWidth: css.outlineWidth,
  };`);
  check("multi-select keeps its keyboard scope without outlining the whole container",
    batchFocus.batchMode && batchFocus.count === 4 && batchFocus.containerFocused &&
    (batchFocus.outlineStyle === "none" || batchFocus.outlineWidth === "0px"), batchFocus);
  await keyPress("ArrowDown", "ArrowDown", 40);
  check("multi-select arrows still move focus onto an individual mail",
    await run(`return S.batchMode && document.activeElement.dataset.open === 'a1|100|INBOX';`));
  await run(`clearBatch(); S.sel = null; S.lists.set('a1|INBOX', []); renderList(); document.querySelector('#list .rows').focus();`);
  await keyPress("s", "KeyS", 83);
  const emptyFocus = await run(`const rows = document.querySelector('#list .rows'), css = getComputedStyle(rows); return {
    containerFocused: document.activeElement === rows, empty: currentList().length === 0,
    outlineStyle: css.outlineStyle, outlineWidth: css.outlineWidth,
  };`);
  check("an empty list retains the fallback keyboard scope without a container ring",
    emptyFocus.containerFocused && emptyFocus.empty && (emptyFocus.outlineStyle === "none" || emptyFocus.outlineWidth === "0px"), emptyFocus);

  const holdOnline = async () => run(`
    const gate = { uid: 100 }; gate.promise = new Promise((resolve,reject) => { gate.resolve=resolve; gate.reject=reject; });
    window.__readerOnlineGate = gate;
  `);
  const holdFlag = async () => run(`
    const gate = {}; gate.promise = new Promise((resolve,reject) => { gate.resolve=resolve; gate.reject=reject; });
    window.__readerFlagGate = gate;
  `);
  await installFixture("long");
  await run(`S.live.a1.state='online'; S.lists.get('a1|INBOX')[0].localOnly=true;`);
  await open(100); await holdFlag();
  await run(`window.__cachedBodyElement=document.querySelector('.mail-body'); window.__cachedBodyText=S.msg.text;
    document.querySelector('.read-scroll').scrollTop=180;
    window.__flagScroll=document.querySelector('.read-scroll').scrollTop;`);
  await keyPress("s", "KeyS", 83); await keyPress("s", "KeyS", 83);
  const cachedPending = await run(`return {
    localOnly:S.msg.localOnly, bodyKept:window.__cachedBodyElement===document.querySelector('.mail-body')&&S.msg.text===window.__cachedBodyText,
    reads:window.__readerCalls.filter(c=>c.command==='get_message_ref'&&!c.args.localOnly).length,
    writes:window.__readerCalls.filter(c=>c.command==='execute_mail_operation').length, flagged:findMsg(S.sel).flagged,
    saving:document.querySelector('.flag-feedback').textContent, busy:document.querySelector('[data-action="star"]').getAttribute('aria-busy'),
    scrollKept:document.querySelector('.read-scroll').scrollTop===window.__flagScroll
  };`);
  check("cached S submits once immediately, shows saving, and preserves body and scroll",
    cachedPending.localOnly && cachedPending.bodyKept && cachedPending.scrollKept && cachedPending.reads===0 && cachedPending.writes===1 && !cachedPending.flagged && cachedPending.busy==='true' && cachedPending.saving.includes('正在加星标'), cachedPending);
  await screenshot('/tmp/mailbeacon-star-saving.png');
  await run(`window.__readerFlagGate.resolve();`);
  await waitFor(`findMsg(S.sel)?.flagged && S.moving.size===0 && S.loading.size===0`, "cached S confirmed operation");
  const cachedFlagged = await run(`return { uid:S.sel.uid, localOnly:S.msg.localOnly, flagged:findMsg(S.sel).flagged,
    writes:window.__readerCalls.filter(c=>c.command==='execute_mail_operation').length,
    bodyKept:window.__cachedBodyElement===document.querySelector('.mail-body'),saving:document.querySelector('.flag-feedback').textContent };`);
  check("successful cached S keeps the cache readable without changing unrelated permissions",
    cachedFlagged.uid===100 && cachedFlagged.localOnly && cachedFlagged.flagged && cachedFlagged.writes===1 && cachedFlagged.bodyKept && !cachedFlagged.saving, cachedFlagged);
  await screenshot('/tmp/mailbeacon-cached-star-verified.png');

  await installFixture("short"); await holdOnline(); await open(100);
  await keyPress("s", "KeyS", 83);
  await waitFor(`findMsg(S.sel)?.flagged && S.moving.size===0`, "flag completes while body waits");
  const independent = await run(`return { syncing:S.msg.syncing, reads:window.__readerCalls.filter(c=>c.command==='get_message_ref'&&!c.args.localOnly).length,
    writes:window.__readerCalls.filter(c=>c.command==='execute_mail_operation').length };`);
  check("S completes without waiting for an ongoing body read", independent.syncing && independent.reads===1 && independent.writes===1, independent);
  await run(`window.__readerOnlineGate.resolve();`);
  await waitFor(`!S.msg.syncing && S.loading.size===0`, "body read finishes independently");

  for (const status of ['failed','unknown']) {
    await installFixture("short");
    await run(`S.lists.get('a1|INBOX')[0].localOnly=true;`);
    await open(100); await holdFlag(); await keyPress("s", "KeyS", 83);
    await run(`window.__readerFlagGate.resolve({status:${JSON.stringify(status)},message:'模拟服务器连接超时'});`);
    await waitFor(`S.flagOperations.size===0 && S.moving.size===0`, "flag outcome");
    const failure = await run(`return { localOnly:S.msg.localOnly, flagged:findMsg(S.sel).flagged, text:S.msg.text,
      error:document.querySelector('#toast').textContent, writes:window.__readerCalls.filter(c=>c.command==='execute_mail_operation').length };`);
    check(status+" flag results keep the mail readable without claiming success",
      failure.localOnly && !failure.flagged && failure.text && failure.writes===1 && (status==='failed'?failure.error.includes('模拟服务器连接超时'):failure.error.includes('待确认')), failure);
  }
  await screenshot('/tmp/mailbeacon-cached-star-failed.png');

  await installFixture("short");
  await run(`S.lists.get('a1|INBOX')[0].localOnly=true;`);
  await open(100); await holdFlag(); await keyPress("s", "KeyS", 83);
  await open(90); await run(`window.__readerFlagGate.resolve();`);
  await waitFor(`S.flagOperations.size===0 && S.loading.size===0`, "original mail star survives navigation");
  const changedSelection = await run(`return { uid:S.sel.uid, flagged:findMsg(S.sel).flagged,original:S.lists.get('a1|INBOX').find(m=>m.uid===100).flagged,
    writes:window.__readerCalls.filter(c=>c.command==='execute_mail_operation').length };`);
  check("switching mail preserves the original S and never stars the newly selected mail", changedSelection.uid===90 && !changedSelection.flagged && changedSelection.original && changedSelection.writes===1, changedSelection);

  await installFixture("short");
  const entryStart = await captureOpeningMotion(100);
  check("opening mail visibly animates both the shortcut hint and reader content",
    entryStart.hint.animationName === "reader-hint-enter" && entryStart.reader.animationName === "reader-content-enter" &&
    [entryStart.hint, entryStart.reader].every(item => item.opacity < 1 && item.animations.some(animation => animation.state === "running")), entryStart);
  await waitForReaderEntry();
  const entryEnd = await state();
  await sleep(70);
  const entryStill = await state();
  check("reader and shortcut hint stop moving after their short entrance",
    entryEnd.activeAnimations === 0 && entryEnd.readerOpacity === 1 && entryEnd.hintOpacity === 1 &&
    entryStill.activeAnimations === 0 && entryStill.readerTransform === entryEnd.readerTransform && entryStill.hintTransform === entryEnd.hintTransform,
    { reader: entryEnd.readerTransform, hint: entryEnd.hintTransform, active: entryStill.activeAnimations });
  await run(`S.msg.partial = true; renderReader(); S.msg.partial = false; renderReader();`);
  const rebuilt = await state();
  check("rebuilding the same mail after entrance does not replay either animation",
    rebuilt.activeAnimations === 0 && rebuilt.readerOpacity === 1 && rebuilt.hintOpacity === 1 && rebuilt.hintUntil === entryEnd.hintUntil,
    { active: rebuilt.activeAnimations, deadline: rebuilt.hintUntil });

  await installFixture("long");
  await open(100);
  const longBefore = await state();
  const longTarget = Math.min(longBefore.outer.total - longBefore.outer.height, longBefore.outer.top + longBefore.outer.height * 0.9);
  await pressSpace({ waitForMotion: false, delay: 40 });
  const longMiddle = await state();
  check("Space has a visible intermediate scroll frame before reaching its target",
    longMiddle.uid === 100 && longMiddle.scrollActive && longMiddle.outer.top > longBefore.outer.top && longMiddle.outer.top < longTarget,
    { top: longMiddle.outer.top, target: longTarget, active: longMiddle.scrollActive });
  await waitForReaderScroll();
  const longAfter = await state();
  check("Space scrolls a long plain mail without changing selection", longBefore.outer.total > longBefore.outer.height && longAfter.uid === 100 && longAfter.outer.top > longBefore.outer.top && longAfter.outer.top <= longBefore.outer.height * 0.9 + 1, { before: longBefore.outer, after: longAfter.outer });
  check("reader keyboard events are real trusted browser events", longAfter.lastKey?.trusted && longAfter.lastKey.key === " ", longAfter.lastKey);
  await run(`const outer = document.querySelector('.read-scroll'); outer.scrollTop = outer.scrollHeight - outer.clientHeight - outer.clientHeight * 0.45;`);
  await pressSpace({ waitForMotion: false, delay: 40 });
  const lastPageMiddle = await state();
  await pressSpace({ waitForMotion: false, delay: 0 });
  await pressSpace({ waitForMotion: false, delay: 0 });
  const repeatedSpace = await state();
  await waitForReaderScroll();
  const lastPageEnd = await state();
  check("additional Space presses during the final page animation keep the current mail",
    lastPageMiddle.scrollActive && lastPageMiddle.outer.top < lastPageMiddle.outer.total - lastPageMiddle.outer.height &&
    repeatedSpace.uid === 100 && lastPageEnd.uid === 100 &&
    Math.abs(lastPageEnd.outer.top - (lastPageEnd.outer.total - lastPageEnd.outer.height)) <= 1,
    { middle: lastPageMiddle.outer.top, repeatedUid: repeatedSpace.uid, final: lastPageEnd.outer });
  await pressSpace();
  check("a new Space after scrolling finishes chooses the later unread mail", (await state()).uid === 80);

  await installFixture("short");
  await open(100);
  const short = await state();
  await pressSpace();
  check("a short mail immediately advances past read mail to the next unread", short.outer.total <= short.outer.height + 1 && (await state()).uid === 80, short.outer);
  await pressSpace();
  check("without a later unread mail Space opens the adjacent row", (await state()).uid === 70);
  await pressSpace();
  const end = await run(`return { uid: S.sel?.uid, reader: !!document.querySelector('.mail-card'), toast: document.querySelector('#toast').textContent };`);
  check("Space at the final row preserves the current mail and reports the end", end.uid === 70 && end.reader && /没有下一封/.test(end.toast), end);

  await installFixture("iframe");
  await open(100);
  await waitFor(`document.querySelector('#mail-frame')?.contentDocument?.scrollingElement?.scrollHeight > 8000 && parseFloat(document.querySelector('#mail-frame').style.height) === 8000`, "overflowing mail iframe");
  await run(`
    const frame = document.querySelector('#mail-frame'), doc = frame.contentDocument;
    doc.addEventListener('keydown', e => { window.__readerLastKey = { key: e.key, trusted: e.isTrusted, source: 'iframe', tag: e.target.tagName }; }, true);
    doc.body.tabIndex = -1;
    frame.contentWindow.focus(); doc.body.focus({ preventScroll: true });
  `);
  let previous = await state(), nested = previous, safeSteps = true, count = 0;
  while (nested.inner.top <= 1 && count++ < 25) {
    await pressSpace();
    nested = await state();
    if (nested.uid !== 100 || !nested.inner) { safeSteps = false; break; }
    const delta = (nested.outer.top - previous.outer.top) + (nested.inner.top - previous.inner.top);
    safeSteps &&= delta > 0 && delta <= previous.outer.height * 0.9 + 2;
    previous = nested;
  }
  check("focused HTML iframe forwards real Space keydowns to the reader", nested.lastKey?.trusted && nested.lastKey.source === "iframe", nested.lastKey);
  check("over-8000px HTML scrolls outer then inner without skipping or changing mail", safeSteps && nested.uid === 100 && nested.inner?.top > 0 && nested.frameBottom <= nested.viewportBottom + 1, { steps: count, outer: nested.outer, inner: nested.inner });
  await run(`
    const outer = document.querySelector('.read-scroll'), inner = document.querySelector('#mail-frame').contentDocument.scrollingElement;
    outer.scrollTop = outer.scrollHeight; inner.scrollTop = 0;
  `);
  const partiallyVisible = await state();
  const visibleHeight = Math.max(0, Math.min(partiallyVisible.frameBottom, partiallyVisible.viewportBottom) - Math.max(partiallyVisible.frameTop, partiallyVisible.viewportTop));
  await pressSpace();
  const outerEnd = await state();
  check("at the outer bottom Space still reads remaining iframe content", outerEnd.uid === 100 && outerEnd.inner.top > 0, outerEnd.inner);
  check("a partly visible iframe advances by its visible area without skipping text", visibleHeight > 0 && visibleHeight < partiallyVisible.outer.height && outerEnd.inner.top <= visibleHeight * 0.9 + 1, { visibleHeight, innerStep: outerEnd.inner.top });
  await run(`const inner = document.querySelector('#mail-frame').contentDocument.scrollingElement; inner.scrollTop = inner.scrollHeight;`);
  await pressSpace();
  check("Space changes mail only after both scroll layers reach their end", (await state()).uid === 80);

  await installFixture("short");
  await open(100);
  await run(`S.searching = true; renderList(); document.querySelector('#q').focus();`);
  await pressSpace();
  const input = await run(`return { uid: S.sel?.uid, value: document.querySelector('#q').value, focus: document.activeElement.id, event: window.__readerLastKey };`);
  check("Space in a text input inserts text and preserves mail selection", input.uid === 100 && input.value === " " && input.focus === "q" && input.event.trusted, input);
  await run(`document.activeElement?.blur(); S.searching = false; S.query = ''; requestServerSearch(); render(); document.querySelector('#reader [data-action="star"]').focus();`);
  await pressSpace();
  const button = await run(`return { uid: S.sel?.uid, flagged: findMsg(S.sel).flagged, calls: window.__readerCalls.filter(c => c.command === 'execute_mail_operation' && c.args.request.action === 'flagged' && c.args.request.value === true).length, event: window.__readerLastKey };`);
  check("Space on a button keeps its native click behavior without navigating", button.uid === 100 && button.flagged && button.calls === 1 && button.event.tag === "BUTTON", button);

  await installFixture("short");
  await open(100);
  const firstHint = await state();
  const hintStyle = await run(`const hint = document.querySelector('.reader-hint'); return { pointer: getComputedStyle(hint).pointerEvents, focusable: hint.querySelectorAll('button,a,input,[tabindex]').length };`);
  check("opening mail shows one passive shortcut hint with all four labels", firstHint.hintCount === 1 && firstHint.hintOpacity === 1 && /Space.*E.*S.*R/.test(firstHint.hintText) && hintStyle.pointer === "none" && hintStyle.focusable === 0, { text: firstHint.hintText, style: hintStyle });
  await screenshot(screenshots[0]);
  await sleep(650);
  await open(90);
  const secondHint = await state();
  await run(`render(); render();`);
  const redrawHint = await state();
  check("switching mail replaces the hint and redraw does not restart its timer", secondHint.hintCount === 1 && secondHint.hintUntil > firstHint.hintUntil && redrawHint.hintCount === 1 && redrawHint.hintUntil === secondHint.hintUntil, { first: firstHint.hintUntil, second: secondHint.hintUntil, redraw: redrawHint.hintUntil });
  await sleep(Math.max(0, firstHint.hintUntil - Date.now() + 80));
  const afterOldTimer = await state();
  check("the old mail's timeout cannot remove the new mail's hint", afterOldTimer.hintCount === 1 && afterOldTimer.hintOpacity === 1, { now: afterOldTimer.now, deadline: secondHint.hintUntil });
  await sleep(Math.max(0, secondHint.hintUntil - Date.now() - 100));
  const beforeTimeout = await state();
  check("the new hint stays visible until nearly two seconds", beforeTimeout.hintCount === 1 && !beforeTimeout.hintHiding && beforeTimeout.hintOpacity === 1, { remaining: secondHint.hintUntil - beforeTimeout.now });
  await sleep(Math.max(0, secondHint.hintUntil - Date.now() + 35));
  const fading = await state();
  check("at two seconds the hint fades instead of disappearing instantly", fading.hintCount === 1 && fading.hintHiding && fading.hintOpacity > 0 && fading.hintOpacity < 1, { opacity: fading.hintOpacity, delay: fading.now - secondHint.hintUntil });
  await sleep(180);
  check("the faded hint is removed", (await state()).hintCount === 0);

  await send("Emulation.setDeviceMetricsOverride", { width: 920, height: 800, deviceScaleFactor: 1, mobile: false });
  await installFixture("short");
  await open(100);
  const narrow = await run(`
    const reader = document.querySelector('#reader'), hint = document.querySelector('.reader-hint');
    const r = reader.getBoundingClientRect(), h = hint.getBoundingClientRect();
    return { readerWidth: r.width, hintWidth: h.width, left: h.left - r.left, right: r.right - h.right,
      overflow: hint.scrollWidth - hint.clientWidth, text: hint.textContent };
  `);
  check("the hint fits a narrow reader without clipping or horizontal overflow", narrow.readerWidth <= 320 && narrow.left >= 0 && narrow.right >= 0 && narrow.overflow <= 1, narrow);
  await screenshot(screenshots[1]);
  await run(`S.folders.a1 = S.folders.a1.filter(f => !['archive','all'].includes(f.role)); S.msg.partial = true; renderReader();`);
  const narrowDisabled = await run(`
    const reader = document.querySelector('#reader'), hint = document.querySelector('.reader-hint');
    const r = reader.getBoundingClientRect(), children = [...hint.children].map(el => el.getBoundingClientRect()).filter(rect => rect.width > 0);
    return { readerWidth: r.width, overflow: hint.scrollWidth - hint.clientWidth, disabled: hint.querySelectorAll('.is-disabled').length,
      childrenFit: children.every(c => c.left >= r.left && c.right <= r.right), text: hint.textContent,
      keys: [...hint.querySelectorAll('kbd')].map(el => el.textContent) };
  `);
  check("unavailable archive and reply hints are hidden while Space and S fit the narrow reader",
    narrowDisabled.overflow <= 1 && narrowDisabled.childrenFit && narrowDisabled.disabled === 0 &&
    JSON.stringify(narrowDisabled.keys) === JSON.stringify(['Space', 'S']) && !/不可归档/.test(narrowDisabled.text), narrowDisabled);
  await screenshot(screenshots[2]);

  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
  await installFixture("long");
  const reducedEntry = await captureOpeningMotion(100);
  check("reduced motion shows the reader and hint without entrance animations or transitions",
    [reducedEntry.hint, reducedEntry.reader].every(item => item.opacity === 1 && item.animationName === "none" &&
      item.transitionDuration.split(",").every(duration => Number.parseFloat(duration) === 0) && item.animations.length === 0), reducedEntry);
  await waitFor(`S.sel?.uid === 100 && S.msg && !S.msg.partial`, "reduced-motion mail body");
  await run(`document.activeElement?.blur();`);
  const reducedBefore = await state();
  const reducedTarget = Math.min(reducedBefore.outer.total - reducedBefore.outer.height, reducedBefore.outer.top + reducedBefore.outer.height * 0.9);
  await pressSpace({ waitForMotion: false, delay: 0 });
  const reducedAfter = await state();
  check("reduced motion scrolls directly to the next page without an active animation",
    reducedAfter.uid === 100 && !reducedAfter.scrollActive && Math.abs(reducedAfter.outer.top - reducedTarget) <= 1,
    { top: reducedAfter.outer.top, target: reducedTarget, active: reducedAfter.scrollActive });
  await send("Emulation.setEmulatedMedia", { features: [] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await installFixture("short");
  await run(`
    const template = window.__readerFixture.summaries[0];
    window.__historyMail = Array.from({ length: 3338 }, (_, i) => ({
      ...template, uid: 3338 - i, uidValidity: 1, receiveEpoch: 1,
      subject: '历史邮件 ' + (3338 - i), messageId: 'history-' + i + '@example.invalid',
      date: Date.now() - i * 60000, seen: true,
    }));
    window.__readerFixture.bodies[3338] = window.__readerFixture.bodies[100];
    window.__readerFixture.summaries = window.__historyMail.slice(0, 100);
    S.lists.set('a1|INBOX', window.__readerFixture.summaries);
    visibleListSnapshot.reset(); render();
  `);
  await open(3338);
  const historyBefore = await run(`
    const rows = document.querySelector('#list .rows'); rows.scrollTop = 66 * 5 + 11;
    window.__historyAnchor = currentList()[Math.floor(rows.scrollTop / 66)].uid;
    return { top: rows.scrollTop, uid: S.sel.uid };
  `);
  await run(`
    window.__readerFixture.summaries = window.__historyMail;
    onMailEvent({ type: 'listIndex', accountId: 'a1', folder: 'INBOX' });
  `);
  await waitFor(`!S.loading.has('a1|INBOX') && S.lists.get('a1|INBOX').length === 3338`, "historical index completion");
  const historyAfter = await run(`
    const rows = document.querySelector('#list .rows');
    return { length: currentList().length, pending: visibleListPending, banner: !!document.querySelector('.list-updates'),
      top: rows.scrollTop, anchor: currentList()[Math.floor(rows.scrollTop / 66)].uid, uid: S.sel.uid,
      expectedAnchor: window.__historyAnchor, domRows: rows.querySelectorAll('.row').length };
  `);
  check("historical index completion displays 3238 older mails without a new-mail banner or viewport jump",
    historyAfter.length === 3338 && historyAfter.pending === 0 && !historyAfter.banner &&
    historyAfter.top === historyBefore.top && historyAfter.anchor === historyAfter.expectedAnchor &&
    historyAfter.uid === historyBefore.uid && historyAfter.domRows < 100, historyAfter);
  await run(`
    window.__actualNewMail = { ...window.__historyMail[0], uid: 3339, date: Date.now() + 1000, subject: '新收到的邮件', messageId: 'new@example.invalid', seen: false };
    onMailEvent({ type: 'newMail', accountId: 'a1', messages: [window.__actualNewMail] });
    onMailEvent({ type: 'newMail', accountId: 'a1', messages: [window.__actualNewMail] });
    renderList(); renderList();
  `);
  const beforeMerge = await run(`
    const rows = document.querySelector('#list .rows');
    return { length: currentList().length, pending: visibleListPending, text: document.querySelector('.list-updates')?.textContent,
      top: rows.scrollTop, uid: S.sel.uid, calls: window.__readerCalls.filter(c =>
        (c.command === 'get_message_ref' && !c.args.localOnly) ||
        (c.command === 'execute_mail_operation' && c.args.request.action === 'seen')).length };
  `);
  check("a real new-mail event is deferred once while reading and explicitly labeled as new mail",
    beforeMerge.length === 3338 && beforeMerge.pending === 1 && beforeMerge.text === '有 1 封新邮件 · 点击显示' &&
    beforeMerge.top === historyBefore.top && beforeMerge.uid === historyBefore.uid, beforeMerge);
  await run(`document.querySelector('[data-action="merge-list"]').click();`);
  const afterMerge = await run(`
    const rows = document.querySelector('#list .rows');
    return { length: currentList().length, pending: visibleListPending, banner: !!document.querySelector('.list-updates'),
      top: rows.scrollTop, anchor: currentList()[Math.floor(rows.scrollTop / 66)].uid, uid: S.sel.uid,
      seen: S.lists.get('a1|INBOX').find(m => m.uid === 3339).seen,
      calls: window.__readerCalls.filter(c =>
        (c.command === 'get_message_ref' && !c.args.localOnly) ||
        (c.command === 'execute_mail_operation' && c.args.request.action === 'seen')).length };
  `);
  check("clicking new mail merges it without marking it read or moving the current reading anchor",
    afterMerge.length === 3339 && afterMerge.pending === 0 && !afterMerge.banner &&
    afterMerge.top === historyBefore.top + 66 && afterMerge.anchor === historyAfter.anchor &&
    afterMerge.uid === historyBefore.uid && !afterMerge.seen && afterMerge.calls === beforeMerge.calls, afterMerge);
  console.log(`Screenshots: ${screenshots.join(", ")}`);
} catch (error) {
  check("browser test completed", false, error.stack || String(error));
} finally {
  await cleanup();
}

const passed = results.filter((result) => result.ok).length;
console.log(`\n${passed}/${results.length} reader browser checks passed; test Chrome closed.`);
if (passed !== results.length) process.exitCode = 1;
