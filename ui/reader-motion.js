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
      acknowledge(message) {
        if (!initialized) return;
        const id=key(message);
        arrivals.delete(id);
        displayed.add(id);
      },
      update(incoming, options={}) {
        const {gesture=false, force=false, anchor=null, offset=0}=options;
        const next=new Map(incoming.map(item=>[key(item),item]));
        const sameScope=initialized && scope===options.scope;
        if (!sameScope) { displayed.clear(); arrivals.clear(); }
        if (force) arrivals.clear();
        // A search can receive the event before its asynchronous result. Keep
        // unmatched candidates without counting them; only a previously matching
        // arrival that disappears has been removed or filtered out.
        for (const [id,matched] of arrivals) {
          if (next.has(id)) arrivals.set(id,true);
          else if (matched) arrivals.delete(id);
        }
        if (sameScope && gesture && !force) {
          // A server removal must end a gesture in the caller; never invent a replacement identity.
          items=items.filter(item=>next.has(key(item))).map(item=>next.get(key(item)));
        } else items=incoming.slice();
        // Visibility and arrival navigation are independent: reading does not
        // hide new rows, and showing a row does not acknowledge the new mail.
        for (const item of items) displayed.add(key(item));
        const currentArrivals=incoming.filter(item=>arrivals.has(key(item)));
        scope=options.scope; initialized=true;
        return {items:items.slice(),pending:currentArrivals.length,arrivals:currentArrivals,anchorIndex:anchor==null?-1:items.findIndex(item=>key(item)===anchor),offset};
      },
    };
  }
  function anchorScroll(previous,next,scroll,height) {
    if(!next.length || scroll===0)return 0;
    const old=Math.min(previous.length-1,Math.max(0,Math.floor(scroll/height)));
    if(old<0)return Math.max(0,scroll);
    const members=value=>Array.isArray(value)?value:[value];
    const findShared=value=> {
      const keys=new Set(members(value));
      return next.findIndex(candidate=>members(candidate).some(id=>keys.has(id)));
    };
    let index=findShared(previous[old]);
    if(index<0) {
      for(let i=old+1;i<previous.length && index<0;i++)index=findShared(previous[i]);
      if(index<0)index=Math.min(old,next.length-1);
    }
    return index*height+(scroll%height);
  }
  const api = { createScrollMotion, createListSnapshot, anchorScroll };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconReaderMotion = api;
})(typeof window !== "undefined" ? window : globalThis);
