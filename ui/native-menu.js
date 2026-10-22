/* 原生菜单只描述当前可执行的操作，实际动作复用 app.js 的现有路径。 */
(function (root) {
  'use strict';
  const online = (S, id) => !['offline', 'error', 'connecting'].includes(S.live[id]?.state);
  function createFolderAccount(S) {
    const candidates = S.acct === 'all' ? S.accounts : S.accounts.filter(a => a.id === S.acct);
    return candidates.find(a => a.receiveProtocol !== 'pop3' && online(S, a.id)) || null;
  }
  function states(S, { message: m, modal = false, pending = false, now = Date.now() } = {}) {
    const ready = !modal && !pending;
    const selectedAccount = S.accounts.find(a => a.id === S.acct) || (S.acct === 'all' ? S.accounts[0] : null);
    const hasAccounts = !!selectedAccount;
    const a = m && S.accounts.find(a => a.id === m.accountId);
    const identity = !!a && !!S.sel && ['accountId', 'folder', 'uid'].every(k => S.sel[k] === m[k])
      && (a.receiveEpoch ?? 1) === (m.receiveEpoch ?? 1)
      && (S.sel.receiveEpoch == null || S.sel.receiveEpoch === (m.receiveEpoch ?? 1))
      && (S.sel.uidValidity == null || S.sel.uidValidity === (m.uidValidity ?? 0));
    const folders = m ? S.folders[m.accountId] || [] : [];
    const role = folders.find(f => f.path === m?.folder)?.role;
    const localFolder = !!m && (['__outbox', '__local_drafts'].includes(m.folder) || ['outbox', 'local-drafts'].includes(role));
    const selected = ready && identity && !S.batchMode && !S.batch.size && !localFolder
      && !S.moving.has(`${m.accountId}|${m.folder}|${m.uid}`);
    const single = selected && !m.localOnly && !S.msg?.localOnly && online(S, m.accountId);
    const imap = single && a.receiveProtocol !== 'pop3';
    // A complete local body is enough to start a draft; modifying the source still requires online validation.
    const body = selected && !!S.msg && !S.msg.partial && !S.msgError;
    const browsing = ready && hasAccounts;
    const listFilter = browsing && !S.searching && !String(S.query || '').trim();
    const undo = S.lastOperation;
    const result = [
      { id:'settings', enabled:ready },
      { id:'compose', enabled:browsing },
      { id:'check-mail', enabled:browsing && !S.checking, text:S.checking ? '正在收取…' : '收取新邮件' },
      { id:'download-message', enabled:single && !S.downloading.has(`${m?.accountId}|${m?.folder}|${m?.uid}`) },
      { id:'create-folder', enabled:browsing && !!createFolderAccount(S) },
      { id:'search', enabled:browsing },
      { id:'toggle-sidebar', enabled:browsing, checked:!S.ui.collapsed },
      { id:'toggle-threads', enabled:browsing, checked:!!S.threads },
      { id:'filter-all', enabled:listFilter, checked:S.filter === 'all' },
      { id:'filter-unread', enabled:listFilter, checked:S.filter === 'unread' },
      { id:'filter-starred', enabled:listFilter, checked:S.filter === 'flagged' },
      { id:'send-records', enabled:ready },
      { id:'operation-history', enabled:ready },
      { id:'reply', enabled:body },
      { id:'reply-all', enabled:body },
      { id:'forward', enabled:body },
      { id:'archive', enabled:imap && !['archive','all'].includes(role) && folders.some(f => ['archive','all'].includes(f.role)) },
      { id:'move', enabled:imap },
      { id:'trash', enabled:imap && (role === 'trash' || folders.some(f => f.role === 'trash')), text:role === 'trash' ? '彻底删除…' : '删除邮件' },
      { id:'toggle-seen', enabled:single, text:m?.seen ? '标为未读' : '标为已读' },
      { id:'toggle-flagged', enabled:imap, text:m?.flagged ? '取消星标' : '加星标' },
      { id:'shortcuts', enabled:ready },
      { id:'undo-mail-operation', enabled:ready && !S.moving.size && !!undo?.undoToken && undo.expiresAt > now
        && (undo.items || []).every(item => S.accounts.some(a => a.id === item.source.accountId) && online(S, item.source.accountId)) },
    ];
    return result;
  }
  const api = { states, createFolderAccount };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MailBeaconNativeMenu = api;
})(typeof window !== 'undefined' ? window : globalThis);
