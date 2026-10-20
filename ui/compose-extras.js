/* Scheduled send times and read-receipt prompts. */
(function (root) {
  "use strict";

  const MIN = 60 * 1000;
  const pad = (n) => String(n).padStart(2, "0");
  const dayStart = (ms) => { const d = new Date(ms); d.setHours(0, 0, 0, 0); return d.getTime(); };
  const atClock = (ms, days, h, m = 0) => { const d = new Date(dayStart(ms)); d.setDate(d.getDate() + days); d.setHours(h, m, 0, 0); return d.getTime(); };

  /** Shortcuts for the schedule popover; ones less than 5 minutes away are dropped. */
  function quickSendTimes(now) {
    return [
      { label: "今天 18:00", at: atClock(now, 0, 18) },
      { label: "明天 08:00", at: atClock(now, 1, 8) },
      { label: "明天 14:00", at: atClock(now, 1, 14) },
    ].filter((q) => q.at - now >= 5 * MIN);
  }

  /** Next full hour, at least ten minutes away. */
  function defaultSendAt(now) {
    const d = new Date(now);
    d.setMinutes(0, 0, 0);
    let t = d.getTime() + 60 * MIN;
    if (t - now < 10 * MIN) t += 60 * MIN;
    return t;
  }

  const toLocalInput = (ms) => {
    const d = new Date(ms);
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
  };
  const fromLocalInput = (value) => (value ? new Date(value).getTime() : NaN);

  function fmtSendAt(ms, now = Date.now()) {
    const d = new Date(ms), clock = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
    const days = Math.round((dayStart(ms) - dayStart(now)) / (24 * 60 * MIN));
    if (days === 0) return `今天 ${clock}`;
    if (days === 1) return `明天 ${clock}`;
    const date = `${d.getMonth() + 1}月${d.getDate()}日`;
    return d.getFullYear() === new Date(now).getFullYear() ? `${date} ${clock}` : `${d.getFullYear()}年${date} ${clock}`;
  }

  /** Never automatic: only offered for other people's mail that asked and wasn't answered yet. */
  const NO_RECEIPT_ROLES = new Set(["sent", "drafts", "local-drafts", "outbox", "trash", "junk"]);
  function offerReceipt(view, role, myEmail, handledHere) {
    if (!view?.receiptTo || view.receiptHandled || handledHere || NO_RECEIPT_ROLES.has(role)) return false;
    return view.receiptTo.toLowerCase() !== String(myEmail || "").toLowerCase();
  }

  /** Receipts going somewhere other than the sender are often tracking; say so. */
  function receiptNote(view) {
    return view.receiptTo.toLowerCase() === String(view.from?.addr || "").toLowerCase() ? "" : `（回执地址 ${view.receiptTo} 不是发件人）`;
  }

  const api = { quickSendTimes, defaultSendAt, toLocalInput, fromLocalInput, fmtSendAt, offerReceipt, receiptNote };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconExtras = api;
})(typeof window !== "undefined" ? window : globalThis);
