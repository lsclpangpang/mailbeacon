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

  const api = { createScrollMotion };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconReaderMotion = api;
})(typeof window !== "undefined" ? window : globalThis);
