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

  /**
   * Decide how to open a notification selection without using go()'s ++openSeq
   * cancel (which can race a following openMessage).
   *
   * @param {{accountId: string, folder: string, uid?: number}|null} sel
   * @param {{acct: string, folder: string}} current
   * @returns {{sel: object, switchView: boolean, openUid: boolean}|null}
   */
  function planOpenFromNotification(sel, current) {
    if (!sel || typeof sel !== "object") return null;
    if (!sel.accountId || !sel.folder) return null;
    const acct = current && typeof current.acct === "string" ? current.acct : "";
    const folder = current && typeof current.folder === "string" ? current.folder : "";
    return {
      sel,
      switchView: sel.accountId !== acct || sel.folder !== folder,
      openUid: Object.prototype.hasOwnProperty.call(sel, "uid"),
    };
  }

  const api = { selectionFromNotification, planOpenFromNotification };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconNotificationOpen = api;
})(typeof window !== "undefined" ? window : globalThis);
