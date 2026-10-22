(function (root, factory) {
  const api = factory();
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MailBeaconSendRecords = api;
})(typeof window !== 'undefined' ? window : globalThis, function () {
  'use strict';
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&':'&amp;', '<':'&lt;', '>':'&gt;', '"':'&quot;', "'":'&#39;' })[c]);
  function statusLabel(r) {
    if (r.status === 'accepted') return r.copyStatus === 'saved' ? '已发送' : '已发送 · 副本待同步';
    return { prepared:'尚未提交', submitting:'正在发送', rejected:'未发送', unknown:'需要确认' }[r.status] || '状态未知';
  }
  function submissionId(state, draft, create) {
    const fingerprint = JSON.stringify(draft);
    if (state._sendId && state._sendFingerprint !== fingerprint) throw new Error('内容已修改，请先检查上一条发送记录；确认后另存草稿重新编辑。');
    state._sendFingerprint = fingerprint;
    return state._sendId ||= create();
  }
  function actions(r) {
    const result = ['view', 'export'];
    if (r.status === 'accepted' && r.copyStatus !== 'saved') result.push('copy');
    if (r.status === 'unknown') result.push('check');
    if (['prepared', 'rejected', 'unknown'].includes(r.status)) result.push('resend');
    return result;
  }
  const labels = { view:'查看', export:'导出原文', copy:'重试同步副本', check:'检查已发送', resend:'重新发送…' };
  function recordHtml(r) {
    const date = new Date(r.createdAt).toLocaleString('zh-CN', { month:'2-digit', day:'2-digit', hour:'2-digit', minute:'2-digit' });
    return `<article class="send-record" data-send-record="${esc(r.id)}"><div class="send-record-heading"><strong>${esc(r.subject || '（无主题）')}</strong><span class="send-record-status ${r.status === 'unknown' || r.status === 'rejected' ? 'attention' : ''}">${esc(statusLabel(r))}</span></div>
      <p class="send-record-meta">${esc((r.recipients || []).join('、'))}<span>${esc(date)}</span></p>
      <p class="send-record-from">${esc(r.from || '')}${r.scheduledId ? ' · 定时发送' : ''}${r.previousId ? ' · 手动重新发送' : ''}</p>
      ${r.error || r.copyError ? `<p class="send-record-error">${esc(r.error || r.copyError)}</p>` : ''}
      <div class="send-record-actions">${actions(r).map((action) => `<button class="secondary" type="button" data-record-action="${action}" data-record-id="${esc(r.id)}">${labels[action]}</button>`).join('')}</div></article>`;
  }
  function confirmationHtml(row) {
    return `<h4 id="send-record-confirm-title">重新发送这封邮件？</h4><p class="compose-confirm-message">${esc(row.subject || '（无主题）')}</p>
      <p>原邮件可能已经发送。请先检查已发送记录；继续会另发一封邮件，可能造成重复。</p>
      <div class="compose-confirm-actions"><button class="secondary" type="button" data-record-action="resend-cancel">取消</button><button class="primary" type="button" data-record-action="resend-confirm">确认重新发送</button></div>`;
  }
  function cancelConfirmation(state, restoreFocus = true) {
    const pending = state.confirmation;
    state.confirmation = null;
    const pane = state.overlay.querySelector('.send-records-confirm');
    if (pane) { pane.hidden = true; pane.innerHTML = ''; }
    for (const selector of ['.send-records-list', '.send-records-tools', '.send-records-more']) {
      const node = state.overlay.querySelector(selector);
      if (node) node.inert = false;
    }
    if (restoreFocus && pending?.trigger.isConnected) pending.trigger.focus();
  }
  function showConfirmation(state, row, trigger) {
    state.confirmation = { id:row.id, trigger };
    const pane = state.overlay.querySelector('.send-records-confirm');
    pane.innerHTML = confirmationHtml(row); pane.hidden = false;
    for (const selector of ['.send-records-list', '.send-records-tools', '.send-records-more']) {
      const node = state.overlay.querySelector(selector);
      if (node) node.inert = true;
    }
    pane.querySelector('[data-record-action="resend-cancel"]').focus();
    pane.scrollIntoView({ block:'nearest' });
  }
  let active = null;
  async function open(options) {
    const { overlay } = options;
    const state = { ...options, rows:[], hasMore:false, busy:false, seq:0, confirmation:null };
    active = state;
    overlay.innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="send-records-title"><section class="sheet send-records-sheet"><div class="sheet-head"><h3 id="send-records-title">发送记录</h3><button class="icon-btn" type="button" data-action="overlay-close" aria-label="关闭">×</button></div>
      <p class="send-records-note">“已发送”表示发信服务器已接受，不代表对方已读。记录和完整原文保存在本机。</p>
      <div class="send-records-tools"><button class="secondary" type="button" data-record-action="refresh">刷新记录</button><span class="send-records-notice" role="status"></span></div>
      <div class="compose-confirm send-records-confirm" role="alertdialog" aria-labelledby="send-record-confirm-title" hidden></div>
      <div class="send-records-error" role="alert" hidden></div><div class="send-records-list"></div><div class="send-records-preview" hidden></div>
      <button class="secondary send-records-more" type="button" data-record-action="more" hidden>加载更多记录</button></section></div>`;
    const sheet = overlay.querySelector('.send-records-sheet');
    sheet.addEventListener('click', async (event) => {
      const button = event.target.closest('[data-record-action]');
      if (!button) return;
      event.preventDefault(); event.stopPropagation();
      if (state.busy || !current(state)) return;
      let action = button.dataset.recordAction, id = button.dataset.recordId;
      if (action === 'resend-cancel') return cancelConfirmation(state);
      const confirmed = action === 'resend-confirm';
      if (confirmed) {
        if (!state.confirmation) return;
        id = state.confirmation.id; action = 'resend';
        cancelConfirmation(state, false);
      } else if (state.confirmation) return;
      const row = state.rows.find((r) => r.id === id);
      if (action === 'refresh' || action === 'more') return load(state, action === 'more');
      if (!row || !actions(row).includes(action)) return;
      if (action === 'check') return options.onCheck?.(row.accountId);
      if (action === 'resend' && !confirmed) return showConfirmation(state, row, button);
      state.busy = true; button.disabled = true;
      error(state, '');
      try {
        if (action === 'view') {
          const view = await options.invoke('read_send_record', { sendId:id });
          if (!current(state)) return;
          const pane = sheet.querySelector('.send-records-preview');
          pane.hidden = false;
          pane.innerHTML = `<h4>${esc(view.record.subject || '（无主题）')}</h4><pre>${esc(view.text || '（无纯文本正文，可展开原文或导出查看）')}</pre><details><summary>查看邮件原文</summary><pre>${esc(view.raw)}</pre></details>${view.truncated ? '<p class="hint">预览已截取，导出可获得完整原文和附件。</p>' : ''}`;
          pane.scrollIntoView({ block:'nearest', behavior:'smooth' });
        } else if (action === 'export') {
          const path = await options.invoke('export_send_record', { sendId:id });
          notice(state, `原文已保存：${path}`);
        } else if (action === 'copy') {
          const result = await options.invoke('retry_send_copy', { sendId:id });
          notice(state, statusLabel(result));
          await load(state);
        } else if (action === 'resend') {
          const result = await options.invoke('resend_record', { sendId:id, confirmed:true });
          notice(state, statusLabel(result));
          await load(state);
        }
      } catch (e) { error(state, String(e)); }
      finally { state.busy = false; if (button.isConnected) button.disabled = false; }
    });
    sheet.addEventListener('keydown', (event) => {
      if (!state.confirmation || event.isComposing || event.keyCode === 229) return;
      if (event.key === 'Escape') {
        event.preventDefault(); event.stopPropagation();
        cancelConfirmation(state);
      } else if (event.key === 'Tab') {
        const buttons = [...sheet.querySelectorAll('.send-records-confirm button')];
        const index = buttons.indexOf(sheet.ownerDocument.activeElement);
        event.preventDefault(); event.stopPropagation();
        buttons[(index + (event.shiftKey ? buttons.length - 1 : 1)) % buttons.length]?.focus();
      }
    });
    overlay.querySelector('[data-action="overlay-close"]').focus();
    await load(state);
  }
  function current(state) { return active === state && !!state.overlay.querySelector('.send-records-sheet'); }
  function error(state, message) {
    if (!current(state)) return;
    const el = state.overlay.querySelector('.send-records-error');
    el.textContent = message; el.hidden = !message;
  }
  function notice(state, message) { if (current(state)) state.overlay.querySelector('.send-records-notice').textContent = message; }
  async function load(state, more = false) {
    const seq = ++state.seq;
    notice(state, '正在读取…'); error(state, '');
    try {
      const rows = await state.invoke('list_send_records', { accountId:state.accountId || null, offset:more ? state.rows.length : 0 });
      if (!current(state) || seq !== state.seq) return;
      state.rows = more ? state.rows.concat(rows) : rows;
      const list = state.overlay.querySelector('.send-records-list');
      list.innerHTML = state.rows.length ? state.rows.map(recordHtml).join('') : '<p class="send-records-empty">还没有发送记录。普通发送和定时发送开始提交后会显示在这里。</p>';
      state.overlay.querySelector('.send-records-more').hidden = rows.length < 100;
      notice(state, `已显示 ${state.rows.length} 条记录`);
    } catch (e) { if (current(state) && seq === state.seq) { notice(state, '读取未完成'); error(state, `读取发送记录失败：${e}`); } }
  }
  function changed() { if (active && current(active) && !active.busy && !active.confirmation) return load(active); }
  function close() { active = null; }
  return { statusLabel, actions, recordHtml, confirmationHtml, submissionId, open, changed, close };
});
