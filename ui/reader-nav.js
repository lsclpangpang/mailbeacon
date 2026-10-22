/* Pure navigation and scroll decisions shared by the reader's Space shortcut. */
(function (root) {
  "use strict";

  const BOTTOM_TOLERANCE = 1;

  /** Entries represent visible list rows, including whole conversations. */
  function nextMailTarget(entries, currentIndex, { preferUnread = true } = {}) {
    if (!Array.isArray(entries) || !Number.isInteger(currentIndex) || currentIndex < 0 || currentIndex >= entries.length - 1) return null;
    const following = entries.slice(currentIndex + 1);
    const next = (preferUnread && following.find((entry) => entry.unread)) || following[0];
    const { accountId, folder, uid } = next.target;
    return { accountId, folder, uid };
  }

  function scrollRemaining(metrics) {
    if (!metrics) return 0;
    const { scrollTop, clientHeight, scrollHeight } = metrics;
    if (![scrollTop, clientHeight, scrollHeight].every(Number.isFinite)) return 0;
    return Math.max(0, scrollHeight - clientHeight - Math.max(0, scrollTop));
  }

  function shouldSpaceScrollFirst(metrics) {
    return scrollRemaining(metrics) > BOTTOM_TOLERANCE;
  }

  /** `top` is a signed scroll delta, not an absolute scroll position. */
  function spaceScrollPlan({ outer, inner, frameTop, frameBottom, viewportTop, viewportBottom }) {
    const outerRemaining = scrollRemaining(outer);
    const innerRemaining = scrollRemaining(inner);
    const height = Number.isFinite(outer?.clientHeight) ? outer.clientHeight : 0;
    const page = Math.max(1, height * 0.9);

    if (innerRemaining > BOTTOM_TOLERANCE) {
      const hasGeometry = [frameTop, frameBottom, viewportTop, viewportBottom].every(Number.isFinite);
      if (hasGeometry && frameBottom <= viewportTop) {
        // A wheel/trackpad may have moved beyond the iframe. Reveal it again
        // before advancing any text hidden above the reading viewport.
        const outerTop = Number.isFinite(outer?.scrollTop) ? Math.max(0, outer.scrollTop) : 0;
        return { target: "outer", top: Math.max(-outerTop, frameBottom - viewportBottom) };
      }
      // First reveal the iframe's visible end. Crossing it would skip body
      // content still hidden inside the iframe before reaching the footer.
      const untilFrameEnd = Number.isFinite(frameBottom) && Number.isFinite(viewportBottom)
        ? frameBottom - viewportBottom
        : outerRemaining;
      if (untilFrameEnd > BOTTOM_TOLERANCE && outerRemaining > BOTTOM_TOLERANCE) {
        return { target: "outer", top: Math.min(page, outerRemaining, untilFrameEnd) };
      }
      // A wheel/trackpad may also leave only part of the iframe on screen.
      // Page by that visible intersection so no unseen lines are skipped.
      const visibleHeight = hasGeometry
        ? Math.max(0, Math.min(frameBottom, viewportBottom) - Math.max(frameTop, viewportTop))
        : height;
      if (hasGeometry && visibleHeight === 0) return { target: "outer", top: 0 };
      return { target: "inner", top: Math.min(Math.max(1, visibleHeight * 0.9), innerRemaining) };
    }

    return outerRemaining > BOTTOM_TOLERANCE
      ? { target: "outer", top: Math.min(page, outerRemaining) }
      : null;
  }

  function readerHintLabels({ canArchive, canReply }) {
    return [
      { key: "Space", label: "滚动 / 下一封", disabled: false },
      { key: "E", label: canArchive ? "归档" : "不可归档", disabled: !canArchive },
      { key: "S", label: "星标", disabled: false },
      { key: "R", label: canReply ? "回复" : "暂不可回复", disabled: !canReply },
    ];
  }

  const api = { nextMailTarget, shouldSpaceScrollFirst, spaceScrollPlan, readerHintLabels };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconReaderNav = api;
})(typeof window !== "undefined" ? window : globalThis);
