// Real Chrome checks for attachment menus, nested keyboard interaction and application presentation.
// Uses ui/mock.js only. Run: node scripts/attachment-menu-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, ATTACHMENT_CDP_PORT, ATTACHMENT_APPS_FIXTURE (screenshot only).
// No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.ATTACHMENT_CDP_PORT || 9341);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-attachment-menu-"));
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
  // Stub the backend before app.js captures invoke. UI and native input are real;
  // all email/app records are fixtures, and no actual file/application is opened.
  const actualFixture = process.env.ATTACHMENT_APPS_FIXTURE;
  const screenshotApps = actualFixture ? JSON.parse(readFileSync(actualFixture, 'utf8')) : null;
  await send('Page.addScriptToEvaluateOnNewDocument', { source: `
    (() => {
      const png = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aPioAAAAASUVORK5CYII=';
      window.__attachmentApps = [
        { name: 'Google Chrome', path: '/Applications/Google Chrome.app', icon: png },
        { name: '预览', path: '/System/Applications/Preview.app', icon: png, isDefault: true },
        { name: '文本编辑', path: '/System/Applications/TextEdit.app', icon: png },
        { name: '应用图标缺失', path: '/Applications/Fallback.app', icon: null },
        { name: '损坏图标', path: '/Applications/Broken.app', icon: 'data:image/png;base64,AAAA' },
        { name: '一个用于验证长名称省略显示且不挤压菜单的应用程序', path: '/Applications/Long.app', icon: png },
        ...Array.from({ length: 20 }, (_, i) => ({ name: '其他应用 ' + (i + 1), path: '/Applications/Fixture' + i + '.app', icon: png })),
      ];
      window.__attachmentCalls = [];
      let tauri;
      Object.defineProperty(window, '__TAURI__', { configurable: true,
        get() { return tauri; },
        set(value) {
          const invoke = value.core.invoke;
          value.core.invoke = async (command, args) => {
            if (['attachment_open_with', 'open_attachment', 'open_attachment_with_app', 'save_attachment', 'reveal_attachment'].includes(command)) {
              window.__attachmentCalls.push({ command, args: structuredClone(args) });
              if (command === 'attachment_open_with') return { path: '/tmp/mock/风电润滑油专访.png', recommended: window.__attachmentApps, installed: [] };
              return '/tmp/mock/风电润滑油专访.png';
            }
            return invoke(command, args);
          };
          tauri = value;
        }
      });
    })();
  ` });
  await send('Page.navigate', { url: pathToFileURL(join(ui, 'index.html')).href });
  await waitFor("typeof S !== 'undefined' && S.accounts.length === 2 && S.folders.a1 && S.loading.size === 0", 'mock account startup');
  await run(`
    S.acct = 'a1'; S.folder = 'INBOX';
    const message = S.lists.get('a1|INBOX').find(m => m.attachment) || S.lists.get('a1|INBOX')[0];
    await openMessage(selectionOf(message));
    S.msg.subject = '测试邮件';
    S.msg.from = { name: '80654925', addr: '80654925@qq.com' };
    S.msg.to = [{ name: '我', addr: S.accounts[0].email }];
    S.msg.text = ['您好，', '', '这是一封附件菜单交互演示邮件。', '', '您可以使用默认应用打开附件，也可以选择其他应用。'].join(String.fromCharCode(10));
    S.msg.html = null; S.msg.attachments = [{ index: 0, name: '风电润滑油专访.png', size: 1500000 }];
    renderReader();
    window.__attachmentOriginalSource = JSON.parse(document.querySelector('[data-att]').dataset.attachmentSource);
  `);
  await openMenu();
  const main = await run(`
    const menu = document.querySelector('.menu:not(.menu-submenu)');
    const r = menu.getBoundingClientRect();
    return { labels: [...menu.querySelectorAll('button')].map(b => b.querySelector('.menu-label')?.textContent || b.textContent.trim()), width:r.width, height:r.height,
      submenu: !!menu.querySelector('[aria-haspopup="menu"]'), expanded: menu.querySelector('[aria-haspopup="menu"]')?.getAttribute('aria-expanded') };
  `);
  check('attachment root menu contains four compact actions', JSON.stringify(main.labels) === JSON.stringify(['打开', '打开方式', '保存到「下载」', '在 Finder 中显示']), main);
  check('root menu is compact and exposes a collapsed submenu', main.width <= 290 && main.height <= 200 && main.submenu && main.expanded === 'false', main);
  if (!main.submenu) throw new Error('The attachment root menu has no open-with submenu.');

  await click('.menu:not(.menu-submenu) [aria-haspopup="menu"]');
  await waitFor("document.querySelector('.menu-submenu')", 'open-with submenu');
  const submenu = await run(`
    const menu = document.querySelector('.menu-submenu'), buttons = [...menu.querySelectorAll('button')];
    return { role: menu.getAttribute('role'), count: buttons.length, first: buttons[0].querySelector('.menu-label')?.textContent,
      default: buttons[0].querySelector('.sub')?.textContent,
      images: [...menu.querySelectorAll('img')].filter(i => i.complete && i.naturalWidth > 0).length,
      scrollable: menu.scrollHeight > menu.clientHeight, height: menu.getBoundingClientRect().height,
      expanded: document.querySelector('[aria-haspopup="menu"]').getAttribute('aria-expanded') };
  `);
  check('submenu prioritizes default application and uses concise names', submenu.role === 'menu' && submenu.first === '预览' && submenu.default === '默认' && submenu.expanded === 'true', submenu);
  check('application choices use loaded image icons', submenu.images > 0, submenu.images);
  check('long application list stays scrollable instead of being truncated', submenu.count === 26 && submenu.scrollable && submenu.height <= 450, submenu);

  await key('ArrowLeft', 37);
  check('ArrowLeft closes child and restores its parent focus', await parentFocused());
  await key('ArrowRight', 39);
  await waitFor("document.querySelector('.menu-submenu')?.contains(document.activeElement)", 'keyboard focus inside submenu');
  const keyboard = await run(`return document.activeElement.querySelector('.menu-label')?.textContent;`);
  await key('End', 35);
  const endFocus = await run(`const menu = document.querySelector('.menu-submenu'); return { last: menu.querySelector('button:last-of-type') === document.activeElement, scroll: menu.scrollTop };`);
  await key('Home', 36);
  const homeFocus = await run(`return document.querySelector('.menu-submenu button') === document.activeElement;`);
  await key('ArrowUp', 38);
  const wrapped = await run(`return document.querySelector('.menu-submenu button:last-of-type') === document.activeElement;`);
  check('ArrowRight, Home, End and ArrowUp navigate and reveal application choices', !!keyboard && endFocus.last && homeFocus && wrapped && endFocus.scroll > 0, { keyboard, endFocus, homeFocus, wrapped });
  await key('Escape', 27);
  check('Escape dismisses only child and returns focus to open-with', await parentFocused());
  await key('Escape', 27);
  check('second Escape closes whole menu and returns focus to attachment', await run(`return !document.querySelector('.menu') && document.activeElement.matches('[data-att]');`));

  await openMenu();
  await hover('.menu:not(.menu-submenu) [aria-haspopup="menu"]');
  await waitFor("document.querySelector('.menu-submenu')", 'pointer hover submenu');
  check('pointer hover opens submenu', true);
  await hover('.menu-submenu button');
  await sleep(100);
  check('pointer can cross to submenu without dismissing it', await run(`return !!document.querySelector('.menu-submenu');`));
  await hover('.menu:not(.menu-submenu) button:first-child');
  await waitFor("!document.querySelector('.menu-submenu')", 'hover another parent item closes child');
  check('hovering a different main action closes submenu', true);

  await hover('.menu:not(.menu-submenu) [aria-haspopup="menu"]');
  await waitFor("document.querySelector('.menu-submenu')", 'pointer submenu reopen');
  {
    const fallback = await run(`
      const buttons = [...document.querySelectorAll('.menu-submenu button')];
      const missing = buttons.find(b => b.textContent.includes('应用图标缺失'));
      const broken = buttons.find(b => b.textContent.includes('损坏图标'));
      const usableFallback = b => [...b.querySelectorAll('svg')].some(svg => { const css = getComputedStyle(svg); return css.display !== 'none' && css.visibility !== 'hidden' && svg.getBoundingClientRect().width > 0; });
      return { missing: usableFallback(missing), broken: usableFallback(broken), brokenImageHidden: ![...broken.querySelectorAll('img')].some(img => getComputedStyle(img).display !== 'none' && img.getBoundingClientRect().width > 0) };
    `);
    check('missing and broken application icons use visible fallback icons', fallback.missing && fallback.broken && fallback.brokenImageHidden, fallback);
    const longName = await run(`const button = [...document.querySelectorAll('.menu-submenu button')].find(b => b.textContent.includes('一个用于')); const label = button.querySelector('.menu-label'); return { overflow: label.scrollWidth > label.clientWidth, ellipsis: getComputedStyle(label).textOverflow, title:button.title };`);
    check('long application names truncate visually and retain complete title', longName.overflow && longName.ellipsis === 'ellipsis' && longName.title.includes('应用程序'), longName);
  }
  await run(`S.sel = { accountId: 'a2', folder: 'Elsewhere', uid: 9000 };`);
  await click('.menu-submenu button');
  const invocation = await run(`return { call: window.__attachmentCalls.find(c => c.command === 'open_attachment_with_app'), source:window.__attachmentOriginalSource, closed: !document.querySelector('.menu') };`);
  check('choosing an app keeps original attachment source even after selection changes', invocation.closed && invocation.call?.args.app === '/System/Applications/Preview.app' && Object.entries(invocation.source).every(([k,v])=>invocation.call.args[k]===v), invocation);

  await run(`S.sel = { ...window.__attachmentOriginalSource };`);
  await openMenu();
  await key('ArrowDown',40); await key('ArrowRight',39);
  await waitFor("document.querySelector('.menu-submenu')?.contains(document.activeElement)", 'Enter application choice focus');
  const beforeEnter = await run(`return window.__attachmentCalls.filter(c=>c.command==='open_attachment_with_app').length;`);
  await key('Enter',13);
  const afterEnter = await run(`return { count:window.__attachmentCalls.filter(c=>c.command==='open_attachment_with_app').length, closed:!document.querySelector('.menu') };`);
  check('Enter activates the focused application exactly once and dismisses menu', afterEnter.closed && afterEnter.count===beforeEnter+1, afterEnter);

  await openMenu();
  await send('Input.dispatchKeyEvent', { type:'keyDown', key:'n', code:'KeyN', windowsVirtualKeyCode:78, modifiers:4 });
  await send('Input.dispatchKeyEvent', { type:'keyUp', key:'n', code:'KeyN', windowsVirtualKeyCode:78, modifiers:4 });
  await waitFor("document.querySelector('.compose-sheet')", 'Command-N replaces the menu with compose');
  await run(`window.__composeBeforeResize = document.querySelector('.compose-sheet');`);
  await send('Emulation.setDeviceMetricsOverride', { width:1200, height:850, deviceScaleFactor:1, mobile:false });
  await run(`await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));`);
  const composeAfterResize = await run(`return { connected:window.__composeBeforeResize.isConnected, same:document.querySelector('.compose-sheet')===window.__composeBeforeResize, menuClosed:!document.querySelector('.menu') };`);
  check('resizing after Command-N replaces a menu preserves the compose sheet', composeAfterResize.connected && composeAfterResize.same && composeAfterResize.menuClosed, composeAfterResize);
  await run(`closeOverlay();`);

  for (const [theme, width, height] of [['light', 960, 600], ['dark', 960, 600]]) {
    await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
    await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile:false });
    await run(`await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));`);
    await openMenu(true);
    await click('.menu:not(.menu-submenu) [aria-haspopup="menu"]');
    await waitFor("document.querySelector('.menu-submenu')", 'edge submenu');
    const edge = await run(`
      const parent = document.querySelector('.menu:not(.menu-submenu)'), child = document.querySelector('.menu-submenu');
      const p=parent.getBoundingClientRect(), c=child.getBoundingClientRect(), css=getComputedStyle(child);
      return { parent:{x:p.x,right:p.right,top:p.top,bottom:p.bottom}, child:{x:c.x,right:c.right,top:c.top,bottom:c.bottom}, bg:css.backgroundColor, text:css.color,
        fits: c.x>=7 && c.right<=innerWidth-7 && c.top>=7 && c.bottom<=innerHeight-7, flips:c.right<=p.left+8,
        overflow:child.scrollWidth>child.clientWidth };
    `);
    check(`${theme} submenu flips left and fits viewport at bottom-right`, edge.fits && edge.flips && !edge.overflow, edge);
    const defaultContrast = await run(`
      const menu=document.querySelector('.menu-submenu'), label=menu.querySelector('.sub');
      const color=getComputedStyle(label).color, background=getComputedStyle(menu).backgroundColor;
      const luminance=value=>value.match(/[0-9.]+/g).slice(0,3).map(Number).map(c=>{c/=255;return c<=.04045?c/12.92:((c+.055)/1.055)**2.4;}).reduce((sum,c,i)=>sum+c*[.2126,.7152,.0722][i],0);
      const a=luminance(color),b=luminance(background);
      return {color,background,ratio:(Math.max(a,b)+.05)/(Math.min(a,b)+.05)};
    `);
    check(`${theme} default application hint has readable small-text contrast`,defaultContrast.ratio>=4.5,defaultContrast);
    await key('Escape',27); await key('Escape',27);
  }

  await send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 900, deviceScaleFactor:1, mobile:false });
  await send('Emulation.setEmulatedMedia', { features:[{ name:'prefers-color-scheme', value:'light' }] });
  await run(`
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    document.querySelector('[data-att]').style.cssText = '';
    const message=findMsg(S.sel); message.subject='测试邮件';
    Object.assign(S.msg, { subject:'测试邮件', from:{name:'80654925',addr:'80654925@qq.com'}, to:[{name:'我',addr:S.accounts[0].email}],
      text:['您好，','','这是一封附件菜单交互演示邮件。','','请选择合适的应用打开附件。'].join(String.fromCharCode(10)), html:null,
      attachments:[{index:0,name:'风电润滑油专访.png',size:1500000}] });
    document.querySelector('#reader').dataset.sig=''; renderReader();
    window.__attachmentApps = ${JSON.stringify(screenshotApps)} || window.__attachmentApps.filter(a => !/其他应用|损坏|缺失|一个用于/.test(a.name));
  `);
  await openMenu();
  await hover('.menu:not(.menu-submenu) [aria-haspopup="menu"]');
  await waitFor("document.querySelector('.menu-submenu')", 'screenshot submenu');
  await run(`await Promise.all(document.getAnimations().map(animation=>animation.finished.catch(()=>{})));`);
  const clip=await run(`const r=document.querySelector('#reader').getBoundingClientRect(), menu=document.querySelector('.menu-submenu').getBoundingClientRect(); return {x:r.x,y:55,width:r.width,height:Math.min(innerHeight-55,Math.max(500,menu.bottom+30-55)),scale:1};`);
  const screenshot = await send('Page.captureScreenshot', { format:'png', clip });
  writeFileSync('/tmp/mailbeacon-attachment-menu.png', Buffer.from(screenshot.data,'base64'));
} catch (error) {
  check('browser test completes without unexpected exception', false, error.stack || String(error));
} finally {
  await cleanup();
}
console.log(`\nAttachment menu checks: ${results.filter(r=>r.ok).length}/${results.length} passed. Owned Chrome closed and temporary profile removed.`);
if (results.some(r=>!r.ok)) process.exitCode=1;

async function key(value, code) {
  await send('Input.dispatchKeyEvent', { type:'keyDown', key:value, code:value, windowsVirtualKeyCode:code, ...(value==='Enter'?{text:'\r',unmodifiedText:'\r'}:{}) });
  await send('Input.dispatchKeyEvent', { type:'keyUp', key:value, code:value, windowsVirtualKeyCode:code });
}
async function point(selector) {
  return run(`const el=document.querySelector(${JSON.stringify(selector)}); if(!el) throw new Error('Missing '+${JSON.stringify(selector)}); el.scrollIntoView({block:'nearest'}); const r=el.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2};`);
}
async function click(selector) {
  const p=await point(selector);
  await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...p});
  await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...p});
}
async function hover(selector) { await send('Input.dispatchMouseEvent',{type:'mouseMoved',...(await point(selector))}); }
async function openMenu(edge=false) {
  await run(`closeOverlay(); if (${edge}) document.querySelector('[data-att]').style.cssText='position:fixed;right:10px;bottom:10px;z-index:10';`);
  const p=await point('[data-att]');
  await send('Input.dispatchMouseEvent',{type:'mousePressed',button:'right',clickCount:1,...p});
  await send('Input.dispatchMouseEvent',{type:'mouseReleased',button:'right',clickCount:1,...p});
  await waitFor("document.querySelector('.menu')",'attachment menu');
}
async function parentFocused() {
  return run(`return !document.querySelector('.menu-submenu') && document.activeElement.matches('[aria-haspopup="menu"]') && document.activeElement.getAttribute('aria-expanded')==='false';`);
}
