// Observe the installed WebView2 through a temporary loopback CDP endpoint.
// No mocks, app hooks, saved account changes, or real email traffic.
import { mkdirSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const packaged = value => {
  try { const url = new URL(value); return ['http:', 'https:'].includes(url.protocol) && url.hostname === 'tauri.localhost'; }
  catch { return false; }
};
const welcome = state => packaged(state.url) && state.native && state.platform === 'windows'
  && state.documentReady && state.grid && state.heading === '欢迎使用 MailBeacon'
  && state.readerWidth >= 320 && state.welcomeWidth >= 320
  && state.headingVisible && state.formVisible && state.emailVisible && state.passwordVisible && state.submitVisible;

export async function waitForUi({ sample, click, now = Date.now, sleep: pause = sleep, timeoutMs = 60000 }) {
  let last;
  async function until(predicate, label, budget = timeoutMs) {
    const deadline = now() + budget;
    while (now() < deadline) {
      try { last = await sample(); }
      catch (error) {
        // The initial about:blank document can disappear during navigation.
        // Retry only this transient protocol state, not application exceptions.
        if (!/Execution context was destroyed|Cannot find context with specified id/i.test(error.message)) throw error;
        await pause(250); continue;
      }
      if (last.url && last.url !== 'about:blank' && !packaged(last.url)) throw new Error(`Expected the packaged app, got ${last.url}`);
      if (predicate(last)) return last;
      await pause(250);
    }
    throw new Error(`Timed out waiting for ${label}; last state: ${JSON.stringify(last)}`);
  }
  const before = await until(welcome, 'visible native welcome page and account form');
  if (!before.mailboxButton?.visible || !before.mailboxButton.unobscured || before.selectedKind === 'gmail') {
    throw new Error('The mailbox type button is unavailable, covered, or already selected.');
  }
  await click({ x: before.mailboxButton.x, y: before.mailboxButton.y });
  const after = await until(state => welcome(state) && state.selectedKind === 'gmail' && state.emailPlaceholder === 'you@gmail.com', 'mailbox type interaction', Math.min(timeoutMs, 10000));
  return { ready: true, interaction: true, before, after };
}

export function validateDebugSocket(value, port) {
  const url = new URL(value);
  if (url.protocol !== 'ws:' || !['127.0.0.1', 'localhost'].includes(url.hostname)
    || Number(url.port) !== port || !url.pathname.startsWith('/devtools/') || url.username || url.password) {
    throw new Error('WebView debugging must use the temporary loopback endpoint.');
  }
  url.hostname = '127.0.0.1';
  return url.href;
}

// Sent as one read-only expression. Visibility includes viewport intersection;
// selecting a mailbox type uses real CDP input rather than calling a handler.
export function inspectWelcome() {
  const query = selector => document.querySelector(selector);
  const visible = element => {
    if (!element) return false;
    // Opacity does not inherit into a child's computed style. Its whole ancestor
    // chain must be painted before its positive bounds can prove visibility.
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (Number(style.opacity) <= 0 || style.display === 'none' || style.contentVisibility === 'hidden') return false;
    }
    const r = element.getBoundingClientRect(), s = getComputedStyle(element);
    return r.width > 1 && r.height > 1 && r.bottom > 0 && r.right > 0 && r.top < innerHeight && r.left < innerWidth
      && s.visibility !== 'hidden' && s.visibility !== 'collapse';
  };
  const button = query('[data-kind="gmail"]'), rect = button?.getBoundingClientRect();
  const x = rect ? rect.left + rect.width / 2 : 0, y = rect ? rect.top + rect.height / 2 : 0;
  const layout = selector => {
    const element = query(selector);
    if (!element) return null;
    const r = element.getBoundingClientRect(), s = getComputedStyle(element);
    return { x: r.x, y: r.y, width: r.width, height: r.height, display: s.display, gridColumn: s.gridColumn, gridTemplateColumns: s.gridTemplateColumns };
  };
  return {
    url: location.href, native: !!window.__TAURI_INTERNALS__ && typeof window.__TAURI__?.core?.invoke === 'function',
    platform: document.documentElement.dataset.platform, documentReady: document.readyState === 'complete',
    grid: !!query('#app') && getComputedStyle(query('#app')).display === 'grid',
    heading: query('.welcome h2')?.textContent?.trim(), headingVisible: visible(query('.welcome h2')),
    formVisible: visible(query('#acct-form')), emailVisible: visible(query('#acct-form [name="email"]')),
    passwordVisible: visible(query('#acct-form [name="password"]')), submitVisible: visible(query('#acct-form [type="submit"]')),
    selectedKind: query('.kind.on')?.dataset.kind,
    mailboxButton: { visible: visible(button), unobscured: !!button && document.elementFromPoint(x, y)?.closest('[data-kind]') === button, x, y },
    emailPlaceholder: query('#acct-form [name="email"]')?.placeholder,
    viewport: { width: innerWidth, height: innerHeight },
    readerWidth: query('#reader')?.getBoundingClientRect().width || 0,
    welcomeWidth: query('.welcome')?.getBoundingClientRect().width || 0,
    layout: { app: layout('#app'), reader: layout('#reader'), welcome: layout('.welcome'), form: layout('#acct-form') },
  };
}

const eventTypes = new Set(['Runtime.exceptionThrown', 'Runtime.consoleAPICalled', 'Log.entryAdded', 'Network.loadingFailed']);
export function recordEvent(report, message) {
  if (!eventTypes.has(message.method)) return;
  // Bounded diagnostic storage must never bound the failure signal itself.
  if (message.method === 'Runtime.exceptionThrown') {
    report.exceptionCount = (report.exceptionCount || 0) + 1;
    report.firstException ??= message.params?.exceptionDetails;
  }
  if (report.events.length < 200) report.events.push({ method: message.method, params: message.params });
  else report.eventsDropped = (report.eventsDropped || 0) + 1;
}
export function assertNoStartupExceptions(report) {
  if (report.exceptionCount > 0) throw new Error('An uncaught JavaScript exception occurred during startup; see UI diagnostics.');
}

async function connect(port, report) {
  let target;
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(750) });
      if (response.ok) {
        const pages = (await response.json()).filter(page => page.type === 'page');
        report.targets = pages.map(({ id, title, url }) => ({ id, title, url }));
        target = pages.find(page => packaged(page.url)) || (pages.length === 1 && pages[0].url === 'about:blank' ? pages[0] : null);
        if (target) break;
      }
    } catch (error) { report.connectionNote = error.message; }
    await sleep(250);
  }
  if (!target?.webSocketDebuggerUrl) throw new Error('Installed WebView2 did not expose its packaged page on the temporary debug endpoint.');
  const socket = new WebSocket(validateDebugSocket(target.webSocketDebuggerUrl, port));
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { socket.close(); reject(new Error('CDP connection timed out.')); }, 5000);
    socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('CDP connection failed.')); }, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    const item = pending.get(message.id);
    if (item) {
      clearTimeout(item.timer); pending.delete(message.id);
      if (message.error) item.reject(new Error(JSON.stringify(message.error))); else item.resolve(message.result);
    } else recordEvent(report, message);
  });
  return {
    send(method, params = {}) {
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timed out: ${method}`)); }, 5000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ id, method, params }));
      });
    },
    close() {
      for (const item of pending.values()) { clearTimeout(item.timer); item.reject(new Error('CDP closed.')); }
      pending.clear(); socket.close();
    },
  };
}

async function run(port, directory) {
  const report = { passed: false, exceptionCount: 0, eventsDropped: 0, events: [], states: [] };
  mkdirSync(directory, { recursive: true });
  let cdp, failure;
  const evaluate = async expression => {
    const response = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails));
    return response.result?.value;
  };
  try {
    cdp = await connect(port, report);
    for (const domain of ['Runtime', 'Page', 'Log', 'Network']) await cdp.send(`${domain}.enable`);
    report.ui = await waitForUi({
      sample: async () => {
        const state = await evaluate(`(${inspectWelcome.toString()})()`);
        report.url = state.url;
        // Keep the latest states and the first sample; do not retain form values.
        if (report.states.length >= 12) report.states.splice(1, 1);
        report.states.push(state);
        return state;
      },
      click: async ({ x, y }) => {
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
        await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
      },
    });
    await evaluate('new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)))');
    assertNoStartupExceptions(report);
    report.passed = true;
  } catch (error) { failure = error; report.error = error.message; }
  finally {
    if (cdp) {
      try {
        const page = await evaluate('({url: location.href, readyState: document.readyState, html: document.documentElement.outerHTML})');
        report.url = page.url; report.readyState = page.readyState;
        writeFileSync(join(directory, 'webview-dom.html'), page.html);
      } catch (error) { report.domError = error.message; }
      try {
        const screenshot = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
        writeFileSync(join(directory, 'webview.png'), Buffer.from(screenshot.data, 'base64'));
      } catch (error) { report.screenshotError = error.message; }
      cdp.close();
    }
    try { assertNoStartupExceptions(report); }
    catch (error) {
      report.passed = false;
      if (!failure) { failure = error; report.error = error.message; }
    }
    writeFileSync(join(directory, 'ui-smoke-result.json'), JSON.stringify(report, null, 2));
  }
  if (failure) throw failure;
  console.log('PASS Installed Windows welcome page and mailbox type interaction');
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const port = Number(process.argv[2]);
  if (process.platform !== 'win32' || process.env.GITHUB_ACTIONS !== 'true' || !process.env.RUNNER_TEMP
    || !Number.isInteger(port) || port < 1024 || port > 65535 || !process.argv[3]) {
    throw new Error('UI smoke requires a disposable Windows runner, temporary debug port, and output directory.');
  }
  await run(port, resolve(process.argv[3]));
}
