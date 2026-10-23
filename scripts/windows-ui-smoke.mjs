// Validate snapshots collected by the Windows UI Automation client. This helper
// never connects to a debugger, browser, mailbox, or production application API.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const finiteRect = r => r && ['x', 'y', 'width', 'height'].every(k => Number.isFinite(r[k]))
  && r.width > 1 && r.height > 1;
const nativeItems = popup => {
  const native = popup.win32;
  if (native?.reader !== 'GetMenuBarInfo/GetMenuItemInfoW/GetMenuItemRect'
      || typeof native.menuHandle !== 'string' || !/^[1-9]\d*$/.test(native.menuHandle)
      || native.windowHandle !== popup.handle || !Number.isInteger(native.threadId) || native.threadId <= 0
      || native.threadId !== native.ownerThreadId) return [];
  return (Array.isArray(popup.win32Items) ? popup.win32Items : []).filter(item => item.enabled === true
    && item.separator === false && typeof item.name === 'string' && finiteRect(item.bounds)
    && item.bounds.x >= popup.bounds.x && item.bounds.y >= popup.bounds.y
    && item.bounds.x + item.bounds.width <= popup.bounds.x + popup.bounds.width + 1
    && item.bounds.y + item.bounds.height <= popup.bounds.y + popup.bounds.height + 1);
};
const visible = (element, window) => element.offscreen === false && element.enabled === true
  && finiteRect(element.bounds) && element.bounds.x >= window.bounds.x
  && element.bounds.y >= window.bounds.y
  && element.bounds.x + element.bounds.width <= window.bounds.x + window.bounds.width + 1
  && element.bounds.y + element.bounds.height <= window.bounds.y + window.bounds.height + 1;
const qqHint = '生成授权码';
const gmailHint = 'myaccount.google.com/apppasswords';

export function accountPlan(fixture) {
  if (fixture?.version !== 1 || fixture.host !== '127.0.0.1' || fixture.smtpHost !== '127.0.0.1'
      || fixture.email !== 'windows-smoke@example.invalid'
      || ![fixture.imapPort, fixture.smtpPort].every(port => Number.isInteger(port) && port > 0 && port <= 65535)) {
    throw new Error('Account smoke requires the isolated loopback fixture.');
  }
  return { provider: '其他', submit: '登录并添加', alternate: 'Gmail', fields: {
    email: 'you@example.com', password: '不是网页登录密码', name: '可选，发信时对方看到的名字',
    imapHost: 'imap.example.com', smtpHost: 'smtp.example.com',
  } };
}

export function assessSnapshot(snapshot, expected, stage, { fixture, server } = {}) {
  if (stage === 'account-plan') return accountPlan(fixture);
  if (!['qq', 'gmail', 'custom', 'auth-failed', 'inbox', 'menu-closed', 'menu-open', 'compose-open'].includes(stage)) throw new Error('Unknown UI smoke stage.');
  const window = snapshot?.window;
  if (window && (window.handle !== String(expected.handle) || window.processId !== expected.processId || window.name !== 'MailBeacon')) {
    throw new Error('UI Automation window identity does not match the installed application.');
  }
  const missing = [];
  const accountStage = ['custom', 'auth-failed', 'inbox'].includes(stage);
  const plan = accountStage ? accountPlan(fixture) : null;
  const nextButton = stage === 'qq' || accountStage ? 'Gmail' : 'QQ 邮箱';
  if (!window || !finiteRect(window.bounds) || window.bounds.width < 640 || window.bounds.height < 400
      || window.nativeVisible !== true || window.minimized !== false
      || window.offscreen === true || window.enabled !== true) {
    return { ready: false, nextButton, missing: ['visible application window'] };
  }
  const all = Array.isArray(snapshot.elements) ? snapshot.elements : [];
  const shown = all.filter(element => visible(element, window));
  if (['menu-closed', 'menu-open', 'compose-open'].includes(stage)) {
    if (window.nativeMenuAttached !== false) missing.push('native application menu bar must be detached');
    if (window.nativeCaption !== true) missing.push('native title bar must remain present');
    if (all.some(e => e.controlType === 'MenuBar' && e.inTitleBar !== true && e.offscreen === false && finiteRect(e.bounds))) missing.push('visible application MenuBar');
    const menu = snapshot.nativeMenu;
    const popups = Array.isArray(menu?.windows) ? menu.windows : [];
    if (stage === 'menu-open') {
      if (menu?.inMenuMode !== true || menu.ownerHandle !== String(expected.handle)) missing.push('native popup owned by main window');
      const items = popups.filter(p => p.processId === expected.processId && p.windowClass === '#32768'
        && p.nativeVisible === true && finiteRect(p.bounds))
        .flatMap(p => [...(Array.isArray(p.elements) ? p.elements : []).filter(e => e.controlType === 'MenuItem' && visible(e, p)), ...nativeItems(p)]);
      for (const name of ['MailBeacon', '文件', '编辑', '视图', '邮件', '窗口', '帮助']) {
        if (!items.some(e => e.name.replaceAll('&', '').split('\t')[0] === name)) missing.push(`native menu item: ${name}`);
      }
    } else {
      if (menu?.inMenuMode !== false || popups.length !== 0) missing.push('native popup must close');
      if (stage === 'compose-open') {
        if (!shown.some(e => e.controlType === 'Text' && e.name === '新邮件')) missing.push('compose heading');
        if (!shown.some(e => e.controlType === 'Edit' && e.name === '邮件正文' && e.readOnly === false)) missing.push('editable compose body');
        for (const name of ['取消', '发送']) {
          if (!shown.some(e => e.controlType === 'Button' && e.name === name && e.invoke)) missing.push(`compose button: ${name}`);
        }
      } else {
        const buttons = all.filter(e => e.controlType === 'Button' && e.name === '应用菜单');
        const button = buttons[0];
        const activation = button?.expandCollapse === true
          ? button.expandCollapseState === 'Collapsed' : button?.invoke === true;
        if (buttons.length !== 1 || !visible(button, window) || !activation) missing.push('accessible application menu button');
        if (all.some(e => e.name === '新邮件' && e.controlType === 'Text' || e.name === '邮件正文' && e.controlType === 'Edit')) missing.push('compose sheet must close');
      }
    }
    return { ready: missing.length === 0, missing, menuButton: '应用菜单' };
  }
  if (stage === 'inbox') {
    if (!shown.some(e => ['Text', 'Button'].includes(e.controlType) && e.name.includes(fixture.email))) missing.push('saved fixture account');
    if (!shown.some(e => e.controlType === 'Text' && ['收件箱', '所有收件箱'].includes(e.name))) missing.push('inbox heading');
    if (!shown.some(e => e.controlType === 'Text' && e.name === '这里没有邮件')) missing.push('loaded empty inbox');
    if (all.some(e => e.name === '欢迎使用 MailBeacon' || e.name.includes('fixture-auth-rejected'))) missing.push('onboarding or authentication error still present');
    if (!(server?.authAccepted >= 1 && server?.selectAccepted >= 1)) missing.push('real fixture authentication and successful SELECT');
    return { ready: missing.length === 0, missing };
  }
  if (!shown.some(e => e.controlType === 'Text' && e.name === '欢迎使用 MailBeacon')) missing.push('welcome heading');
  const edits = shown.filter(e => e.controlType === 'Edit' && e.readOnly === false && e.bounds.width >= 120);
  if (edits.filter(e => e.isPassword === false).length < 2) missing.push('editable email and display-name fields');
  if (!edits.some(e => e.isPassword === true)) missing.push('editable password field');
  if (!shown.some(e => e.controlType === 'Button' && e.name === '登录并添加' && e.invoke)) missing.push('login button');
  const buttons = all.filter(e => e.controlType === 'Button' && e.name === nextButton);
  if (buttons.length !== 1 || !visible(buttons[0], window) || !buttons[0].invoke) missing.push('unique invokable mailbox button');
  if (accountStage) {
    if (!shown.some(e => e.controlType === 'Text' && e.name.includes('填写服务器地址'))) missing.push('custom server form');
    for (const key of ['imapHost', 'smtpHost']) {
      const host = edits.find(e => e.name === plan.fields[key]);
      const ports = host && shown.filter(e => e.controlType === 'Edit' && e.readOnly === false && e.isPassword === false
        && e.bounds.width >= 30 && e.bounds.x >= host.bounds.x + host.bounds.width
        && Math.abs(e.bounds.y - host.bounds.y) <= 2);
      if (!host || ports.length !== 1) missing.push(`editable ${key} and port`);
    }
    if (stage === 'auth-failed') {
      if (!shown.some(e => e.controlType === 'Text' && e.name.includes('fixture-auth-rejected'))) missing.push('visible authentication failure');
      if (!(server?.authRejected >= 1)) missing.push('real fixture authentication rejection');
    }
  } else {
    const wanted = stage === 'qq' ? qqHint : gmailHint, previous = stage === 'qq' ? gmailHint : qqHint;
    if (!shown.some(e => e.controlType === 'Text' && e.name.includes(wanted))) missing.push(`${stage} form hint`);
    if (all.some(e => e.controlType === 'Text' && e.name.includes(previous))) missing.push('previous form hint still present');
  }
  return { ready: missing.length === 0, nextButton, missing };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [input, output] = process.argv.slice(2);
  if (!input || !output) throw new Error('Expected snapshot and assessment JSON file paths.');
  const { snapshot, expected, stage, fixture, server } = JSON.parse(readFileSync(input, 'utf8').replace(/^\uFEFF/, ''));
  writeFileSync(output, JSON.stringify(assessSnapshot(snapshot, expected, stage, { fixture, server }), null, 2) + '\n', 'utf8');
}
