// Real Chrome checks for custom dropdown consistency and real form interactions.
// Uses ui/mock.js only. Run: node scripts/dropdown-e2e.mjs [ui-directory]
// Optional: CHROME_BIN, DROPDOWN_CDP_PORT. No npm dependencies are required.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ui = resolve(process.argv[2] || join(here, "../ui"));
const port = Number(process.env.DROPDOWN_CDP_PORT || 9338);
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
  profile = mkdtempSync(join(tmpdir(), "mailbeacon-dropdown-"));
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
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-reduced-motion", value: "no-preference" }, { name: "prefers-color-scheme", value: "light" }] });
  await send("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__dropdownCalls = [];
    let tauri;
    Object.defineProperty(window, '__TAURI__', { configurable: true, get: () => tauri, set(value) {
      const original = value.core.invoke;
      value.core.invoke = async (command, args) => {
        window.__dropdownCalls.push(command);
        if (['send_mail', 'schedule_mail', 'outbox_send_now'].includes(command)) throw new Error('Dropdown checks must not send mail');
        return original(command, args);
      };
      tauri = value;
    } });
  ` });
  await send("Page.navigate", { url: pathToFileURL(join(ui, "index.html")).href });
  await waitFor("typeof S !== 'undefined' && S.accounts.length === 2 && S.folders.a1 && S.folders.a2 && S.loading.size === 0", "mock accounts and initial lists");
  await run("openSettings(true);");
  const customReady = await run(`
    const native = document.querySelector('#receive-protocol'), trigger = native.nextElementSibling;
    return !!trigger?.matches('button.select-trigger[role="combobox"]') && native.classList.contains('select-native') && native.tabIndex === -1 && native.getAttribute('aria-hidden') === 'true';
  `);
  check('native form value is represented by an accessible custom dropdown trigger', customReady);
  if (!customReady) throw new Error('Custom dropdown has not been mounted');
  const protocol = await run(`
    const native = document.querySelector('#receive-protocol'), el = native.nextElementSibling;
    const css = getComputedStyle(native);
    return { expanded: el.getAttribute('aria-expanded'), text: el.textContent.trim(), hiddenNative: (css.clipPath !== 'none' && css.pointerEvents === 'none') || css.display === 'none' };
  `);
  check('receive protocol starts collapsed with its selected label and no visible native control', protocol.expanded === 'false' && protocol.text.includes('IMAP') && protocol.hiddenNative, protocol);
  await styles('add-account form', '#receive-protocol');
  await focusCheck('receive protocol', '#receive-protocol');
  await click('#receive-protocol + .select-trigger');
  const protocolPopup = await run(`
    const item = document.querySelector('.select-option[aria-selected="true"]'), title = item.querySelector('.select-title'), detail = item.querySelector('.select-detail'), mark = item.querySelector('svg');
    return { title: title?.textContent, detail: detail?.textContent, layered: !!detail && parseFloat(getComputedStyle(title).fontSize) > parseFloat(getComputedStyle(detail).fontSize), markOnRight: !!mark && getComputedStyle(mark).visibility === 'visible' && mark.getBoundingClientRect().left >= item.querySelector('.select-text').getBoundingClientRect().right };
  `);
  const hoverPoint = await run(`const r = document.querySelector('.select-option[data-index="1"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 };`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...hoverPoint });
  const hover = await run(`const el = document.querySelector('.select-option[data-index="1"]'), rgb = getComputedStyle(el).backgroundColor.match(/[0-9.]+/g).map(Number); return { active: el.classList.contains('is-active'), neutral: Math.max(...rgb.slice(0, 3)) - Math.min(...rgb.slice(0, 3)) < 24 };`);
  check('protocol popup separates main and supporting text, places its check on the right and uses neutral hover', protocolPopup.title === 'IMAP' && !!protocolPopup.detail && protocolPopup.layered && protocolPopup.markOnRight && hover.active && hover.neutral, { ...protocolPopup, hover });
  await key('Escape', 27);
  await run(`closeOverlay(); openCompose();`);
  await click('#c-from + .select-trigger');
  const modalPopup = await run(`const trigger = document.querySelector('#c-from').nextElementSibling, dialog = trigger.closest('[role="dialog"][aria-modal="true"]'), popup = document.querySelector('.select-popover'); return { dialog: !!dialog, popup: !!popup, owned: !!dialog && !!popup && dialog.contains(popup) };`);
  check('sender options stay inside the active modal dialog for assistive technology', modalPopup.dialog && modalPopup.popup && modalPopup.owned, modalPopup);
  await run(`closeOverlay(); openSettings(true);`);
  await customKeyboardChecks();

  await run(`
    document.querySelector('[data-kind="custom"]').click();
    document.querySelector('#acct-form [name="email"]').value = 'dropdown@example.test';
    document.querySelector('#acct-form [name="name"]').value = 'Dropdown User';
    window.__dropdownAccountSeed = { password: crypto.randomUUID(), smtpHost: 'smtp.dropdown.example.test', smtpPort: '587' };
    for (const [name, value] of Object.entries(window.__dropdownAccountSeed)) document.querySelector('#acct-form').elements.namedItem(name).value = value;
    window.__dropdownOldProtocol = document.querySelector('#receive-protocol');
    document.querySelector('#receive-protocol').focus();
  `);
  await select('#receive-protocol', 'pop3');
  const pop3 = await run(`return { value: document.querySelector('#receive-protocol').value, pop: !!document.querySelector('[name="pop3Host"]'), imap: !!document.querySelector('[name="imapHost"]'), email: document.querySelector('#acct-form [name="email"]').value, name: document.querySelector('#acct-form [name="name"]').value };`);
  check('selecting POP3 updates server fields and retains account identity', pop3.value === 'pop3' && pop3.pop && !pop3.imap && pop3.email === 'dropdown@example.test' && pop3.name === 'Dropdown User', pop3);
  await protocolRetention('switching to POP3');
  await run(`window.__dropdownOldProtocol = document.querySelector('#receive-protocol');`);
  await select('#receive-protocol', 'imap');
  check('switching back to IMAP restores its server fields', await run(`return document.querySelector('#receive-protocol').value === 'imap' && !!document.querySelector('[name="imapHost"]') && !document.querySelector('[name="pop3Host"]') && document.querySelector('#acct-form [name="email"]').value === 'dropdown@example.test';`));
  await protocolRetention('switching back to IMAP');

  await run(`closeOverlay(); S.acct = 'a1'; openSettings();`);
  await styles('notification and download forms', '#prefs-content, #download-filter-a1, #download-scope-a1');
  await select('#prefs-content', 'sender');
  await waitFor("!S.prefsSaving && S.prefs.notifyContent === 'sender'", 'notification preference saved to mock');
  check('notification content dropdown updates the preference', await run(`return document.querySelector('#prefs-content').value === 'sender' && currentPrefs().notifyContent === 'sender';`));
  await run(`document.querySelector('.prefs-form [name="notify"]').click();`);
  await waitFor("document.querySelector('#prefs-content').disabled", 'notification content disabled');
  await disabledCheck('notification content', '#prefs-content');
  await run(`document.querySelector('.prefs-form [name="notify"]').click();`);
  await waitFor("!document.querySelector('#prefs-content').disabled", 'notification content re-enabled');

  await select('#download-filter-a1', 'none');
  await disabledCheck('download matching mode without a filter', '#download-scope-a1');
  await select('#download-filter-a1', 'whitelist');
  await select('#download-scope-a1', 'fields');
  check('download filtering enables field matching and displays its relevant controls', await run(`return !document.querySelector('#download-scope-a1').disabled && document.querySelector('#download-scope-a1').value === 'fields' && !document.querySelector('[data-auto-download-account="a1"] .auto-download-field-keywords').hidden && document.querySelector('[data-auto-download-account="a1"] .auto-download-any-keywords').hidden;`));
  await select('#download-filter-a1', 'none');
  check('turning filtering off disables matching and all keyword fields', await run(`return document.querySelector('#download-scope-a1').disabled && [...document.querySelectorAll('[data-auto-download-account="a1"] textarea')].every(el => el.disabled);`));

  await run(`closeOverlay(); await openRules();`);
  await styles('rule execution account', '#rules-run-account');
  await select('#rules-run-account', 'a2');
  check('rule execution account can be selected without executing rules', await run(`return document.querySelector('#rules-run-account').value === 'a2' && !window.__dropdownCalls.includes('run_rules');`));
  await click('[data-action="rule-new"]');
  await styles('rule editor', '.rule-form select');
  await disabledCheck('rule move target before enabling the action', '.rule-form [name="moveTo"]');
  await select('#rule-account', 'a2');
  await select('#rule-match', 'any');
  await select('.rule-cond [name="field"]', 'attachment');
  await select('.rule-cond [name="op"]', 'notContains');
  check('rule dropdown changes preserve account, match mode and attachment-specific operators', await run(`return S.rulesUi.editing.rule.accountId === 'a2' && !S.rulesUi.editing.rule.matchAll && S.rulesUi.editing.rule.conditions[0].field === 'attachment' && S.rulesUi.editing.rule.conditions[0].op === 'notContains' && !document.querySelector('.rule-cond [name="value"]') && document.querySelector('.rule-cond [name="op"]').options.length === 2;`));
  await click('[data-action="rule-cond-add"]');
  await run(`window.__dropdownOldCondition = document.querySelector('.rule-cond[data-cond-index="1"] [name="field"]'); window.__dropdownOldCondition.focus();`);
  await select('.rule-cond[data-cond-index="1"] [name="field"]', 'attachment');
  const ruleFocus = await run(`
    const current = document.querySelector('.rule-cond[data-cond-index="1"] [name="field"]');
    return { replaced: current !== window.__dropdownOldCondition, oldDetached: !window.__dropdownOldCondition.isConnected, focused: document.activeElement === current.nextElementSibling, value: current.value, firstValue: S.rulesUi.editing.rule.conditions[0].field, activeId: document.activeElement.id, activeTag: document.activeElement.tagName };
  `);
  check('changing the second rule condition restores focus to that row after rerender', ruleFocus.replaced && ruleFocus.oldDetached && ruleFocus.focused && ruleFocus.value === 'attachment' && ruleFocus.firstValue === 'attachment', ruleFocus);
  await run(`document.querySelector('.rule-form [name="move"]').click();`);
  const folderValue = await run(`return [...document.querySelector('.rule-form [name="moveTo"]').options].find(option => option.value)?.value;`);
  if (!folderValue) throw new Error('No mock target folder available');
  await select('.rule-form [name="moveTo"]', folderValue);
  check('enabling move permits an explicit target folder selection', await run(`return !document.querySelector('.rule-form [name="moveTo"]').disabled && S.rulesUi.editing.rule.actions.moveTo === ${JSON.stringify(folderValue)};`));

  await run(`closeOverlay(); openCreateFolder();`);
  await styles('new-folder account', '#folder-account');
  await select('#folder-account', 'a2');
  check('new-folder account selection changes without creating a folder', await run(`return document.querySelector('#folder-account').value === 'a2' && !window.__dropdownCalls.includes('create_folder');`));

  await run(`closeOverlay(); S.acct = 'a1'; openCompose();`);
  await styles('compose sender', '#c-from');
  await styles('compact font and size controls', '.compose-toolbar select', true);
  await focusCheck('compose sender', '#c-from');
  await select('#c-from', 'a2');
  check('sender selection switches to the matching account signature', await run(`return document.querySelector('#c-from').value === 'a2' && document.querySelector('#c-body').textContent.includes('李嘉文 | 市场部') && !document.querySelector('#c-body').textContent.includes('Product · MailBeacon');`));
  await run(`
    const body = document.querySelector('#c-body'); body.innerHTML = '<p id="dropdown-sample">Dropdown formatting</p>';
    body.focus(); const range = document.createRange(); range.selectNodeContents(document.querySelector('#dropdown-sample'));
    getSelection().removeAllRanges(); getSelection().addRange(range); rememberSelection();
  `);
  await select('[data-fmt="font"]', 'Arial, Helvetica, sans-serif');
  const font = await run(`const sample = document.querySelector('#dropdown-sample'); return { html: sample.innerHTML, text: sample.textContent, family: getComputedStyle(sample.querySelector('span, font') || sample).fontFamily };`);
  check('font dropdown applies formatting to the selected message text', font.text === 'Dropdown formatting' && /Arial/.test(font.family), font);
  await run(`
    const body = document.querySelector('#c-body'); body.focus();
    const range = document.createRange(); range.selectNodeContents(document.querySelector('#dropdown-sample'));
    getSelection().removeAllRanges(); getSelection().addRange(range); rememberSelection();
  `);
  await run(`document.querySelector('[data-fmt="size"]').nextElementSibling.focus();`);
  await key(' ', 32);
  await key('Home', 36);
  await key('ArrowDown', 40);
  await key('ArrowDown', 40);
  await key('ArrowDown', 40);
  await key('Enter', 13);
  const size = await run(`const sample = document.querySelector('#dropdown-sample'); const nodes = [sample, ...sample.querySelectorAll('*')]; return { text: sample.textContent, sizes: nodes.map(el => parseFloat(getComputedStyle(el).fontSize)), html: sample.innerHTML };`);
  check('keyboard size selection enlarges the selected message text without losing it', size.text === 'Dropdown formatting' && Math.max(...size.sizes) > 14, size);

  // A long sender option must fit the compose form rather than widening its sheet.
  await run(`closeOverlay(); S.accounts[0].email = 'very.long.email.account.'.repeat(12) + '@example.test'; openCompose();`);
  await send("Emulation.setDeviceMetricsOverride", { width: 900, height: 700, deviceScaleFactor: 1, mobile: false });
  const longSender = await layout('#c-from');
  check('long sender address stays inside the compose field at a narrow width', longSender.inside && longSender.parentFitsSheet && longSender.width > 100, longSender);
  await run(`closeOverlay(); openCreateFolder();`);
  const longFolder = await layout('#folder-account');
  check('long account address stays inside the new-folder form', longFolder.inside && longFolder.parentFitsSheet, longFolder);
  await focusCheck('new-folder account', '#folder-account');
  const light = await appearance('#folder-account', 'light');
  const dark = await appearance('#folder-account', 'dark');
  check('dropdown text, background and custom arrow adapt to dark mode', light.color !== dark.color && light.background !== dark.background && light.arrow !== dark.arrow && dark.arrow !== 'none', { light, dark });
  await click('#folder-account + .select-trigger');
  const popupDark = await run(`const css = getComputedStyle(document.querySelector('.select-popover')); return { background: css.backgroundColor, text: css.color };`);
  await appearance('#folder-account', 'light');
  const popupLight = await run(`const css = getComputedStyle(document.querySelector('.select-popover')); return { background: css.backgroundColor, text: css.color };`);
  check('the open custom listbox also adapts its surface and text to dark mode', popupDark.background !== popupLight.background && popupDark.text !== popupLight.text, { dark: popupDark, light: popupLight });
  await key('Escape', 27);
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }, { name: 'prefers-color-scheme', value: 'dark' }] });
  const reducedSelect = await run(`return getComputedStyle(document.querySelector('#folder-account').nextElementSibling).transitionDuration;`);
  check('reduced motion disables dropdown transitions', zeroDurations(reducedSelect), reducedSelect);
  await click('#folder-account + .select-trigger');
  const reducedPopup = await run(`const el = document.querySelector('.select-popover'), css = getComputedStyle(el); return { name: css.animationName, running: el.getAnimations().some(animation => animation.playState === 'running') };`);
  check('reduced motion also disables custom popup entrance animation', reducedPopup.name === 'none' && !reducedPopup.running, reducedPopup);
  await key('Escape', 27);
  await run(`closeOverlay(); accountMenu(document.querySelector('[data-action="account-menu"]'));`);
  const reducedMenu = await run(`return [...document.querySelectorAll('.menu .account-choice')].map(el => getComputedStyle(el).transitionDuration);`);
  check('reduced motion disables account menu transitions', reducedMenu.length > 0 && reducedMenu.every(zeroDurations), reducedMenu);

  await run(`
    closeOverlay(); window.__dropdownMenuActions = [];
    window.__dropdownLongLabel = 'A long account and folder action '.repeat(12) + 'END';
    showMenu(document.querySelector('[data-action="account-menu"]'), [
      { label: window.__dropdownLongLabel, icon: 'mail', sub: '⌘L', run: () => window.__dropdownMenuActions.push('long') },
      { label: 'Disabled archive action', icon: 'archive', disabled: true, run: () => window.__dropdownMenuActions.push('disabled') },
      { label: 'Available action', icon: 'star', run: () => window.__dropdownMenuActions.push('available') },
    ]);
  `);
  const menuLayout = await run(`
    const menu = document.querySelector('.menu'), button = menu.querySelector('[data-mi="0"]'), label = button.querySelector('.menu-label'), shortcut = button.querySelector('.sub');
    const r = menu.getBoundingClientRect(), br = button.getBoundingClientRect(), sr = shortcut.getBoundingClientRect();
    return { bounded: r.left >= 0 && r.right <= innerWidth && menu.scrollWidth <= menu.clientWidth, title: button.title === window.__dropdownLongLabel, ellipsis: !!label && label.scrollWidth > label.clientWidth && getComputedStyle(label).textOverflow === 'ellipsis', shortcutVisible: sr.width > 0 && sr.right <= br.right };
  `);
  check('long menu labels truncate within the popup while preserving title and shortcut', menuLayout.bounded && menuLayout.title && menuLayout.ellipsis && menuLayout.shortcutVisible, menuLayout);
  const disabledBefore = await run(`const el = document.querySelector('.menu [data-mi="1"]'); return { color: getComputedStyle(el.querySelector('svg')).color, background: getComputedStyle(el).backgroundColor };`);
  const disabledPoint = await run(`const r = document.querySelector('.menu [data-mi="1"]').getBoundingClientRect(); return { x: r.x + r.width / 2, y: r.y + r.height / 2 };`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...disabledPoint });
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...disabledPoint });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...disabledPoint });
  const disabledAfter = await run(`const el = document.querySelector('.menu [data-mi="1"]'); return { color: getComputedStyle(el.querySelector('svg')).color, background: getComputedStyle(el).backgroundColor, actions: window.__dropdownMenuActions.length };`);
  check('hovering or clicking a disabled menu action keeps its disabled appearance and does not run it', disabledAfter.color === disabledBefore.color && disabledAfter.background === disabledBefore.background && disabledAfter.actions === 0, { before: disabledBefore, after: disabledAfter });
  await run(`document.querySelector('.menu [data-mi="0"]').focus();`);
  await key('ArrowDown', 40);
  const keyboardTarget = await run(`return document.activeElement?.dataset.mi;`);
  await key('Enter', 13);
  check('menu keyboard navigation skips disabled actions and activates the next available action', keyboardTarget === '2' && await run(`return !document.querySelector('.menu') && window.__dropdownMenuActions.join(',') === 'available';`), keyboardTarget);
  check('dropdown checks never invoke send, schedule, rule execution or folder creation', await run(`return !window.__dropdownCalls.some(command => ['send_mail', 'schedule_mail', 'outbox_send_now', 'run_rules', 'create_folder'].includes(command));`));
} catch (error) {
  check('browser test completes without an unexpected exception', false, error.stack || String(error));
} finally {
  await cleanup();
}

console.log(`\nDropdown checks: ${results.filter(result => result.ok).length}/${results.length} passed. Owned Chrome closed and temporary profile removed.`);
if (results.some(result => !result.ok)) process.exitCode = 1;

async function key(value, code) {
  const physical = value === ' ' ? 'Space' : value;
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: value, code: physical, windowsVirtualKeyCode: code, ...(value === 'Enter' ? { text: '\r', unmodifiedText: '\r' } : value === ' ' ? { text: ' ', unmodifiedText: ' ' } : {}) });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: value, code: physical, windowsVirtualKeyCode: code });
}

async function select(selector, value) {
  const index = await run(`
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element || element.disabled) throw new Error('Dropdown is absent or disabled: ' + ${JSON.stringify(selector)});
    const index = [...element.options].findIndex(option => option.value === ${JSON.stringify(value)});
    if (index < 0) throw new Error('Missing dropdown option');
    return index;
  `);
  await click(`${selector} + .select-trigger`);
  await waitFor("document.querySelector('.select-popover[role=\"listbox\"]')", 'custom option popup');
  await click(`.select-popover .select-option[data-index="${index}"]`);
  await waitFor(`document.querySelector(${JSON.stringify(selector)})?.value === ${JSON.stringify(value)} && !document.querySelector('.select-popover')`, 'custom selection committed and popup closed');
}

function zeroDurations(value) {
  return value.split(',').every(duration => parseFloat(duration) === 0);
}

async function customKeyboardChecks() {
  await run(`
    const host = document.createElement('div'); host.id = 'dropdown-fixture';
    host.style.cssText = 'position:fixed;right:8px;bottom:8px;width:220px;z-index:61';
    host.innerHTML = '<label for="dropdown-keys">Keyboard options</label><select id="dropdown-keys"><option value="alpha">Alpha</option><option value="beta" disabled>Beta unavailable</option><option value="gamma">Gamma</option><option value="delta">Delta</option>' + Array.from({length: 16}, (_, i) => '<option value="item' + i + '">Item ' + i + '</option>').join('') + '<option value="omega">Omega</option></select><button id="dropdown-after" type="button">Following control</button><select id="dropdown-second" aria-label="Second selector"><option value="one">One</option><option value="two">Two</option></select>';
    document.body.append(host); window.MailBeaconDropdown.mount(host);
    window.__dropdownChangeCount = 0; document.querySelector('#dropdown-keys').addEventListener('change', () => window.__dropdownChangeCount++);
  `);
  await click('#dropdown-keys + .select-trigger');
  const opened = await popupState('#dropdown-keys');
  check('click opens one listbox linked to its combobox with a single selected option', opened.count === 1 && opened.expanded && opened.linked && opened.selected === 1 && opened.value === 'alpha', opened);
  await key('ArrowDown', 40);
  const afterDown = await popupState('#dropdown-keys');
  await key('ArrowUp', 38);
  const afterUp = await popupState('#dropdown-keys');
  await key('End', 35);
  const afterEnd = await popupState('#dropdown-keys');
  await key('Home', 36);
  const afterHome = await popupState('#dropdown-keys');
  await key('g', 71);
  const afterType = await popupState('#dropdown-keys');
  check('arrows skip disabled choices, Home/End and typeahead move without committing', afterDown.active === 'gamma' && afterUp.active === 'alpha' && afterEnd.active === 'omega' && afterHome.active === 'alpha' && afterType.active === 'gamma' && afterType.value === 'alpha', { down: afterDown.active, up: afterUp.active, end: afterEnd.active, home: afterHome.active, typed: afterType.active, value: afterType.value });
  await key('Enter', 13);
  check('Enter commits once, closes the popup and restores trigger focus', await run(`return document.querySelector('#dropdown-keys').value === 'gamma' && window.__dropdownChangeCount === 1 && !document.querySelector('.select-popover') && document.activeElement === document.querySelector('#dropdown-keys').nextElementSibling;`));

  await key(' ', 32);
  await key('End', 35);
  await key('Escape', 27);
  check('Escape cancels a highlighted choice and keeps the surrounding dialog open', await run(`return document.querySelector('#dropdown-keys').value === 'gamma' && !document.querySelector('.select-popover') && !!document.querySelector('.settings-body') && document.activeElement === document.querySelector('#dropdown-keys').nextElementSibling;`));
  await key(' ', 32);
  await key('End', 35);
  await key(' ', 32);
  check('Space opens and commits a keyboard choice', await run(`return document.querySelector('#dropdown-keys').value === 'omega' && window.__dropdownChangeCount === 2 && !document.querySelector('.select-popover');`));
  await click('#dropdown-keys + .select-trigger');
  await key('Tab', 9);
  check('Tab closes the popup and advances to the following control', await run(`return !document.querySelector('.select-popover') && document.activeElement.id === 'dropdown-after';`));

  await click('#dropdown-keys + .select-trigger');
  await click('#dropdown-second + .select-trigger');
  const single = await popupState('#dropdown-second');
  check('opening another selector replaces the previous popup', single.count === 1 && single.expanded && await run(`return document.querySelector('#dropdown-keys').nextElementSibling.getAttribute('aria-expanded') === 'false';`), single.count);
  await click('#dropdown-after');
  check('an outside click dismisses the listbox without changing either value', await run(`return !document.querySelector('.select-popover') && document.querySelector('#dropdown-keys').value === 'omega' && document.querySelector('#dropdown-second').value === 'one';`));

  await click('#dropdown-keys + .select-trigger');
  await run(`document.querySelector('#dropdown-keys').options[0].textContent = 'Renamed Alpha';`);
  const labelMutationClosed = await run(`return !document.querySelector('.select-popover');`);
  await run(`window.MailBeaconDropdown.close();`);
  await click('#dropdown-keys + .select-trigger');
  await run(`document.querySelector('#dropdown-keys').remove(0);`);
  const removalClosed = await run(`return !document.querySelector('.select-popover');`);
  check('option label changes and removal close stale popup contents before another choice', labelMutationClosed && removalClosed, { labelMutationClosed, removalClosed });
  await run(`window.MailBeaconDropdown.close();`);

  await click('#dropdown-keys + .select-trigger');
  const lowerEdge = await popupBounds('#dropdown-keys');
  check('long popup fits above a bottom-edge trigger and keeps its selected item visible', lowerEdge.within && lowerEdge.above && lowerEdge.selectedVisible && lowerEdge.scrollable, lowerEdge);
  await key('Escape', 27);
  await run(`document.querySelector('#dropdown-fixture').style.cssText = 'position:fixed;left:8px;top:8px;width:220px;z-index:61';`);
  await click('#dropdown-keys + .select-trigger');
  const upperEdge = await popupBounds('#dropdown-keys');
  check('popup fits below a top-edge trigger within viewport bounds', upperEdge.within && upperEdge.below, upperEdge);
  await run(`document.querySelector('#dropdown-fixture').remove();`);
  await waitFor("!document.querySelector('.select-popover')", 'detached selector popup cleanup');
  check('removing an open selector during rerender leaves no orphaned popup', await run(`return !document.querySelector('.select-popover') && !document.querySelector('#dropdown-fixture');`));
}

async function popupState(selector) {
  return run(`
    const native = document.querySelector(${JSON.stringify(selector)}), trigger = native.nextElementSibling;
    const popup = document.querySelector('.select-popover'), controls = trigger.getAttribute('aria-controls');
    const active = document.getElementById(trigger.getAttribute('aria-activedescendant') || '') || (document.activeElement?.matches('.select-option') ? document.activeElement : popup?.querySelector('.select-option.is-active'));
    return { count: document.querySelectorAll('.select-popover').length, expanded: trigger.getAttribute('aria-expanded') === 'true', linked: !!popup && !!controls && popup.id === controls && popup.getAttribute('role') === 'listbox', selected: popup?.querySelectorAll('[role="option"][aria-selected="true"]').length, active: active ? native.options[Number(active.dataset.index)]?.value : null, value: native.value };
  `);
}

async function popupBounds(selector) {
  return run(`
    const trigger = document.querySelector(${JSON.stringify(selector)}).nextElementSibling, popup = document.querySelector('.select-popover');
    const r = popup.getBoundingClientRect(), t = trigger.getBoundingClientRect(), selected = popup.querySelector('[aria-selected="true"]')?.getBoundingClientRect();
    return { within: r.left >= 0 && r.right <= innerWidth && r.top >= 0 && r.bottom <= innerHeight, above: r.bottom <= t.top + 1, below: r.top >= t.bottom - 1, selectedVisible: !!selected && selected.top >= r.top && selected.bottom <= r.bottom, scrollable: popup.scrollHeight > popup.clientHeight };
  `);
}

async function protocolRetention(label) {
  const retained = await run(`
    const form = document.querySelector('#acct-form'), protocol = document.querySelector('#receive-protocol');
    return { credentialsRetained: form.elements.namedItem('password').value === window.__dropdownAccountSeed.password,
      smtpRetained: ['smtpHost', 'smtpPort'].every(name => form.elements.namedItem(name).value === window.__dropdownAccountSeed[name]),
      focusRestored: document.activeElement === protocol.nextElementSibling && protocol !== window.__dropdownOldProtocol && !window.__dropdownOldProtocol.isConnected && !document.querySelector('.select-popover') };
  `);
  check(`${label} retains authorization and SMTP values and focuses the replacement control`, retained.credentialsRetained && retained.smtpRetained && retained.focusRestored, retained);
}

async function styles(label, selector, compact = false) {
  const controls = await run(`return [...document.querySelectorAll(${JSON.stringify(selector)})].map(el => {
    const trigger = el.nextElementSibling, css = getComputedStyle(trigger), rect = trigger.getBoundingClientRect();
    return { name: el.id || el.name || el.getAttribute('aria-label'), custom: trigger.matches('.select-trigger[role="combobox"]'), hiddenNative: el.tabIndex === -1 && el.getAttribute('aria-hidden') === 'true', arrow: css.backgroundImage !== 'none' || !!trigger.querySelector('svg'), height: rect.height, border: parseFloat(css.borderTopWidth), radius: parseFloat(css.borderTopLeftRadius) };
  });`);
  check(`${label} uses consistent, usable custom controls`, controls.length > 0 && controls.every(item => item.custom && item.hiddenNative && item.arrow && item.border >= 1 && item.radius >= (compact ? 4 : 7) && (compact ? item.height >= 27 && item.height <= 30 : item.height >= 33.5)), controls);
}

async function focusCheck(label, selector) {
  const before = await run(`const el = document.querySelector(${JSON.stringify(selector)}).nextElementSibling; el.blur(); const css = getComputedStyle(el); return { border: css.borderTopColor, background: css.backgroundColor };`);
  await key('Tab', 9); // Establish keyboard interaction before focusing the target.
  const focus = await run(`
    const el = document.querySelector(${JSON.stringify(selector)}).nextElementSibling; el.focus(); const css = getComputedStyle(el);
    return { active: document.activeElement === el, visible: el.matches(':focus-visible'), outline: css.outlineStyle !== 'none' && parseFloat(css.outlineWidth) > 0, shadow: css.boxShadow !== 'none', border: css.borderTopColor, background: css.backgroundColor };
  `);
  check(`${label} exposes a visible keyboard focus`, focus.active && focus.visible && (focus.outline || focus.shadow || focus.border !== before.border || focus.background !== before.background), focus);
}

async function disabledCheck(label, selector) {
  const state = await run(`
    document.activeElement?.blur(); const native = document.querySelector(${JSON.stringify(selector)}), el = native.nextElementSibling; el.focus(); const css = getComputedStyle(el);
    return { disabled: native.disabled && el.disabled, unfocusable: document.activeElement !== el, cursor: css.cursor, value: native.value };
  `);
  await key('ArrowDown', 40);
  const after = await run(`return document.querySelector(${JSON.stringify(selector)}).value;`);
  check(`${label} is disabled, cannot take focus or change via keyboard`, state.disabled && state.unfocusable && state.cursor === 'default' && after === state.value, state);
}

async function layout(selector) {
  return run(`
    const el = document.querySelector(${JSON.stringify(selector)}).nextElementSibling, r = el.getBoundingClientRect(), p = el.parentElement.parentElement.getBoundingClientRect(), sheet = el.closest('.sheet').getBoundingClientRect();
    return { width: r.width, inside: r.left >= p.left - 1 && r.right <= p.right + 1, parentFitsSheet: p.left >= sheet.left - 1 && p.right <= sheet.right + 1 && sheet.right <= innerWidth + 1 };
  `);
}

async function appearance(selector, theme) {
  await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] });
  return run(`
    const el = document.querySelector(${JSON.stringify(selector)}).nextElementSibling; void el.offsetWidth;
    await Promise.all(el.getAnimations().filter(animation => animation.effect?.getComputedTiming().iterations !== Infinity).map(animation => animation.finished.catch(() => {})));
    const css = getComputedStyle(el); return { color: css.color, background: css.backgroundColor, arrow: css.backgroundImage !== 'none' ? css.backgroundImage : getComputedStyle(el.querySelector('svg')).color };
  `);
}

async function click(selector) {
  const point = await run(`
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) throw new Error('Missing element: ' + ${JSON.stringify(selector)});
    element.scrollIntoView({ block: 'nearest' });
    // Let queued scroll events and mount observers finish before pointer input.
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const rect = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
    return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
  `);
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
