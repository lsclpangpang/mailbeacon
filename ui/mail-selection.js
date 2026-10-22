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
  const api = { source, key, items, all, retain, state, toggle, groupByMailbox };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconSelection = api;
})(typeof window !== "undefined" ? window : globalThis);
