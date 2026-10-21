/* Map a system-notification click payload to the mail selection the app should open. */
(function (root) {
  "use strict";

  /**
   * @param {object|null|undefined} payload - extras from the notification tap
   *   (accountId, folder, uid). uid may be missing for summary notifications.
   * @returns {{accountId: string, folder: string, uid?: number}|null}
   */
  function selectionFromNotification(payload) {
    if (!payload || typeof payload !== "object") return null;
    const accountId = typeof payload.accountId === "string" ? payload.accountId.trim() : "";
    const folder = typeof payload.folder === "string" ? payload.folder.trim() : "";
    if (!accountId || !folder) return null;
    const sel = { accountId, folder };
    const raw = payload.uid;
    if (raw != null && raw !== "") {
      const uid = typeof raw === "number" ? raw : Number(raw);
      if (Number.isFinite(uid)) sel.uid = uid;
    }
    return sel;
  }

  const api = { selectionFromNotification };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconNotificationOpen = api;
})(typeof window !== "undefined" ? window : globalThis);
