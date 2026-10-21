const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const N = require('../ui/notification-open.js');

test('selectionFromNotification opens the mail when uid is present', () => {
  assert.deepEqual(
    N.selectionFromNotification({ accountId: 'a1', folder: 'INBOX', uid: 42 }),
    { accountId: 'a1', folder: 'INBOX', uid: 42 },
  );
  assert.deepEqual(
    N.selectionFromNotification({ accountId: 'a1', folder: '工作', uid: 7 }),
    { accountId: 'a1', folder: '工作', uid: 7 },
  );
});

test('selectionFromNotification without uid only switches account/folder', () => {
  assert.deepEqual(
    N.selectionFromNotification({ accountId: 'a1', folder: 'INBOX' }),
    { accountId: 'a1', folder: 'INBOX' },
  );
  assert.deepEqual(
    N.selectionFromNotification({ accountId: 'a1', folder: 'INBOX', uid: null }),
    { accountId: 'a1', folder: 'INBOX' },
  );
  const sel = N.selectionFromNotification({ accountId: 'a1', folder: '工作', uid: undefined });
  assert.deepEqual(sel, { accountId: 'a1', folder: '工作' });
  assert.equal('uid' in sel, false, 'summary click must not call openMessage with a uid');
});

test('selectionFromNotification rejects incomplete payloads', () => {
  assert.equal(N.selectionFromNotification(null), null);
  assert.equal(N.selectionFromNotification(undefined), null);
  assert.equal(N.selectionFromNotification({}), null);
  assert.equal(N.selectionFromNotification({ accountId: 'a1' }), null, 'folder required');
  assert.equal(N.selectionFromNotification({ folder: 'INBOX' }), null, 'accountId required');
  assert.equal(N.selectionFromNotification({ accountId: '', folder: 'INBOX', uid: 1 }), null);
});

test('index.html loads notification-open.js before app.js', () => {
  const html = fs.readFileSync(path.join(__dirname, '../ui/index.html'), 'utf8');
  const openAt = html.indexOf('notification-open.js');
  const appAt = html.indexOf('app.js');
  assert.ok(openAt >= 0, 'index.html must include notification-open.js');
  assert.ok(appAt >= 0, 'index.html must include app.js');
  assert.ok(openAt < appAt, 'notification-open.js must load before app.js');
});

// Contract for the later implementer: backend emits open-mail; app listens and opens.
test('app boot will listen for open-mail and open or focus from the payload', () => {
  const app = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const lib = fs.readFileSync(require.resolve('../src-tauri/src/lib.rs'), 'utf8');
  // Narrow to setup / notification wiring, not the whole crate.
  const setupSlice = lib.includes('.setup(|app|')
    ? lib.slice(lib.indexOf('.setup(|app|'), lib.indexOf('.setup(|app|') + 12000)
    : lib;
  assert.match(app, /listen\(["']open-mail["']/, 'frontend must listen for open-mail');
  assert.match(app, /selectionFromNotification|MailBeaconNotificationOpen/, 'use the notification-open helper');
  assert.match(setupSlice, /on_action/, 'notification on_action must be registered in setup');
  assert.match(setupSlice, /open-mail/, 'tap handler must emit open-mail to the webview');
  assert.match(
    setupSlice,
    /show_main_window|set_focus/,
    'tap should show/focus the main window before opening mail',
  );
});

test('sync notify attaches click extras from notification_payloads', () => {
  const sync = fs.readFileSync(require.resolve('../src-tauri/src/sync.rs'), 'utf8');
  const start = sync.indexOf('fn notify(');
  assert.ok(start >= 0, 'sync.rs must define notify');
  const slice = sync.slice(start, start + 2500);
  assert.match(slice, /notification_payloads/, 'notify must build payloads with open targets');
  assert.doesNotMatch(
    slice,
    /notifications_for\(/,
    'notify should not use title/body-only notifications_for once payloads exist',
  );
  assert.match(slice, /\.extra\(\s*["']accountId["']/, 'builder must set accountId extra');
  assert.match(slice, /\.extra\(\s*["']folder["']/, 'builder must set folder extra');
  assert.match(slice, /\.extra\(\s*["']uid["']/, 'builder must set uid extra when present');
});
