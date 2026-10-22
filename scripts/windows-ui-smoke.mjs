// Validate snapshots collected by the Windows UI Automation client. This helper
// never connects to a debugger, browser, mailbox, or production application API.
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const finiteRect = r => r && ['x', 'y', 'width', 'height'].every(k => Number.isFinite(r[k]))
  && r.width > 1 && r.height > 1;
const visible = (element, window) => element.offscreen === false && element.enabled === true
  && finiteRect(element.bounds) && element.bounds.x >= window.bounds.x
  && element.bounds.y >= window.bounds.y
  && element.bounds.x + element.bounds.width <= window.bounds.x + window.bounds.width + 1
  && element.bounds.y + element.bounds.height <= window.bounds.y + window.bounds.height + 1;
const qqHint = '生成授权码';
const gmailHint = 'myaccount.google.com/apppasswords';

export function assessSnapshot(snapshot, expected, stage) {
  if (!['qq', 'gmail'].includes(stage)) throw new Error('Unknown UI smoke stage.');
  const window = snapshot?.window;
  if (window && (window.handle !== String(expected.handle) || window.processId !== expected.processId || window.name !== 'MailBeacon')) {
    throw new Error('UI Automation window identity does not match the installed application.');
  }
  const missing = [];
  const nextButton = stage === 'qq' ? 'Gmail' : 'QQ 邮箱';
  if (!window || !finiteRect(window.bounds) || window.bounds.width < 640 || window.bounds.height < 400
      || window.offscreen !== false || window.enabled !== true) {
    return { ready: false, nextButton, missing: ['visible application window'] };
  }
  const all = Array.isArray(snapshot.elements) ? snapshot.elements : [];
  const shown = all.filter(element => visible(element, window));
  if (!shown.some(e => e.controlType === 'Text' && e.name === '欢迎使用 MailBeacon')) missing.push('welcome heading');
  const edits = shown.filter(e => e.controlType === 'Edit' && e.readOnly === false && e.bounds.width >= 120);
  if (edits.filter(e => e.isPassword === false).length < 2) missing.push('editable email and display-name fields');
  if (!edits.some(e => e.isPassword === true)) missing.push('editable password field');
  if (!shown.some(e => e.controlType === 'Button' && e.name === '登录并添加' && e.invoke)) missing.push('login button');
  const buttons = all.filter(e => e.controlType === 'Button' && e.name === nextButton);
  if (buttons.length !== 1 || !visible(buttons[0], window) || !buttons[0].invoke) missing.push('unique invokable mailbox button');
  const wanted = stage === 'qq' ? qqHint : gmailHint, previous = stage === 'qq' ? gmailHint : qqHint;
  if (!shown.some(e => e.controlType === 'Text' && e.name.includes(wanted))) missing.push(`${stage} form hint`);
  if (all.some(e => e.controlType === 'Text' && e.name.includes(previous))) missing.push('previous form hint still present');
  return { ready: missing.length === 0, nextButton, missing };
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [input, output] = process.argv.slice(2);
  if (!input || !output) throw new Error('Expected snapshot and assessment JSON file paths.');
  const { snapshot, expected, stage } = JSON.parse(readFileSync(input, 'utf8').replace(/^\uFEFF/, ''));
  writeFileSync(output, JSON.stringify(assessSnapshot(snapshot, expected, stage), null, 2) + '\n', 'utf8');
}
