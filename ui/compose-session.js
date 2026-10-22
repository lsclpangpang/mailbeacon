/* One durable compose session, independent of its expanded/inline/collapsed view. */
(function (root) {
  'use strict';
  const clone = value => JSON.parse(JSON.stringify(value));
  const fingerprint = value => JSON.stringify(value);

  function create({ id, revision = 0, initial, saved = false, save, remove, onState = () => {} }) {
    let latest = clone(initial), current = revision, persisted = saved ? revision : -1;
    let savedValue = saved ? fingerprint(latest) : null, running = null, closed = false, state = saved ? 'saved' : 'unsaved';
    const notify = next => { state = next; onState(next); };
    const session = {
      id,
      get revision() { return current; },
      get dirty() { return fingerprint(latest) !== savedValue; },
      get state() { return state; },
      get closed() { return closed; },
      get value() { return clone(latest); },
      update(value) {
        if (closed) throw new Error('写信会话已结束');
        if (fingerprint(value) !== fingerprint(latest)) { latest = clone(value); ++current; notify('unsaved'); }
        return current;
      },
      async flush() {
        if (closed) throw new Error('写信会话已结束');
        while (running) await running;
        while (persisted < current) {
          const version = current, snapshot = clone(latest);
          notify('saving');
          running = (async () => {
            const result = await save({ sessionId: id, revision: version, ...snapshot });
            if (!result || result.sessionId !== id || result.revision !== version) throw new Error('草稿保存版本未确认，请重试');
            persisted = version; savedValue = fingerprint(snapshot);
          })();
          try { await running; }
          catch (error) { notify('error'); throw error; }
          finally { running = null; }
        }
        notify('saved');
        return { sessionId: id, revision: persisted };
      },
      async end() {
        if (closed) return;
        closed = true;
        while (running) await running.catch(() => {});
        try { await remove({ sessionId: id, revision: ++current }); notify('closed'); }
        catch (error) { closed = false; notify('error'); throw error; }
      },
    };
    return session;
  }

  // Navigation resolves only the latest intent; no caller may mutate selection
  // before the durability barrier finishes.
  function navigationGate() {
    let sequence = 0;
    return async function navigate(prepare, commit) {
      const request = ++sequence;
      await prepare();
      if (request !== sequence) return false;
      await commit();
      return true;
    };
  }
  function canSend({ mode, busy, focused, blocked, composing = false }) {
    return mode !== 'collapsed' && !busy && focused && !blocked && !composing;
  }
  const api = { create, navigationGate, canSend };
  if (typeof module === 'object' && module.exports) module.exports = api;
  else root.MailBeaconComposeSession = api;
})(typeof window !== 'undefined' ? window : globalThis);
