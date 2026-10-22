/* Selection is a snapshot of loaded messages, never an implicit server query. */
(function (root) {
  "use strict";
  const source = m => ({ accountId: m.accountId, receiveEpoch: m.receiveEpoch ?? 1, folder: m.folder || "INBOX", uidValidity: m.uidValidity ?? 0, uid: m.uid });
  const key = m => JSON.stringify(Object.values(source(m)));
  const members = group => group.items || [group.latest || group];
  function items(groups, selected) {
    const found = new Map();
    for (const group of groups) for (const m of members(group)) {
      const id = key(m);
      if (selected.has(id)) found.set(id, m);
    }
    return [...found.values()];
  }
  const all = groups => new Set(groups.flatMap(g => members(g).map(key)));
  const retain = (selected, groups) => { const available=all(groups); return new Set([...selected].filter(id => available.has(id))); };
  function state(group, selected) {
    const keys = [...new Set(members(group).map(key))];
    const count = keys.filter(id => selected.has(id)).length;
    return count === 0 ? "none" : count === keys.length ? "all" : "mixed";
  }
  function toggle(selected, groups, index, anchor = null) {
    const next = new Set(selected);
    if (!groups[index]) return next;
    const range = Number.isInteger(anchor) && groups[anchor];
    const chosen = range ? groups.slice(Math.min(index, anchor), Math.max(index, anchor) + 1) : [groups[index]];
    const remove = !range && state(groups[index], selected) === "all";
    for (const id of all(chosen)) remove ? next.delete(id) : next.add(id);
    return next;
  }
  // Each pointer update starts from the same snapshot, so reversing the drag
  // restores rows that have left the range instead of accumulating selection.
  function dragRange(base, groups, anchor, index, remove = false) {
    const next = new Set(base);
    if (!Number.isInteger(anchor) || !Number.isInteger(index) || !groups[anchor] || !groups[index]) return next;
    const chosen = groups.slice(Math.min(anchor, index), Math.max(anchor, index) + 1);
    for (const id of all(chosen)) remove ? next.delete(id) : next.add(id);
    return next;
  }
  function rowIndexAt(y, top, scrollTop, count, rowHeight = 66) {
    if (![y, top, scrollTop, count, rowHeight].every(Number.isFinite) || count < 1 || rowHeight <= 0) return -1;
    return Math.max(0, Math.min(Math.floor(count) - 1, Math.floor((y - top + scrollTop) / rowHeight)));
  }
  function edgeVelocity(y, top, bottom, edge = 36, max = 18) {
    if (![y, top, bottom, edge, max].every(Number.isFinite) || bottom <= top || edge <= 0 || max <= 0) return 0;
    const zone = Math.min(edge, (bottom - top) / 2);
    if (y < top + zone) return -max * Math.min(1, (top + zone - y) / zone);
    if (y > bottom - zone) return max * Math.min(1, (y - bottom + zone) / zone);
    return 0;
  }
  function groupByMailbox(messages) {
    const groups = new Map();
    for (const m of messages) {
      const ref = source(m);
      const id = JSON.stringify([ref.accountId, ref.receiveEpoch, ref.folder, ref.uidValidity]);
      if (!groups.has(id)) groups.set(id, []);
      groups.get(id).push(m);
    }
    return [...groups.values()];
  }
  const api = { source, key, items, all, retain, state, toggle, dragRange, rowIndexAt, edgeVelocity, groupByMailbox };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconSelection = api;
})(typeof window !== "undefined" ? window : globalThis);
