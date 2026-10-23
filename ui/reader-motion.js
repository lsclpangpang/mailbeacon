/* A cancellable one-page scroll animation; navigation remains the caller's job. */
(function (root) {
  "use strict";

  const DURATION = 180;
  const clamp = (value, maximum) => Math.max(0, Math.min(value, maximum));

  function createScrollMotion({ requestFrame, cancelFrame, now, reducedMotion = false }) {
    let frame = null;
    let token = 0;
    let active = false;
    const reduce = () => typeof reducedMotion === "function" ? reducedMotion() : reducedMotion;

    function cancel() {
      ++token;
      if (frame !== null) cancelFrame(frame);
      frame = null;
      active = false;
    }

    function scroll(el, delta) {
      if (active || !el || el.isConnected === false || !Number.isFinite(delta) || delta === 0) return false;
      const { scrollTop: start, clientHeight, scrollHeight } = el;
      if (![start, clientHeight, scrollHeight].every(Number.isFinite) || clientHeight <= 0) return false;
      const maximum = Math.max(0, scrollHeight - clientHeight);
      const target = clamp(start + delta, maximum);
      if (!maximum || target === start) return false;

      const generation = ++token;
      if (reduce()) {
        el.scrollBy({ top: delta, behavior: "auto" });
        return true;
      }

      const startedAt = now();
      active = true;
      const tick = () => {
        if (generation !== token || !active) return;
        frame = null;
        if (el.isConnected === false) { cancel(); return; }
        const limit = Math.max(0, el.scrollHeight - el.clientHeight);
        if (reduce()) {
          el.scrollBy({ top: clamp(target, limit) - el.scrollTop, behavior: "auto" });
          active = false;
          return;
        }

        const progress = Math.max(0, Math.min(1, (now() - startedAt) / DURATION));
        const eased = 1 - (1 - progress) ** 3;
        el.scrollTop = clamp(start + (target - start) * eased, limit);
        if (progress === 1) active = false;
        else frame = requestFrame(tick);
      };
      frame = requestFrame(tick);
      return true;
    }

    return { scroll, cancel, get active() { return active; } };
  }

  /** Keep row identities fixed during gestures; callers restore the returned anchor. */
  function createListSnapshot(key) {
    let scope, items=[], initialized=false;
    const displayed=new Set(), arrivals=new Map();
    return {
      reset() { initialized=false; items=[]; scope=undefined; displayed.clear(); arrivals.clear(); },
      // Only an explicit receive event identifies new mail. Index completion,
      // pagination and refreshed search results merely fill the existing list.
      markNew(messages) {
        if (!initialized) return;
        for (const message of messages) {
          const id=key(message);
          if (!displayed.has(id) && !arrivals.has(id)) arrivals.set(id,false);
        }
      },
      update(incoming, options={}) {
        const {gesture=false, reading=false, force=false, anchor=null, offset=0}=options;
        const next=new Map(incoming.map(item=>[key(item),item]));
        const sameScope=initialized && scope===options.scope;
        if (!sameScope) { displayed.clear(); arrivals.clear(); }
        if (force) arrivals.clear();
        // A search can receive the event before its asynchronous result. Keep
        // unmatched candidates without counting them; only a previously matching
        // pending result that disappears has been removed or filtered out.
        for (const [id,deferred] of arrivals) if (deferred && !next.has(id)) arrivals.delete(id);
        // Once deferred, new mail waits for an explicit merge. Ending a drag
        // releases historical rows without inserting new mail under the pointer.
        if (gesture || reading || [...arrivals.values()].some(Boolean)) {
          for (const id of arrivals.keys()) if (next.has(id)) arrivals.set(id,true);
        }
        if (sameScope && gesture && !force) {
          // A server removal must end a gesture in the caller; never invent a replacement identity.
          items=items.filter(item=>next.has(key(item))).map(item=>next.get(key(item)));
        } else items=incoming.filter(item=>!arrivals.get(key(item)));
        for (const item of items) { const id=key(item); displayed.add(id); arrivals.delete(id); }
        scope=options.scope; initialized=true;
        return {items:items.slice(),pending:[...arrivals.values()].filter(Boolean).length,anchorIndex:anchor==null?-1:items.findIndex(item=>key(item)===anchor),offset};
      },
    };
  }
  function anchorScroll(previous,next,scroll,height) {
    if(!next.length)return 0;
    const old=Math.min(previous.length-1,Math.max(0,Math.floor(scroll/height)));
    if(old<0)return Math.max(0,scroll);
    let index=next.indexOf(previous[old]);
    if(index<0) {
      for(let i=old+1;i<previous.length && index<0;i++)index=next.indexOf(previous[i]);
      if(index<0)index=Math.min(old,next.length-1);
    }
    return index*height+(scroll%height);
  }
  const api = { createScrollMotion, createListSnapshot, anchorScroll };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconReaderMotion = api;
})(typeof window !== "undefined" ? window : globalThis);
