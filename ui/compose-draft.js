/* Draft autosave and outgoing attachment bookkeeping for the compose sheet. */
(function (root) {
  "use strict";

  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

  /**
   * Save the latest snapshot after the user pauses for `delay` ms. Saves never
   * overlap: each one receives the location of the previous version so the
   * backend can replace it.
   */
  function createAutosaver({ save, delay = 30000, timers = root, onError = () => {}, onSaved = () => {} }) {
    let latest = null, saved = null, ref = null, timer = null, running = null, paused = false;

    async function run() {
      // Wait for an in-flight save; it may already cover the latest snapshot.
      while (running) await running.catch(() => {});
      if (latest === null || same(latest, saved)) return ref;
      const snapshot = latest;
      running = (async () => {
        try {
          ref = await save(snapshot, ref);
          saved = snapshot;
          onSaved(ref);
        } catch (e) {
          onError(e);
          throw e;
        } finally {
          running = null;
        }
      })();
      await running;
      // Edits made while saving are saved next, but only after the user pauses.
      return ref;
    }

    function cancelTimer() {
      if (timer !== null) timers.clearTimeout(timer);
      timer = null;
    }

    return {
      change(snapshot) {
        latest = snapshot;
        cancelTimer();
        if (paused || same(latest, saved)) return;
        timer = timers.setTimeout(() => { timer = null; run().catch(() => {}); }, delay);
      },
      /** Save now (if anything changed) and resolve to the draft location. */
      flush() {
        cancelTimer();
        return run();
      },
      /** Mark a snapshot as already saved, e.g. a draft that was just reopened. */
      reset(snapshot, location = ref) {
        cancelTimer();
        latest = saved = snapshot;
        ref = location;
      },
      /**
       * Stop autosaving (before sending or discarding) and wait for a save that
       * is already in flight, so no new draft appears afterwards.
       */
      async settle() {
        paused = true;
        cancelTimer();
        while (running) await running.catch(() => {});
        return ref;
      },
      /** Resume autosaving after a failed send. */
      resume() {
        paused = false;
        if (latest !== null && !same(latest, saved)) this.change(latest);
      },
      dispose: cancelTimer,
      get ref() { return ref; },
      get dirty() { return latest !== null && !same(latest, saved); },
    };
  }

  const attachmentKey = (a) => a.kind === "message"
    ? `m:${a.accountId}|${a.folder}|${a.uid}|${a.index}`
    : `f:${a.token}`;

  function addAttachments(list, picked) {
    const out = list.slice();
    for (const p of picked) {
      const item = p.kind ? { ...p } : { kind: "file", token: p.token, name: p.name, size: p.size };
      if (!out.some((a) => attachmentKey(a) === attachmentKey(item))) out.push(item);
    }
    return out;
  }

  const removeAttachment = (list, key) => list.filter((a) => attachmentKey(a) !== key);
  const totalSize = (list) => list.reduce((n, a) => n + (a.size || 0), 0);

  function limitMessage(list, max) {
    const total = totalSize(list);
    if (total <= max) return "";
    const mb = (n) => (n / 1024 / 1024).toFixed(1);
    return `附件共 ${mb(total)} MB，超过 ${Math.round(max / 1024 / 1024)} MB 上限`;
  }

  /** What the backend needs: tokens and message locations, no names or sizes. */
  const toRefs = (list) => list.map((a) => a.kind === "message"
    ? { kind: "message", accountId: a.accountId, receiveEpoch: a.receiveEpoch ?? 1, folder: a.folder, uid: a.uid, uidValidity: a.uidValidity ?? null, index: a.index }
    : { kind: "file", token: a.token });

  /** Attachments of an existing message (forwarding, reopening a draft). */
  const messageAttachments = (view, location) => (view.attachments || []).map((x) => ({
    kind: "message",
    accountId: location.accountId,
    receiveEpoch: location.receiveEpoch ?? 1,
    folder: location.folder,
    uid: location.uid,
    uidValidity: location.uidValidity ?? view.uidValidity ?? null,
    index: x.index,
    name: x.name,
    size: x.size,
  }));

  const person = (p) => (p.name ? `${p.name} <${p.addr}>` : p.addr);
  const angle = (id) => (id && !id.startsWith("<") ? `<${id}>` : id || "");

  /**
   * Compose fields for continuing an existing draft. `picked` are the draft's
   * attachments extracted as tokens; autosave replaces the old version, so the
   * new draft must not depend on it.
   */
  function draftFromView(view, location, picked) {
    const where = { ...location, uidValidity: location.uidValidity ?? view.uidValidity ?? null };
    return {
      accountId: location.accountId,
      to: (view.to || []).map(person).join(", "),
      cc: (view.cc || []).map(person).join(", "),
      bcc: (view.bcc || []).map(person).join(", "),
      subject: view.subject || "",
      body: view.text || "",
      attachments: picked ? addAttachments([], picked) : messageAttachments(view, where),
      inReplyTo: view.inReplyTo ? angle(view.inReplyTo) : null,
      references: (view.references || []).map(angle),
      draftRef: {
        accountId: location.accountId,
        receiveEpoch: location.receiveEpoch ?? view.receiveEpoch ?? 1,
        folder: location.folder,
        uid: location.uid,
        uidValidity: where.uidValidity,
        messageId: angle(view.messageId),
      },
    };
  }

  const api = {
    createAutosaver, attachmentKey, addAttachments, removeAttachment, totalSize,
    limitMessage, toRefs, messageAttachments, draftFromView,
  };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailPulseDraft = api;
})(typeof window !== "undefined" ? window : globalThis);
