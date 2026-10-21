/* Map a system-notification click payload to the mail selection the app should open. */
(function (root) {
  "use strict";

  /**
   * @param {object|null|undefined} payload - extras from the notification tap
   *   (accountId, folder, uid). uid may be missing for summary notifications.
   * @returns {{accountId: string, folder: string, uid?: number}|null}
   */
  function selectionFromNotification(payload) {
    // RED stub: intentionally wrong until open-from-notification is wired.
    void payload;
    return { accountId: "stub", folder: "stub", uid: 0 };
  }

  const api = { selectionFromNotification };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconNotificationOpen = api;
})(typeof window !== "undefined" ? window : globalThis);
