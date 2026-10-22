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
    return {
      reset() { initialized=false; items=[]; scope=undefined; },
      update(incoming, options={}) {
        const {gesture=false, reading=false, force=false, anchor=null, offset=0}=options;
        const next=new Map(incoming.map(item=>[key(item),item]));
        const known=new Set(items.map(key));
        const added=incoming.filter(item=>!known.has(key(item))).length;
        const preserve=initialized && scope===options.scope && !force && (gesture || (reading && added>0));
        if (preserve) {
          // A server removal must end a gesture in the caller; never invent a replacement identity.
          items=items.filter(item=>next.has(key(item))).map(item=>next.get(key(item)));
        } else items=incoming.slice();
        scope=options.scope; initialized=true;
        return {items:items.slice(),pending:preserve?added:0,anchorIndex:anchor==null?-1:items.findIndex(item=>key(item)===anchor),offset};
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
