/* Conversation view: group mail into threads by Message-ID / In-Reply-To / References. */
(function (root) {
  "use strict";

  const PREFIX = /^\s*(?:(?:re|fw|fwd|aw|sv|回复|答复|转发)\s*[:：]\s*)+/i;
  const normalizeSubject = (s) => String(s || "").replace(PREFIX, "").trim();
  const isReply = (s) => PREFIX.test(String(s || ""));
  const same = (a, b) => a.accountId === b.accountId && a.folder === b.folder && a.uid === b.uid;

  /**
   * Group a list (newest first) into conversations. Each group keeps the list
   * order; groups are ordered by their newest message. `isMine(m)` marks mail
   * sent from one of my own addresses so it shows as "我".
   */
  function groupThreads(items, isMine) {
    const parent = items.map((_, i) => i);
    const find = (i) => (parent[i] === i ? i : (parent[i] = find(parent[i])));
    const union = (a, b) => { const ra = find(a), rb = find(b); if (ra !== rb) parent[Math.max(ra, rb)] = Math.min(ra, rb); };
    const byId = new Map();
    const byRoot = new Map();
    const k = (m, id) => `${m.accountId}|${id}`;
    items.forEach((m, i) => { if (m.messageId && !byId.has(k(m, m.messageId))) byId.set(k(m, m.messageId), i); });
    items.forEach((m, i) => {
      const refs = [...(m.references || []), m.inReplyTo].filter(Boolean);
      for (const id of refs) if (byId.has(k(m, id))) union(i, byId.get(k(m, id)));
      // 根邮件没加载时，引用同一个根的邮件也是同一会话
      const rootId = m.references?.[0] || m.inReplyTo;
      if (rootId) {
        if (byRoot.has(k(m, rootId))) union(i, byRoot.get(k(m, rootId)));
        else byRoot.set(k(m, rootId), i);
      }
      if (m.messageId && byRoot.has(k(m, m.messageId))) union(i, byRoot.get(k(m, m.messageId)));
    });
    // 没有会话信息的回复：挂到同一账号里同主题、更早的最近一封
    items.forEach((m, i) => {
      if (m.inReplyTo || m.references?.length || !isReply(m.subject)) return;
      const subject = normalizeSubject(m.subject);
      if (!subject) return;
      const j = items.findIndex((x, n) => n > i && x.accountId === m.accountId && normalizeSubject(x.subject) === subject);
      if (j >= 0) union(i, j);
    });
    const groups = new Map();
    items.forEach((m, i) => {
      const r = find(i);
      if (!groups.has(r)) groups.set(r, []);
      groups.get(r).push(m);
    });
    return [...groups.values()].map((list) => {
      const names = [];
      for (const m of [...list].sort((a, b) => a.date - b.date)) {
        const name = isMine(m) ? "我" : m.fromName || m.fromAddr || "(未知发件人)";
        if (!names.includes(name)) names.push(name);
      }
      return {
        key: `${list[0].accountId}|${list[0].folder}|${list[0].uid}`,
        items: list,
        latest: list[0],
        count: list.length,
        unread: list.filter((m) => !m.seen).length,
        flagged: list.some((m) => m.flagged),
        attachment: list.some((m) => m.attachment),
        participants: names,
      };
    });
  }

  /**
   * The conversation shown in the reader, oldest first: what the server found
   * plus anything in the list it missed. Mail already in a list keeps its list
   * object so read/star changes stay in sync.
   */
  function mergeConversation(server, listed) {
    const out = (server || []).map((m) => listed.find((x) => same(x, m)) || m);
    for (const m of listed) {
      if (!out.some((x) => same(x, m) || (m.messageId && x.messageId === m.messageId))) out.push(m);
    }
    return out.sort((a, b) => a.date - b.date || a.uid - b.uid);
  }

  const api = { normalizeSubject, isReply, groupThreads, mergeConversation };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconThreads = api;
})(typeof window !== "undefined" ? window : globalThis);
