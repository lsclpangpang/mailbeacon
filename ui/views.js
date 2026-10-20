/* Virtual folders (aggregated across accounts) and list filters. */
(function (root) {
  "use strict";

  const VIEWS = [
    { key: "__unread", label: "所有未读", icon: "unread" },
    { key: "__starred", label: "已加星标", icon: "star" },
    { key: "__drafts", label: "所有草稿", icon: "draft" },
    { key: "__sent", label: "所有已发送", icon: "sent" },
  ];

  const FILTERS = [
    { key: "all", label: "全部" },
    { key: "unread", label: "未读" },
    { key: "flagged", label: "红旗" },
    { key: "attachment", label: "附件" },
    { key: "mentions", label: "@我" },
  ];

  const isVirtual = (folder) => VIEWS.some((v) => v.key === folder);
  /** Views built from role folders: without a folder list they can't be loaded. */
  const needsFolders = (folder) => folder === "__drafts" || folder === "__sent";
  const label = (folder) => VIEWS.find((v) => v.key === folder)?.label || "";

  /**
   * The real folders (and server-side criteria) that make up a virtual view
   * for one account, as arguments for list_messages.
   */
  function sources(view, folders) {
    const byRole = (role) => (folders || []).filter((f) => f.role === role).map((f) => ({ folder: f.path }));
    switch (view) {
      case "__unread": return [{ folder: "INBOX", unreadOnly: true }];
      case "__starred": return [{ folder: "INBOX", flaggedOnly: true }];
      case "__drafts": return byRole("drafts").concat(byRole("local-drafts"));
      case "__sent": return byRole("sent");
      default: return [];
    }
  }

  const MATCH = {
    all: () => true,
    unread: (m) => !m.seen,
    flagged: (m) => m.flagged,
    attachment: (m) => m.attachment,
  };

  /**
   * Apply a list filter. `myEmails(accountId)` gives the account's addresses
   * for "@我"; `keep(m)` keeps e.g. the open message visible.
   */
  function filterMessages(items, filter, myEmails, keep = () => false) {
    const match = filter === "mentions"
      ? (m) => {
        const mine = new Set((myEmails(m.accountId) || []).map((e) => e.toLowerCase()));
        return (m.recipients || []).some((r) => mine.has(String(r).toLowerCase()));
      }
      : MATCH[filter] || MATCH.all;
    return items.filter((m) => match(m) || keep(m));
  }

  const api = { VIEWS, FILTERS, isVirtual, needsFolders, label, sources, filterMessages };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconViews = api;
})(typeof window !== "undefined" ? window : globalThis);
