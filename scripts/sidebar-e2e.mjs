// Real Chrome checks for sidebar account identity, keyboard interaction and fixed bottom controls.
// Uses ui/mock.js only. Run: node scripts/sidebar-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, SIDEBAR_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.SIDEBAR_CDP_PORT || 9337);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-sidebar-"));
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
    window.__sidebarOriginalAccounts = structuredClone(S.accounts);
    S.acct = 'all'; S.folder = 'INBOX'; S.sel = null; S.msg = null;
    S.live = { a1: { state: 'online', detail: 'IMAP IDLE connected' }, a2: { state: 'online', detail: 'Ready' } };
    render();
  `);

  for (const theme of ['light', 'dark']) {
    await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: theme }] });
    const contrast = await run(`
      const foot = document.querySelector('.side-foot');
      const css = getComputedStyle(foot), bg = getComputedStyle(document.querySelector('#sidebar')).backgroundColor;
      const channels = color => color.match(/[\\d.]+/g).slice(0, 3).map(Number);
      const luminance = color => channels(color).map(c => { c /= 255; return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4; }).reduce((total, c, i) => total + c * [.2126, .7152, .0722][i], 0);
      const a = luminance(css.color), b = luminance(bg);
      return { foreground: css.color, background: bg, fontSize: css.fontSize, ratio: (Math.max(a, b) + .05) / (Math.min(a, b) + .05) };
    `);
    check(`${theme} small status text has at least 4.5:1 contrast`, contrast.ratio >= 4.5, contrast);
  }

  const identities = await run(`return [...document.querySelectorAll('#sidebar .account-badge')].map(el => el.dataset.provider);`);
  check("sidebar identifies aggregate, Gmail and enterprise accounts", JSON.stringify(identities) === JSON.stringify(['all', 'gmail', 'exmail']), identities);

  await click('[data-action="account-menu"]');
  const picker = await run(`return [...document.querySelectorAll('.menu [role="menuitemradio"]')].map(el => ({ label: el.textContent.trim(), selected: el.getAttribute('aria-checked'), styled: el.classList.contains('is-selected'), provider: el.querySelector('.account-badge')?.dataset.provider }));`);
  check("account picker exposes radio selection and identity for every choice", picker.length === 3 && picker[0].selected === 'true' && picker[0].styled && picker.slice(1).every(item => item.selected === 'false') && picker.map(item => item.provider).join(',') === 'all,gmail,exmail', picker);
  await key('ArrowDown', 40);
  const focusAfterDown = await run(`return document.activeElement?.textContent.trim();`);
  await key('Enter', 13);
  await waitFor("S.acct === 'a1' && !document.querySelector('.menu')", "keyboard account switch");
  check("ArrowDown then Enter activates the focused account", focusAfterDown === 'jarvis@gmail.com' && await run("return S.acct === 'a1' && S.folder === 'INBOX';"), focusAfterDown);

  await click('[data-action="account-menu"]');
  const selected = await run(`return [...document.querySelectorAll('.menu [aria-checked="true"]')].map(el => el.textContent.trim());`);
  check("reopening the picker reflects the newly selected account", selected.length === 1 && selected[0] === 'jarvis@gmail.com', selected);
  await key('End', 35);
  const endFocus = await run(`return document.activeElement.textContent.trim();`);
  await key('Home', 36);
  const homeFocus = await run(`return document.activeElement.textContent.trim();`);
  await key('ArrowUp', 38);
  const wrapFocus = await run(`return document.activeElement.textContent.trim();`);
  check("Home, End and ArrowUp navigate across account menu actions", endFocus === '添加邮箱…' && homeFocus === '所有账户' && wrapFocus === endFocus, { endFocus, homeFocus, wrapFocus });
  // Background mail refresh replaces the account trigger while this menu stays open.
  await run(`await loadList('a1', 'INBOX', { force: true });`);
  await key('Escape', 27);
  check("Escape dismisses account menu and returns focus to its trigger", await run(`return !document.querySelector('.menu') && document.activeElement?.matches('[data-action="account-menu"]');`));

  await click('[data-action="account-menu"]');
  await click('.menu [data-mi="2"]');
  await waitFor("S.acct === 'a2' && !document.querySelector('.menu')", "pointer account switch");
  check("pointer selection opens the enterprise inbox", await run("return S.acct === 'a2' && S.folder === 'INBOX';"));

  const colors = await run(`
    S.acct = 'all'; render();
    return S.accounts.map(a => {
      const nav = document.querySelector('[data-switch="' + a.id + '"] .account-color');
      const row = [...document.querySelectorAll('[data-open]')].find(el => el.dataset.open.startsWith(a.id + '|'))?.querySelector('.acct-dot');
      return { email: a.email, badge: nav ? getComputedStyle(nav).backgroundColor : null, source: row ? getComputedStyle(row).backgroundColor : null };
    });
  `);
  check("account badge colors match the corresponding aggregate mail source", colors.length === 2 && colors.every(item => item.badge && item.badge === item.source), colors);

  for (const [state, expectedText, expectedClass] of [
    ['online', '已同步 · 实时推送', ''], ['connecting', '正在同步…', 'connecting'],
    ['error', 'jarvis@gmail.com 登录失败', 'error'], ['offline', '网络断开，正在重连…', 'offline'],
  ]) {
    const status = await run(`
      S.live.a1 = { state: ${JSON.stringify(state)}, detail: 'Server detail with <angle> and "quotes" END' };
      renderSidebar();
      const foot = document.querySelector('.side-foot');
      return { text: foot.textContent.trim(), title: foot.title, dot: foot.querySelector('.dot').className };
    `);
    check(`${state} status keeps its text, indicator and complete details`, status.text === expectedText && status.title.includes(expectedText) && status.title.includes('jarvis@gmail.com：Server detail with <angle> and "quotes" END') && status.title.includes('li@company.com：Ready') && status.dot.trim() === `dot ${expectedClass}`.trim(), status);
  }

  await run(`
    const bad = S.accounts[0];
    bad.email = 'very.long.account.address.'.repeat(8) + '"<&@example.test';
    bad.name = '<img src=x onerror="window.__sidebarInjected=true">';
    bad.kind = '"><svg onload="window.__sidebarInjected=true">';
    S.live.a1 = { state: 'error', detail: 'Authentication failed: '.repeat(30) + 'FINAL DETAIL' };
    S.acct = 'a1'; S.folder = 'INBOX';
    S.folders.a1 = Array.from({ length: 45 }, (_, i) => ({ path: 'Folder' + i, name: '长文件夹 ' + i, role: 'other' }));
    renderSidebar();
  `);
  const escaped = await run(`return { provider: document.querySelector('.account-card .account-badge')?.dataset.provider, email: document.querySelector('.account-card .mail').textContent, name: document.querySelector('.account-card .name').textContent, injected: !!window.__sidebarInjected || !!document.querySelector('#sidebar img'), title: document.querySelector('.side-foot').title };`);
  check("unknown provider and untrusted account text render safely without losing full titles", escaped.provider === 'custom' && !escaped.injected && escaped.name.startsWith('<img') && escaped.email.includes('"<&@example.test') && escaped.title.endsWith('li@company.com：Ready') && escaped.title.includes('FINAL DETAIL'), escaped.provider);

  await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 520, deviceScaleFactor: 1, mobile: false });
  // Resizing can schedule a sidebar render; measure its current nodes after it settles.
  await run(`await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));`);
  const fixed = await run(`
    const scroll = document.querySelector('.side-scroll');
    const settings = document.querySelector('#sidebar [data-action="settings"]');
    const status = document.querySelector('.side-foot');
    const bottom = document.querySelector('.side-bottom');
    const before = settings.getBoundingClientRect();
    scroll.scrollTop = scroll.scrollHeight;
    // scrollTop and layout reads are synchronous; do not yield to unrelated mock renders.
    const after = settings.getBoundingClientRect();
    const sr = document.querySelector('#sidebar').getBoundingClientRect();
    const tr = status.getBoundingClientRect();
    const text = status.querySelector('.sync-text');
    return { outside: !scroll.contains(settings), shared: !!bottom && bottom.contains(settings) && bottom.contains(status), scroll: scroll.scrollTop, topBefore: before.top, topAfter: after.top, visible: after.top >= 0 && after.bottom <= innerHeight && after.left >= sr.left && after.right <= sr.right, noOverlap: after.right <= tr.left && tr.right <= sr.right, settingsWidth: after.width, truncated: !!text && text.scrollWidth > text.clientWidth, noSidebarOverflow: document.querySelector('#sidebar').scrollWidth <= document.querySelector('#sidebar').clientWidth };
  `);
  check("settings and status share the fixed area outside overflowing folders", fixed.outside && fixed.shared && fixed.scroll > 0 && Math.abs(fixed.topBefore - fixed.topAfter) < 1 && fixed.visible, fixed);
  check("long failing account cannot squeeze settings away or overflow the narrow sidebar", fixed.settingsWidth >= 60 && fixed.noOverlap && fixed.truncated && fixed.noSidebarOverflow, fixed);
  await click('#sidebar [data-action="settings"]');
  await waitFor("document.querySelector('.sheet .settings-body')", "settings opened from fixed bottom button");
  check("fixed settings button still opens settings after navigation scrolls", await run(`return !!document.querySelector('.sheet .settings-body');`));
  await run("closeOverlay();");

  // Three stable account identities exercise color allocation separately from
  // sender avatars, including the same identity rendered in four locations.
  await run(`
    S.accounts = [...structuredClone(window.__sidebarOriginalAccounts), { id: 'a3', kind: 'qq', name: 'Third account', email: 'third@example.test' }];
    S.live.a3 = { state: 'online', detail: 'Fixture' };
    S.folders.a3 = [{ path: 'INBOX', name: '收件箱', role: 'inbox' }];
    const first = S.lists.get('a1|INBOX')[0];
    S.lists.set('a3|INBOX', [{ ...first, accountId: 'a3', uid: 99991, subject: 'Third account fixture' }]);
    S.acct = 'all'; S.folder = 'INBOX'; render();
  `);
  const accountColors = await badgeColors();
  check('small account dots retain at least 7px of visible color inside their borders', accountColors.length === 3 && accountColors.every(item => item.fill >= 7), accountColors.map(({ id, fill }) => ({ id, fill })));
  const hues = accountColors.map(item => hue(item.color));
  const hueGaps = hues.flatMap((a, i) => hues.slice(i + 1).map(b => Math.min(Math.abs(a - b), 360 - Math.abs(a - b))));
  check('the first three accounts have distinct colors and clearly separated hues', new Set(accountColors.map(item => item.color)).size === 3 && hueGaps.every(gap => gap >= 60), { colors: accountColors.map(item => item.color), hues, minimumGap: Math.min(...hueGaps) });
  for (const theme of ['light', 'dark']) {
    await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    const values = await badgeColors();
    const contrast = values.map(item => ({ id: item.id, ratio: contrastRatio(item.color, item.background) }));
    check(`${theme} account dots have at least 3:1 contrast against their badge`, contrast.length === 3 && contrast.every(item => item.ratio >= 3), contrast);
  }
  const surfaces = await run(`
    const color = el => el ? getComputedStyle(el).backgroundColor : null;
    return S.accounts.map(a => ({ id: a.id,
      sidebar: color(document.querySelector('[data-switch="' + a.id + '"] .account-color')),
      row: color([...document.querySelectorAll('[data-open]')].find(el => el.dataset.open.startsWith(a.id + '|'))?.querySelector('.acct-dot')),
    }));
  `);
  await click('[data-action="account-menu"]');
  const pickerColors = await run(`return S.accounts.map(a => ({ id: a.id, color: getComputedStyle([...document.querySelectorAll('.menu [role="menuitemradio"]')].find(el => el.title === a.email).querySelector('.account-color')).backgroundColor }));`);
  await run(`closeOverlay(); openSettings();`);
  const settingsColors = await run(`return S.accounts.map(a => ({ id: a.id, color: getComputedStyle(document.querySelector('[data-settings-account="' + a.id + '"] .account-color')).backgroundColor }));`);
  check('all three account colors agree across sidebar, picker, settings and aggregate rows', surfaces.every(item => item.sidebar && item.sidebar === item.row && item.sidebar === pickerColors.find(a => a.id === item.id)?.color && item.sidebar === settingsColors.find(a => a.id === item.id)?.color), surfaces);
  await run(`
    closeOverlay(); S.accounts.reverse();
    S.accounts.find(a => a.id === 'a1').email = 'renamed.account@example.test';
    renderSidebar();
  `);
  const reordered = await badgeColors();
  check('account identity keeps its assigned color after reordering and an address update', reordered.every(item => item.color === surfaces.find(before => before.id === item.id)?.sidebar), reordered.map(({ id, color }) => ({ id, color })));
  const selectedSource = await run(`
    const message = S.lists.get('a1|INBOX')[0];
    S.sel = { accountId: message.accountId, folder: message.folder, uid: message.uid };
    renderList();
    const dot = document.querySelector('.row.sel .acct-dot');
    const css = dot && getComputedStyle(dot);
    return { color: css?.backgroundColor, ring: css?.boxShadow, width: css && parseFloat(css.width) };
  `);
  check('selected aggregate mail keeps its account color visible inside a white isolation ring', selectedSource.color === surfaces.find(item => item.id === 'a1')?.sidebar && selectedSource.width >= 8 && selectedSource.ring?.includes('rgb(255, 255, 255)') && /0px 0px 0px [1-9][0-9.]*px/.test(selectedSource.ring), selectedSource);
  const expandedColors = await run(`
    const previousAccounts = S.accounts;
    try {
      S.accounts = Array.from({ length: 16 }, (_, i) => ({ id: 'a' + (i + 1), kind: 'custom', name: 'Account ' + (i + 1), email: 'account' + (i + 1) + '@example.test' }));
      renderSidebar();
      return S.accounts.map(a => ({ id: a.id, color: getComputedStyle(document.querySelector('[data-switch="' + a.id + '"] .account-color')).backgroundColor }));
    } finally {
      S.accounts = previousAccounts;
      renderSidebar();
    }
  `);
  const expandedPairs = expandedColors.flatMap((a, i) => expandedColors.slice(i + 1).map(b => ({
    accounts: [a.id, b.id], colors: [a.color, b.color],
    distance: Math.hypot(...channels(a.color).map((value, channel) => value - channels(b.color)[channel])),
  })));
  const closestPair = expandedPairs.reduce((closest, pair) => pair.distance < closest.distance ? pair : closest);
  check('sixteen accounts never receive near-identical rendered source colors', expandedColors.length === 16 && expandedPairs.every(pair => pair.distance >= 45), { minimumRgbDistance: closestPair.distance, accounts: closestPair.accounts, colors: closestPair.colors });
  await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });
  await run(`
    S.accounts = [...structuredClone(window.__sidebarOriginalAccounts), { id: 'a3', kind: 'qq', name: 'Third account', email: 'third@example.test' }];
    S.live = Object.fromEntries(S.accounts.map(a => [a.id, { state: 'online', detail: 'Mock account ready' }]));
    S.acct = 'all'; S.folder = 'INBOX'; S.sel = null; S.msg = null;
    render();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  `);
  const crop = await run(`
    const sidebar = document.querySelector('#sidebar').getBoundingClientRect();
    const accounts = [...document.querySelectorAll('#sidebar .account-nav')];
    const first = accounts[0].getBoundingClientRect(), last = accounts.at(-1).getBoundingClientRect();
    const y = Math.max(0, first.top - 35);
    return { x: sidebar.x, y, width: sidebar.width, height: last.bottom + 35 - y, scale: 1 };
  `);
  const screenshot = await send('Page.captureScreenshot', { format: 'png', clip: crop });
  writeFileSync('/tmp/mailbeacon-account-colors.png', Buffer.from(screenshot.data, 'base64'));

} catch (error) {
  check('browser test completes without an unexpected exception', false, error.stack || String(error));
} finally {
  await cleanup();
}

console.log(`\nSidebar checks: ${results.filter(result => result.ok).length}/${results.length} passed. Owned Chrome closed and temporary profile removed.`);
if (results.some(result => !result.ok)) process.exitCode = 1;

async function key(value, code) {
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: value, code: value, windowsVirtualKeyCode: code, ...(value === 'Enter' ? { text: '\r', unmodifiedText: '\r' } : {}) });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: value, code: value, windowsVirtualKeyCode: code });
}

async function badgeColors() {
  return run(`return S.accounts.map(a => {
    const dot = document.querySelector('[data-switch="' + a.id + '"] .account-color'), css = getComputedStyle(dot);
    return { id: a.id, color: css.backgroundColor, background: getComputedStyle(dot.closest('.account-badge')).backgroundColor, fill: parseFloat(css.width) - parseFloat(css.borderLeftWidth) - parseFloat(css.borderRightWidth) };
  });`);
}

function channels(color) {
  return color.match(/[0-9.]+/g).slice(0, 3).map(Number);
}

function hue(color) {
  const [r, g, b] = channels(color).map(c => c / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b), delta = max - min;
  if (!delta) return 0;
  const h = max === r ? (g - b) / delta : max === g ? (b - r) / delta + 2 : (r - g) / delta + 4;
  return (h * 60 + 360) % 360;
}

function contrastRatio(foreground, background) {
  const luminance = color => channels(color).map(c => { c /= 255; return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4; }).reduce((sum, c, i) => sum + c * [.2126, .7152, .0722][i], 0);
  const a = luminance(foreground), b = luminance(background);
  return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
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
