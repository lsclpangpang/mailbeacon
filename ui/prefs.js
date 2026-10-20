/* Notification, tray and startup preferences shown in Settings. */
(function (root) {
  "use strict";

  const CONTENT = [
    { key: "full", label: "发件人和主题" },
    { key: "sender", label: "只显示发件人" },
    { key: "hidden", label: "只提示有新邮件" },
  ];

  /** Inbox always alerts; sent/drafts/trash/junk and "All Mail" would only repeat or annoy. */
  const SKIP_ROLES = new Set(["inbox", "sent", "drafts", "trash", "junk", "all", "local-drafts"]);
  const notifiableFolders = (folders) => (folders || []).filter((f) => !SKIP_ROLES.has(f.role) && !String(f.path).startsWith("__"));

  /**
   * Merge what the settings form shows into the saved preferences. Controls that
   * are absent (undefined) keep their saved value; `accounts[id].folders === null`
   * means that account's folder list wasn't loaded, so its folders are kept.
   */
  function fromControls(prev, controls) {
    const next = { ...prev, mutedAccounts: [...(prev.mutedAccounts || [])], notifyFolders: { ...(prev.notifyFolders || {}) } };
    for (const k of ["notify", "notifyContent", "dnd", "dndStart", "dndEnd", "tray", "autostart"]) {
      if (controls[k] !== undefined) next[k] = controls[k];
    }
    for (const [id, c] of Object.entries(controls.accounts || {})) {
      if (c.notify !== undefined) {
        next.mutedAccounts = next.mutedAccounts.filter((a) => a !== id);
        if (!c.notify) next.mutedAccounts.push(id);
      }
      if (Array.isArray(c.folders)) {
        if (c.folders.length) next.notifyFolders[id] = [...c.folders];
        else delete next.notifyFolders[id];
      }
    }
    return next;
  }

  /** Preferences without a removed account's mute and folder choices. */
  function forgetAccount(prefs, id) {
    const notifyFolders = { ...(prefs.notifyFolders || {}) };
    delete notifyFolders[id];
    return { ...prefs, mutedAccounts: (prefs.mutedAccounts || []).filter((a) => a !== id), notifyFolders };
  }

  function accountNotifyHint(prefs, id) {
    if (!prefs.notify) return "新邮件通知已关闭";
    if ((prefs.mutedAccounts || []).includes(id)) return "此账号不弹出通知";
    const extra = (prefs.notifyFolders?.[id] || []).length;
    return extra ? `收件箱和另外 ${extra} 个文件夹有新邮件时提醒` : "收件箱有新邮件时提醒";
  }

  const api = { CONTENT, notifiableFolders, fromControls, forgetAccount, accountNotifyHint };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconPrefs = api;
})(typeof window !== "undefined" ? window : globalThis);
