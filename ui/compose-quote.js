/* Keep the original message out of the reply editor, and send a safe HTML quote. */
(function (root) {
  "use strict";
  const SCOPE = ".mailpulse-original";
  // Mail is untrusted input. Keep recursive CSS handling comfortably below the
  // JS stack limit and bound the synchronous work done when opening a reply.
  const MAX_CSS_DEPTH = 32;
  const MAX_CSS_LENGTH = 1024 * 1024;
  const normalize = (value) => String(value || "").replace(/\r\n?/g, "\n");
  const escapeHtml = (value) => String(value || "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const textHtml = (text) => escapeHtml(normalize(text)).replace(/\n/g, "<br>");

  function safeUrl(value, image) {
    const url = String(value || "").trim();
    // Reject controls instead of letting a browser normalize an obfuscated scheme.
    if (!url || /[\u0000-\u0020\u007f<>"'\\]/.test(url)) return false;
    if (/^(?:https?:\/\/|\/\/)/i.test(url)) return true;
    if (image) {
      return /^cid:[^<>"']+$/i.test(url) || /^data:image\/(?:png|jpe?g|gif|webp|bmp|x-icon|vnd\.microsoft\.icon|avif);base64,[a-z0-9+/=]+$/i.test(url);
    }
    return /^(?:mailto:|tel:|#)/i.test(url);
  }

  function decodeCss(value) {
    return value.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\\([0-9a-f]{1,6})(?:\r\n|[\t\n\r\f ])?|\\([^\r\n\f])/gi, (_, hex, char) => {
      const point = hex ? parseInt(hex, 16) : 0;
      return hex ? (point > 0 && point <= 0x10ffff ? String.fromCodePoint(point) : "\ufffd") : char;
    });
  }

  // CSS separators inside strings, selectors or functions are not separators.
  function splitTopLevel(source, delimiter) {
    const parts = [];
    let start = 0, quote = "", round = 0, square = 0, braces = 0;
    for (let i = 0; i < source.length; i++) {
      const c = source[i];
      if (c === "\\") { i++; continue; }
      if (quote) { if (c === quote) quote = ""; continue; }
      if (c === '"' || c === "'") { quote = c; continue; }
      if (c === "(") round++;
      else if (c === ")") round--;
      else if (c === "[") square++;
      else if (c === "]") square--;
      else if (c === "{") braces++;
      else if (c === "}") braces--;
      else if (c === delimiter && !round && !square && !braces) { parts.push(source.slice(start, i)); start = i + 1; }
    }
    parts.push(source.slice(start));
    return parts;
  }

  function safeCssValue(value) {
    const decoded = decodeCss(value);
    if (/[<>\u0000]|(?:expression|javascript|vbscript)\s*[:(]|@import|@charset/i.test(decoded)) return false;
    // URL parsing is deliberately conservative; exotic/escaped quoted URLs can
    // lose a background, but must never turn into a new executable URL.
    const urls = [...decoded.matchAll(/url\s*\(\s*(?:"([^"\r\n]*)"|'([^'\r\n]*)'|([^)'"\s]*))\s*\)/gi)];
    if ((decoded.match(/url\s*\(/gi) || []).length !== urls.length) return false;
    return urls.every((match) => safeUrl(match[1] ?? match[2] ?? match[3], true));
  }

  function sanitizeDeclarations(source) {
    return splitTopLevel(source, ";").filter((part) => {
      const colon = part.indexOf(":");
      if (colon < 1) return false;
      const property = decodeCss(part.slice(0, colon)).trim().toLowerCase();
      return /^-?[a-z][a-z-]*$/.test(property) && !/^(?:behavior|-moz-binding)$/.test(property) && safeCssValue(part.slice(colon + 1));
    }).join(";");
  }

  function scopedSelector(selector) {
    const value = selector.trim();
    if (!value || /[<>@\\]/.test(value)) return "";
    // The quote wrapper carries the original body's class/style attributes.
    // Collapse document roots to that wrapper; all other selectors stay inside it.
    const roots = value.replace(/^(?:html\s*(?:>\s*)?)?(?:body|:root)(?=[\s.#[:>+~]|$)/i, SCOPE)
      .replace(/^html(?=[\s.#[:>+~]|$)/i, SCOPE);
    return roots.startsWith(SCOPE) ? roots : SCOPE + " " + value;
  }

  function sanitizeCss(source, inline = false, nesting = 0) {
    const input = String(source || "");
    if (input.length > MAX_CSS_LENGTH || nesting >= MAX_CSS_DEPTH) return "";
    const css = input.replace(/\/\*[\s\S]*?\*\//g, "");
    if (inline) return sanitizeDeclarations(css);
    let out = "", start = 0, quote = "", depth = 0, open = -1, round = 0;
    for (let i = 0; i < css.length; i++) {
      const c = css[i];
      if (c === "\\") { i++; continue; }
      if (quote) { if (c === quote) quote = ""; continue; }
      if (c === '"' || c === "'") { quote = c; continue; }
      if (c === "(") round++;
      else if (c === ")") round--;
      if (round) continue;
      if (c === ";" && !depth) { start = i + 1; continue; } // No @import or other external rules.
      if (c === "{") { if (!depth) open = i; depth++; }
      if (c !== "}" || !depth || --depth) continue;
      const selector = css.slice(start, open).trim(), body = css.slice(open + 1, i);
      if (/^@(media|supports)\b/i.test(selector) && !/[<>\\]/.test(selector)) {
        const nested = sanitizeCss(body, false, nesting + 1);
        if (nested) out += selector + " { " + nested + " }";
      } else if (!selector.startsWith("@")) {
        const selectors = splitTopLevel(selector, ",").map(scopedSelector).filter(Boolean);
        const declarations = sanitizeDeclarations(body);
        if (selectors.length && declarations.trim()) out += selectors.join(", ") + " {" + declarations + "}";
      }
      start = i + 1;
    }
    return out;
  }

  function cidFromUrl(value) {
    if (!/^cid:/i.test(value) || !safeUrl(value, true)) return null;
    try { return decodeURIComponent(value.slice(4)); } catch (_) { return value.slice(4); }
  }

  function cssCids(css) {
    const cids = [];
    const source = String(css || "").replace(/\/\*[\s\S]*?\*\//g, "");
    let quote = "";
    for (let i = 0; i < source.length; i++) {
      const char = source[i];
      // Preserve escapes while finding string boundaries; decoding \" first
      // would make ordinary quoted content look like an actual URL function.
      if (quote) {
        if (char === "\\") i++;
        else if (char === quote) quote = "";
        continue;
      }
      if (char === '"' || char === "'") { quote = char; continue; }
      if (i && /[a-z0-9_-]/i.test(source[i - 1])) continue;
      const functionName = source.slice(i).match(/^((?:[a-z_-]|\\[0-9a-f]{1,6}(?:\r\n|[\t\n\r\f ])?|\\[^\r\n\f])+)\s*\(/i);
      if (!functionName || decodeCss(functionName[1]).toLowerCase() !== "url") continue;
      const args = source.slice(i + functionName[0].length).match(/^\s*(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^)'"\s]*))\s*\)/);
      if (!args) continue;
      const cid = cidFromUrl(decodeCss(args[1] ?? args[2] ?? args[3]));
      if (cid) cids.push(cid);
      i += functionName[0].length + args[0].length - 1;
    }
    return cids;
  }

  function inertSource(source) {
    // A template fragment ignores the real <body> tag and its attributes. Keep
    // them on an inert marker instead, without touching CSS/text/quoted values.
    let rawTag = "";
    return source.replace(/<!--[\s\S]*?(?:-->|$)|<(\/?)([a-zA-Z][\w:-]*)(?=[\s/>])(?:[^>"']|"[^"]*"|'[^']*')*>/g, (tag, closing, name) => {
      if (!name) return tag;
      const lower = name.toLowerCase();
      if (rawTag) { if (closing && lower === rawTag) rawTag = ""; return tag; }
      if (!closing && /^(?:script|style|textarea|title|xmp|iframe|noembed|noframes|plaintext)$/.test(lower)) rawTag = lower;
      return lower === "body" ? tag.replace(/^(<\/?)body/i, "$1mailpulse-body") : tag;
    });
  }

  function sanitizeHtml(source) {
    if (typeof root.DOMParser !== "function") return null;
    // Parsing real mail directly with DOMParser may fetch img/iframe URLs.
    // Template contents belong to an inert document and never enter the app DOM.
    const emptyDoc = new root.DOMParser().parseFromString("", "text/html");
    const template = emptyDoc.createElement("template");
    template.innerHTML = inertSource(source);
    const fragment = template.content;
    const doc = fragment.ownerDocument;
    // Conditional comments can contain alternate HTML in older mail clients.
    // Remove them rather than forwarding markup that never reached sanitization.
    const comments = [], walker = doc.createTreeWalker(fragment, 128);
    while (walker.nextNode()) comments.push(walker.currentNode);
    for (const comment of comments) comment.remove();
    const allowed = new Set("a abbr address article aside b bdi bdo blockquote br caption center cite code col colgroup dd del details div dl dt em figcaption figure font footer h1 h2 h3 h4 h5 h6 header hr i img ins kbd label li main mark nav ol p pre q rp rt ruby s samp section small span strike strong sub summary sup table tbody td tfoot th thead time tr tt u ul var wbr style".split(" "));
    const remove = new Set("title script noscript iframe frame frameset object embed applet form input button textarea select option optgroup link meta base svg math canvas audio video source track template".split(" "));
    const attrs = new Set("class id style title lang dir align valign width height bgcolor color face size border cellpadding cellspacing colspan rowspan background alt src href name start type reversed value datetime cite".split(" "));
    const cids = new Set();
    // Avoid passing every child as a function argument: a large flat message
    // can exceed the engine's argument/stack limit even without deep nesting.
    const unwrap = (element) => {
      const children = doc.createDocumentFragment();
      while (element.firstChild) children.appendChild(element.firstChild);
      element.replaceWith(children);
    };
    const sanitizeElement = (element) => {
      for (const attr of [...element.attributes]) {
        const key = attr.name.toLowerCase();
        if (!attrs.has(key)) { element.removeAttribute(attr.name); continue; }
        if (key === "style") {
          const style = sanitizeCss(attr.value, true);
          if (style) element.setAttribute("style", style); else element.removeAttribute("style");
        } else if (key === "src" || key === "background") {
          if ((key === "src" && element.localName !== "img") || !safeUrl(attr.value, true)) element.removeAttribute(attr.name);
        } else if ((key === "href" || key === "cite") && !safeUrl(attr.value, false)) element.removeAttribute(attr.name);
      }
      if (element.localName === "a" && element.hasAttribute("href")) {
        element.setAttribute("target", "_blank");
        element.setAttribute("rel", "noopener noreferrer");
      }
    };
    for (const element of [...fragment.querySelectorAll("*")]) {
      if (!fragment.contains(element)) continue;
      if (remove.has(element.localName) || element.namespaceURI !== "http://www.w3.org/1999/xhtml") { element.remove(); continue; }
      if (!allowed.has(element.localName) && element.localName !== "mailpulse-body") { unwrap(element); continue; }
      sanitizeElement(element);
      if (element.localName === "style") element.textContent = sanitizeCss(element.textContent);
    }
    const originalBody = fragment.querySelector("mailpulse-body");
    const wrapper = doc.createElement("div");
    for (const attr of [...(originalBody?.attributes || [])]) wrapper.setAttribute(attr.name, attr.value);
    wrapper.classList.add("mailpulse-original");
    // Legacy body backgrounds otherwise stop rendering when body becomes a div.
    const background = wrapper.getAttribute("background");
    if (background && !wrapper.style.backgroundImage) wrapper.style.backgroundImage = `url("${background}")`;
    const bgcolor = wrapper.getAttribute("bgcolor");
    if (bgcolor && !wrapper.style.backgroundColor && safeCssValue(bgcolor)) wrapper.style.backgroundColor = bgcolor;
    wrapper.appendChild(fragment);
    for (const body of wrapper.querySelectorAll("mailpulse-body")) unwrap(body);
    for (const element of [wrapper, ...wrapper.querySelectorAll("*")]) {
      for (const attr of ["src", "background"]) {
        const cid = cidFromUrl(element.getAttribute(attr) || "");
        if (cid) cids.add(cid);
      }
      for (const cid of cssCids(element.getAttribute("style") || "")) cids.add(cid);
      if (element.localName === "style") for (const cid of cssCids(element.textContent)) cids.add(cid);
    }
    // Text is used only if the message did not supply a text/plain alternative.
    const plain = wrapper.cloneNode(true);
    for (const style of plain.querySelectorAll("style")) style.remove();
    for (const br of plain.querySelectorAll("br")) br.replaceWith("\n");
    for (const block of plain.querySelectorAll("p,div,tr,li,blockquote,h1,h2,h3,h4,h5,h6")) block.append("\n");
    return { html: wrapper.outerHTML, text: normalize(plain.textContent).trim(), cids: [...cids] };
  }

  function createQuote({ html = "", text = "", header = "" } = {}) {
    let clean = null, notice = "";
    if (html) {
      try { clean = sanitizeHtml(String(html)); }
      catch (_) { notice = text ? "原邮件格式无法保留，已改为纯文本引用。" : "原邮件格式无法保留，未附加原文；请检查后再发送。"; }
    }
    const plain = normalize(text || clean?.text || "");
    return {
      html: clean?.html || `<div class="mailpulse-original" style="white-space:normal">${textHtml(plain)}</div>`,
      text: plain,
      header: normalize(header),
      rich: !!clean,
      cids: clean?.cids || [],
      notice,
    };
  }

  /**
   * Outgoing text and HTML bodies. `replyHtml` is the rich editor's content;
   * without it the typed text is escaped into HTML (only when the quote is rich).
   */
  function composeBody(reply, quote, replyHtml) {
    const body = normalize(reply);
    const font = "font:14px/1.6 -apple-system,Arial,sans-serif";
    const doc = (inner) => `<!doctype html><html><head><meta charset="utf-8"></head><body>${inner}</body></html>`;
    const typed = replyHtml != null ? replyHtml : textHtml(body);
    if (!quote) {
      return { body, htmlBody: replyHtml != null ? doc(`<div style="${font}">${typed}</div>`) : null };
    }
    const quoted = normalize(quote.text).split("\n").map((line) => "> " + line).join("\n");
    const result = { body: body + "\n\n" + normalize(quote.header) + "\n" + quoted, htmlBody: null };
    if (quote.rich || replyHtml != null) {
      const original = quote.rich ? quote.html : `<div style="white-space:pre-wrap">${textHtml(quote.text)}</div>`;
      result.htmlBody = doc(
        `<div style="${font};white-space:normal">${typed}</div>` +
        `<div style="margin-top:20px;${font}">${textHtml(quote.header)}</div>` +
        `<blockquote style="margin:8px 0 0;padding-left:12px;border-left:2px solid #ddd">${original}</blockquote>`);
    }
    return result;
  }

  function previewDocument(quote, { allowRemoteImages = false } = {}) {
    const remote = allowRemoteImages ? " https: http:" : "";
    const csp = `default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data: cid:${remote}; font-src data:; form-action 'none'; base-uri 'none'`;
    // Keep table columns wide enough for whole words, just as in the reader.
    return `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>html,body{margin:0;padding:0;background:white;color:#1d1d1f}body{font:14px/1.6 -apple-system,Arial,sans-serif;overflow-wrap:break-word}img{max-width:100%;height:auto}pre{white-space:pre-wrap}</style></head><body>${quote.html}</body></html>`;
  }

  const api = { createQuote, composeBody, previewDocument, sanitizeHtml, sanitizeCss, safeUrl, cssCids, inertSource };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else {
    root.MailBeaconQuote = api;
    // 兼容旧版调试脚本和已打开的草稿页面。
    root.MailPulseQuote = api;
  }
})(typeof globalThis !== "undefined" ? globalThis : this);
