/* Rich-text compose helpers: toolbar commands, signature block, HTML to text. */
(function (root) {
  "use strict";

  const SIZES = { small: "2", normal: "3", large: "5", huge: "6" };
  const SIMPLE = {
    bold: "bold", italic: "italic", underline: "underline", strike: "strikeThrough",
    ul: "insertUnorderedList", ol: "insertOrderedList", indent: "indent", outdent: "outdent",
    "align-left": "justifyLeft", "align-center": "justifyCenter", "align-right": "justifyRight",
    clear: "removeFormat", undo: "undo", redo: "redo",
  };

  /** [execCommand name, value] for a toolbar action, or null if not allowed. */
  function commandFor(action, value) {
    if (SIMPLE[action]) return [SIMPLE[action], null];
    if (action === "size") return SIZES[value] ? ["fontSize", SIZES[value]] : null;
    if (action === "color") return /^#[0-9a-f]{6}$/i.test(value || "") ? ["foreColor", value] : null;
    if (action === "font") {
      return /^[\w\s,"'㐀-鿿-]{1,80}$/.test(value || "") ? ["fontName", value] : null;
    }
    return null;
  }

  /** Accept http(s), mailto and tel links; bare domains get https://, bare addresses mailto:. */
  function normalizeLink(input) {
    const value = String(input || "").trim();
    if (!value || /[\s\u0000-\u001f\u007f<>"]/.test(value)) return null;
    if (/^https?:\/\//i.test(value)) return /^https?:\/\/[^\/?#]+(?:[/?#].*)?$/i.test(value) ? value : null;
    if (/^mailto:/i.test(value)) return value.length > "mailto:".length ? value : null;
    if (/^tel:/i.test(value)) return /\d/.test(value.slice("tel:".length)) ? value : null;
    if (/^[^@/:]+@[^@/:]+\.[^@/:]+$/.test(value)) return "mailto:" + value;
    if (/^[a-z][a-z0-9+.-]*:/i.test(value)) return null; // some other scheme
    if (/^[^/:]+\.[^/:]+/.test(value)) return "https://" + value;
    return null;
  }

  function tableHtml(rows, cols) {
    const r = Math.min(20, Math.max(1, rows | 0));
    const c = Math.min(10, Math.max(1, cols | 0));
    const cell = '<td style="border:1px solid #c8c8c8;padding:6px;min-width:60px">&nbsp;</td>';
    const row = `<tr>${cell.repeat(c)}</tr>`;
    return `<table border="1" cellpadding="6" style="border-collapse:collapse"><tbody>${row.repeat(r)}</tbody></table><div><br></div>`;
  }

  // Escape like a browser serializes text nodes, so a block we insert can be
  // found again verbatim in editor.innerHTML.
  const escapeText = (text) => String(text || "").replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/ /g, "&nbsp;");
  const textToHtml = (text) => String(text || "").replace(/\r\n?/g, "\n").split("\n").map(escapeText).join("<br>");

  function signatureBlock(signature) {
    const text = String(signature || "").replace(/\r\n?/g, "\n");
    return text ? `<div class="mp-signature" data-mp-signature="">${textToHtml(text)}</div>` : "";
  }

  /** Editor content for a new message: room to type, then the signature. */
  function initialHtml(body, signature) {
    const typed = body ? `<div>${textToHtml(body)}</div>` : "";
    return "<div><br></div><div><br></div>" + signatureBlock(signature) + typed;
  }

  /**
   * Replace the automatic signature when the sender changes. `block` is the
   * signature HTML we inserted ("" for none, null once the user edited it).
   */
  function switchSignature(html, block, signature) {
    const next = signatureBlock(signature);
    if (block === null || block === undefined) return { html, block: null };
    if (block === "") return next ? { html: html + next, block: next } : { html, block: "" };
    if (html.includes(block)) return { html: html.replace(block, next), block: next };
    return { html, block: null };
  }

  const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " " };
  function decode(text) {
    return text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (match, name) => {
      if (name[0] === "#") {
        const code = name[1] === "x" || name[1] === "X" ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
        return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : match;
      }
      return ENTITIES[name.toLowerCase()] ?? match;
    });
  }

  const BLOCKS = new Set(["div", "li", "tr", "table", "ul", "ol", "blockquote", "h1", "h2", "h3", "h4", "h5", "h6", "hr"]);
  // 属性值里可能有 ">"，按引号整体匹配
  const TOKEN = /<(\/?)([a-z][\w:-]*)((?:[^>"']|"[^"]*"|'[^']*')*)>|<!--[\s\S]*?-->|([^<]+)/gi;

  /** Readable plain text for the text/plain alternative of an HTML message. */
  function htmlToText(html) {
    const source = String(html || "").replace(/<(script|style)\b[\s\S]*?<\/\1\s*>/gi, "");
    let out = "";
    let pre = 0;
    const lists = [];
    const links = [];
    const newline = () => { if (out && !out.endsWith("\n")) out += "\n"; };
    const blank = () => { if (out) { newline(); if (!out.endsWith("\n\n")) out += "\n"; } };
    for (const match of source.matchAll(TOKEN)) {
      if (match[4] !== undefined) {
        const text = pre ? match[4] : match[4].replace(/[\t\n\r ]+/g, " ");
        out += decode(text).replace(/ /g, " ");
        continue;
      }
      if (match[2] === undefined) continue; // comment
      const closing = match[1] === "/", tag = match[2].toLowerCase();
      if (tag === "br") { out += "\n"; continue; }
      if (tag === "p") { blank(); continue; }
      if (tag === "pre") { closing ? (pre = Math.max(0, pre - 1)) : pre++; newline(); continue; }
      if (tag === "ol" || tag === "ul") {
        if (closing) lists.pop(); else lists.push({ ordered: tag === "ol", n: 0 });
        newline();
        continue;
      }
      if (BLOCKS.has(tag)) {
        newline();
        if (!closing && tag === "li") {
          const list = lists[lists.length - 1];
          out += list?.ordered ? `${++list.n}. ` : "- ";
        }
        if (!closing && tag === "hr") out += "----\n";
        continue;
      }
      if (tag === "td" || tag === "th") {
        if (!closing && out && !out.endsWith("\n")) out += "\t";
        continue;
      }
      if (tag === "a") {
        if (!closing) {
          const href = /\bhref\s*=\s*("([^"]*)"|'([^']*)')/i.exec(match[3]);
          links.push({ href: href ? decode(href[2] ?? href[3]) : "", start: out.length });
        } else {
          const link = links.pop();
          const visible = out.slice(link?.start ?? out.length).trim();
          const displayHref = link?.href?.replace(/^(mailto:|tel:)/i, "");
          if (link?.href && /^(https?:|mailto:|tel:)/i.test(link.href) && visible !== link.href && visible !== displayHref) {
            out += ` <${displayHref}>`;
          }
        }
      }
    }
    return out.replace(/[ \t]+\n/g, "\n").replace(/\n{3,}/g, "\n\n").replace(/^\n+|\n+$/g, "");
  }

  /**
   * HTML for change detection: inline image data (often megabytes of base64)
   * is replaced by its length and a sampled hash.
   */
  function fingerprintHtml(html) {
    return String(html || "").replace(/(["'])data:([^"',]*),([^"']*)\1/gi, (_, q, meta, data) => {
      let hash = 2166136261;
      const step = Math.max(1, Math.floor(data.length / 4096));
      for (let i = 0; i < data.length; i += step) hash = Math.imul(hash ^ data.charCodeAt(i), 16777619);
      return `${q}data:${meta},#${data.length}:${(hash >>> 0).toString(36)}:${data.slice(-32)}${q}`;
    });
  }

  /**
   * 发信时正文外面会包一层带字体样式的 <div>；重新打开草稿时去掉这一层，
   * 否则每次打开再发送都会多套一层。只在整段内容恰好被一个 <div> 包住时才去掉。
   */
  function unwrapOuterDiv(html) {
    const text = String(html || "").trim();
    const open = /^<div(\s[^>]*)?>/i.exec(text);
    if (!open || !/<\/div>$/i.test(text)) return String(html || "");
    let depth = 0;
    for (const match of text.matchAll(/<(\/?)div\b(?:[^>"']|"[^"]*"|'[^']*')*>/gi)) {
      depth += match[1] ? -1 : 1;
      if (depth === 0) {
        return match.index + match[0].length === text.length
          ? text.slice(open[0].length, match.index)
          : String(html || "");
      }
    }
    return String(html || "");
  }

  const api = { commandFor, normalizeLink, tableHtml, escapeText, textToHtml, signatureBlock, initialHtml, switchSignature, htmlToText, fingerprintHtml, unwrapOuterDiv };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailPulseEditor = api;
})(typeof window !== "undefined" ? window : globalThis);
