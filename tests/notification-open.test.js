const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
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

// Contract for the later implementer: backend emits open-mail; app listens and opens.
test('app boot will listen for open-mail and open or focus from the payload', () => {
  const app = fs.readFileSync(require.resolve('../ui/app.js'), 'utf8');
  const lib = fs.readFileSync(require.resolve('../src-tauri/src/lib.rs'), 'utf8');
  assert.match(app, /listen\(["']open-mail["']/, 'frontend must listen for open-mail');
  assert.match(app, /selectionFromNotification|MailBeaconNotificationOpen/, 'use the notification-open helper');
  assert.match(lib, /on_action/, 'notification on_action must be registered in setup');
  assert.match(lib, /open-mail/, 'tap handler must emit open-mail to the webview');
});
