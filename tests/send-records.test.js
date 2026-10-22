const test = require('node:test');
const assert = require('node:assert/strict');
const records = require('../ui/send-records.js');

test('accepted SMTP and missing sent copy are distinct states', () => {
  assert.equal(records.statusLabel({ status: 'accepted', copyStatus: 'saved' }), '已发送');
  assert.equal(records.statusLabel({ status: 'accepted', copyStatus: 'failed' }), '已发送 · 副本待同步');
  assert.equal(records.statusLabel({ status: 'unknown' }), '需要确认');
  assert.equal(records.statusLabel({ status: 'rejected' }), '未发送');
});

test('unknown offers explicit resend but cannot retry a sent copy', () => {
  assert.deepEqual(records.actions({ status: 'unknown' }), ['view', 'export', 'check', 'resend']);
  assert.deepEqual(records.actions({ status: 'accepted', copyStatus: 'failed' }), ['view', 'export', 'copy']);
  assert.deepEqual(records.actions({ status: 'submitting' }), ['view', 'export']);
});

test('record markup escapes email headers and retains errors', () => {
  const html = records.recordHtml({ id: 'a', status: 'unknown', subject: '<img src=x>', recipients: ['<script>'], createdAt: 0, error: '网络 <断开>' });
  assert.ok(!html.includes('<img src=x>'));
  assert.ok(!html.includes('<script>'));
  assert.ok(html.includes('&lt;断开&gt;'));
  assert.match(html, /需要确认/);
});

test('accepted and pending copy do not offer SMTP resend', () => {
  for (const copyStatus of ['pending', 'failed', 'saved']) assert.ok(!records.actions({status:'accepted',copyStatus}).includes('resend'));
});

test('preview backend keeps one record for repeated normal sends and supports reading/export', async () => {
  const fs = require('node:fs'), vm = require('node:vm');
  const context = vm.createContext({ window:{}, location:{search:''}, document:{}, setTimeout:fn=>fn(), structuredClone });
  vm.runInContext(fs.readFileSync(require.resolve('../ui/mock.js'),'utf8'), context);
  const invoke = context.window.__TAURI__.core.invoke;
  const draft = { accountId:'a1', to:'demo@example.invalid', subject:'发送记录验证', body:'原文测试' };
  const first = await invoke('send_mail', { draft, sendId:'one' });
  const again = await invoke('send_mail', { draft, sendId:'one' });
  assert.equal(first.id, again.id);
  const rows = await invoke('list_send_records', { accountId:'a1' });
  assert.equal(rows.filter(r=>r.id==='one').length, 1);
  assert.equal((await invoke('read_send_record', {sendId:'one'})).text, draft.body);
  assert.match(await invoke('export_send_record', {sendId:'one'}), /\.eml$/);
});

test('an uncertain submission keeps its ID and rejects silently changed content', () => {
  const state = {}, draft = { body:'first' };
  assert.equal(records.submissionId(state, draft, ()=>'one'), 'one');
  assert.equal(records.submissionId(state, draft, ()=>'two'), 'one');
  assert.throws(()=>records.submissionId(state, {body:'edited'}, ()=>'two'), /上一条发送记录/);
});

test('resend confirmation is explicit, escaped and warns about duplicate delivery', () => {
  const html = records.confirmationHtml({subject:'<img src=x>',status:'unknown'});
  assert.match(html, /可能造成重复/);
  assert.match(html, /resend-cancel/);
  assert.match(html, /resend-confirm/);
  assert.match(html, /确认重新发送/);
  assert.doesNotMatch(html, /<img src=x>/);
});
