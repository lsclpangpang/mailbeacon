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
      if (!Number.isSafeInteger(uid) || uid <= 0 || uid > 0xffffffff) return null;
      sel.uid = uid;
    }
    // Windows cold-start tickets carry the mailbox generation. Keep it intact;
    // an old notification must not borrow a new account or folder identity.
    for (const field of ['receiveEpoch', 'uidValidity']) {
      if (payload[field] == null) continue;
      const value = Number(payload[field]);
      if (!Number.isSafeInteger(value) || value <= 0 || (field === 'uidValidity' && value > 0xffffffff)) return null;
      sel[field] = value;
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

  /** Queue early clicks until account initialization and the native handshake finish. */
  function createDelivery({ invoke, open, onError }) {
    let ready = false, pending = null, handshake = null;
    const deliver = selection => Promise.resolve().then(() => open(selection)).catch(onError);
    return {
      receive(payload) {
        const selection = selectionFromNotification(payload);
        if (!selection) return;
        if (!ready) { pending = selection; return; }
        return deliver(selection);
      },
      ready() {
        if (handshake) return handshake;
        handshake = (async () => {
          let nativePending = null;
          try { nativePending = selectionFromNotification(await invoke('notification_ready')); }
          catch (error) { onError(error); }
          ready = true;
          const selection = pending || nativePending;
          pending = null;
          if (selection) await deliver(selection);
        })();
        return handshake;
      },
    };
  }

  const api = { selectionFromNotification, planOpenFromNotification, createDelivery };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconNotificationOpen = api;
})(typeof window !== "undefined" ? window : globalThis);
