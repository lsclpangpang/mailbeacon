/* Sidebar totals come from complete sources, never the currently loaded list. */
(function (root) {
  'use strict';

  const views = typeof module === 'object' && module.exports ? require('./views.js') : root.MailBeaconViews;
  const unknown = () => ({ value: null, exact: false, snapshotAt: null });
  const validNumber = value => Number.isSafeInteger(value) && value >= 0;
  const exact = value => ({ value, exact: true, snapshotAt: null });
  const lookup = (collection, id) => typeof collection?.get === 'function' ? collection.get(id) : collection?.[id];

  function normalize(count) {
    const valid = validNumber(count?.value);
    return {
      value: valid && (count.exact === true || count.value > 0) ? count.value : null,
      exact: valid && count.exact === true,
      snapshotAt: validNumber(count?.snapshotAt) ? count.snapshotAt : null,
    };
  }

  function combine(counts) {
    const rows = counts.map(normalize);
    const value = rows.reduce((sum, row) => sum + (row.value ?? 0), 0);
    const complete = rows.every(row => row.exact);
    const times = rows.map(row => row.snapshotAt).filter(at => at !== null);
    return normalize({ value, exact: complete, snapshotAt: times.length ? Math.min(...times) : null });
  }

  function accountCounts(state, account) {
    const row = lookup(state.sidebarCounts, account.id);
    return row?.accountId === account.id && row.receiveEpoch === (account.receiveEpoch ?? 1) ? row : null;
  }

  function folderCount(row, folder) {
    if (!row) return unknown();
    if (folder === '__local_drafts') return normalize(row.localDrafts);
    return normalize(row.folders?.find(entry => entry.folder === folder)?.count);
  }

  function roleCount(state, account, folder) {
    const row = accountCounts(state, account);
    const catalog = lookup(state.folders, account.id);
    if (!row || !Array.isArray(catalog) || !views) return unknown();
    // A local draft source is already included by views.sources('__drafts').
    const sources = new Set(views.sources(folder, catalog).map(source => source.folder));
    return combine([...sources].map(source => folderCount(row, source)));
  }

  function reminderCount(state, folder) {
    if (!Array.isArray(state.mailReminders)) return unknown();
    const configured = new Set((state.accounts || []).map(account => account.id));
    // Keep this predicate identical to productivity-panels.js reminderList.
    // The parity test covers every reminder kind/state and account scope.
    const rows = state.mailReminders.filter(row =>
      configured.has(row.accountId)
      && (state.acct === 'all' || !state.acct || row.accountId === state.acct)
      && !['completed', 'cancelled'].includes(row.state)
      && (folder === '__snoozed'
        ? row.kind === 'snooze' && row.state === 'scheduled'
        : row.kind === 'followup' || row.state === 'due' || row.state === 'orphaned'));
    return normalize({ value: rows.length, exact: state.remindersLoaded === true, snapshotAt: null });
  }

  /**
   * Uses the app state: sidebarCounts is Map(accountId, snapshot account row).
   * remindersLoaded becomes true only after list_mail_reminders succeeds.
   * Unknown live unread values must be null/undefined, not an initial fake 0.
   */
  function resolve(state, folderOrAction) {
    if (typeof folderOrAction !== 'string' || !folderOrAction) return null;
    const folder = folderOrAction;
    if (folder === '__snoozed' || folder === '__reminders') return reminderCount(state, folder);
    if (folder === '__outbox') {
      if (!Array.isArray(state.outbox)) return unknown();
      return exact(state.outbox.filter(row => state.acct === 'all' || !state.acct || row.accountId === state.acct).length);
    }
    const accounts = (state.accounts || []).filter(account => state.acct === 'all' || !state.acct || account.id === state.acct);
    const perAccount = account => {
      if (folder === 'INBOX' || folder === '__unread') {
        const live = lookup(state.live, account.id);
        if (live?.receiveEpoch != null && live.receiveEpoch !== (account.receiveEpoch ?? 1)) return unknown();
        return normalize({ value: live?.unread, exact: true, snapshotAt: null });
      }
      if (folder === '__drafts' || folder === '__sent') return roleCount(state, account, folder);
      const row = accountCounts(state, account);
      // The backend starred field is derived only from INBOX, like views.sources.
      if (folder === '__starred') return normalize(row?.starred);
      if (folder === '__attachments') return normalize(row?.attachments);
      if (folder === 'send-records') return normalize(row?.sendRecords);
      return folderCount(row, folder);
    };
    const known = ['INBOX', '__unread', '__starred', '__drafts', '__sent', '__attachments', '__local_drafts', 'send-records'].includes(folder)
      || accounts.some(account => (lookup(state.folders, account.id) || []).some(entry => entry.path === folder)
        || accountCounts(state, account)?.folders?.some(entry => entry.folder === folder));
    if (!known) return null;
    if (state.acct && state.acct !== 'all' && accounts.length === 0) return unknown();
    const included = folder === '__attachments' ? accounts.filter(account => account.receiveProtocol !== 'pop3') : accounts;
    return combine(included.map(perAccount));
  }

  /** Text only: the renderer escapes these fields when adding them to markup. */
  function badge(count, folderOrAction) {
    if (count == null) return null;
    const normalized = normalize(count);
    const text = normalized.value === null ? '—' : `${normalized.value}`;
    if (folderOrAction === '__attachments') {
      const quantity = normalized.value === null ? '附件数量尚未确认'
        : normalized.exact ? `共 ${normalized.value} 个附件`
          : `已发现 ${normalized.value} 个附件，完整数量尚未确认`;
      const title = `${quantity}；范围为当前 IMAP 账户，不含草稿、垃圾邮件和已删除`;
      return { text, title, ariaLabel: title, partial: !normalized.exact };
    }
    const title = normalized.value === null ? '数量尚未确认'
      : normalized.exact ? `共 ${normalized.value} 项`
        : `已统计 ${normalized.value} 项，完整数量尚未确认`;
    return { text, title, ariaLabel: title, partial: !normalized.exact };
  }

  const api = { resolve, badge };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MailBeaconSidebarCounts = api;
})(typeof window !== 'undefined' ? window : globalThis);
