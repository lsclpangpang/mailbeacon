/* Recipient field helpers: quote-aware tokens, suggestion insertion, de-duplication. */
(function (root) {
  "use strict";

  const SEPARATORS = new Set([",", ";", "，", "；", "\n"]);

  /** Separator positions outside quoted names (same rules as the Rust side). */
  function separators(value) {
    const out = [];
    let quoted = false, escaped = false;
    for (let i = 0; i < value.length; i++) {
      const c = value[i];
      if (escaped) { escaped = false; continue; }
      if (c === "\\" && quoted) { escaped = true; continue; }
      if (c === '"') { quoted = !quoted; continue; }
      if (!quoted && SEPARATORS.has(c)) out.push(i);
    }
    return out;
  }

  function tokens(value) {
    const text = String(value || "");
    const cuts = separators(text);
    const out = [];
    let start = 0;
    for (const cut of [...cuts, text.length]) {
      const raw = text.slice(start, cut);
      if (raw.trim()) out.push(raw.trim());
      start = cut + 1;
    }
    return out;
  }

  /** The recipient being typed at `caret`: raw [start, end) and its trimmed text. */
  function currentToken(value, caret) {
    const text = String(value || "");
    let start = 0, end = text.length;
    for (const cut of separators(text)) {
      if (cut < caret) start = cut + 1;
      else { end = cut; break; }
    }
    return { start, end, text: text.slice(start, end).trim() };
  }

  function formatAddress(name, email) {
    const n = String(name || "").trim();
    if (!n) return email;
    if (/[,;<>@"():\\]/.test(n)) return `"${n.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}" <${email}>`;
    return `${n} <${email}>`;
  }

  const suggestionText = (s) => s.kind === "group" ? (s.members || []).join(", ") : formatAddress(s.name, s.email);

  /** Replace the token under the caret with `insertion`, followed by a separator. */
  function applySuggestion(value, caret, insertion) {
    const text = String(value || "");
    const token = currentToken(text, caret);
    let before = text.slice(0, token.start);
    const after = text.slice(token.end);
    if (before && !/\s$/.test(before)) before += " ";
    const hasNext = after.trim().length > 0;
    const next = before + insertion + (hasNext ? after : ", ");
    return { value: next, caret: before.length + insertion.length + (hasNext ? 0 : 2) };
  }

  function emailOf(token) {
    const match = /<([^<>]+)>\s*$/.exec(token);
    return (match ? match[1] : token).trim();
  }

  /** Append addresses that are not already in the field (compared by email, case-insensitive). */
  function addRecipients(value, addresses) {
    const list = tokens(value);
    const seen = new Set(list.map((t) => emailOf(t).toLowerCase()));
    for (const address of addresses) {
      const key = emailOf(address).toLowerCase();
      if (!key || seen.has(key)) continue;
      seen.add(key);
      list.push(address.trim());
    }
    return list.join(", ");
  }

  /** Drop repeated addresses; keep a trailing separator so the user can keep typing. */
  function dedupe(value) {
    const text = String(value || "");
    const trailing = SEPARATORS.has(text.trimEnd().slice(-1));
    const result = addRecipients("", tokens(text));
    return result && trailing ? result + ", " : result;
  }

  const api = { tokens, dedupe, currentToken, formatAddress, suggestionText, applySuggestion, emailOf, addRecipients };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconRecipients = api;
})(typeof window !== "undefined" ? window : globalThis);
