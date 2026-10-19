"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

// ---------------------------------------------------------------------------
// 图标（线条风格，24 网格）
// ---------------------------------------------------------------------------
const ICON = {
  pen: '<path d="M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/><path d="M18.4 2.6a1 1 0 0 1 3 3l-9 9a2 2 0 0 1-.85.5l-2.87.84a.5.5 0 0 1-.62-.62l.84-2.87a2 2 0 0 1 .5-.85z"/>',
  search: '<circle cx="11" cy="11" r="7.5"/><path d="m20.5 20.5-4-4"/>',
  sliders: '<path d="M21 4h-7M10 4H3M21 12h-9M8 12H3M21 20h-5M12 20H3M14 2v4M8 10v4M16 18v4"/>',
  inbox: '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
  star: '<path d="M12 2.5l2.94 5.96 6.56.95-4.75 4.63 1.12 6.53L12 17.5l-5.87 3.07 1.12-6.53L2.5 9.41l6.56-.95z"/>',
  draft: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z"/><path d="M14 2v4a2 2 0 0 0 2 2h4M16 13H8M16 17H8M10 9H8"/>',
  sent: '<path d="M22 2 15 22l-4-9-9-4z"/><path d="M22 2 11 13"/>',
  mail: '<rect x="2" y="4" width="20" height="16" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/>',
  junk: '<circle cx="12" cy="12" r="9.5"/><path d="M12 7.5v5M12 16.5h.01"/>',
  trash: '<path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>',
  archive: '<rect x="2" y="3" width="20" height="5" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8M10 12h4"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z"/>',
  x: '<path d="M18 6 6 18M6 6l12 12"/>',
  left: '<path d="m15 18-6-6 6-6"/>',
  right: '<path d="m9 18 6-6-6-6"/>',
  updown: '<path d="m7 15 5 5 5-5M7 9l5-5 5 5"/>',
  more: '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
  filter: '<path d="M22 3H2l8 9.46V19l4 2v-8.54z"/>',
  reply: '<path d="M9 17 4 12l5-5"/><path d="M20 18v-2a4 4 0 0 0-4-4H4"/>',
  replyAll: '<path d="m7 17-5-5 5-5M12 17l-5-5 5-5"/><path d="M22 18v-2a4 4 0 0 0-4-4H7"/>',
  forward: '<path d="m15 17 5-5-5-5"/><path d="M4 18v-2a4 4 0 0 1 4-4h12"/>',
  clip: '<path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
  unread: '<path d="M21.2 8.4c.5.38.8.97.8 1.6v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V10a2 2 0 0 1 .8-1.6l8-6a2 2 0 0 1 2.4 0z"/><path d="m22 10-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 10"/>',
  plus: '<path d="M5 12h14M12 5v14"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  checkAll: '<path d="M18 6 7 17l-5-5M22 10l-7.5 7.5L13 16"/>',
  refresh: '<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/>',
};
const icon = (name, cls = "") => `<svg class="i ${cls}" viewBox="0 0 24 24">${ICON[name]}</svg>`;

const ROLE_ICON = { inbox: "inbox", drafts: "draft", sent: "sent", all: "mail", junk: "junk", trash: "trash", archive: "archive", other: "folder" };
const KINDS = [
  { id: "exmail", label: "腾讯企业邮", hint: "网页版 设置 → 客户端设置 → 开启 IMAP/SMTP；开了「安全登录」的需要生成客户端专用密码。" },
  { id: "qq", label: "QQ 邮箱", hint: "网页版 设置 → 账号 → 开启 IMAP/SMTP 服务 → 生成授权码。" },
  { id: "163", label: "网易 163", hint: "网页版 设置 → POP3/SMTP/IMAP → 开启 IMAP/SMTP → 新增授权密码。" },
  { id: "126", label: "网易 126", hint: "网页版 设置 → POP3/SMTP/IMAP → 开启 IMAP/SMTP → 新增授权密码。" },
  { id: "gmail", label: "Gmail", hint: "先开启两步验证，再到 myaccount.google.com/apppasswords 生成应用专用密码。" },
  { id: "custom", label: "其他", hint: "填写服务器地址。SMTP 端口 587 走 STARTTLS，465 等其他端口走 SSL。" },
];
const PALETTE = ["#2f6fec", "#d6409f", "#12a594", "#e5484d", "#8e4ec6", "#f76b15", "#0090ff", "#30a46c", "#ab4aba", "#e54666"];

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------
const S = {
  accounts: [],
  live: {},            // accountId → { state, detail, unread }
  folders: {},         // accountId → [{ path, name, role }]
  lists: new Map(),    // "acct|folder" → [summary]（folder 也可能是 "__starred"）
  loading: new Set(),  // 正在加载的 "acct|folder"
  exhausted: new Set(),// 已经翻到底的 "acct|folder"
  previews: new Map(), // "acct|folder|uid" → 摘要
  acct: "all",         // 当前账号，或 "all"
  folder: "INBOX",     // 当前文件夹 path，或 "__starred"
  sel: null,           // { accountId, folder, uid }
  msg: null,           // 打开的邮件正文
  msgLoading: false,
  msgError: "",
  showImages: new Set(),
  searching: false,
  query: "",
  unreadOnly: false,
};

const $ = (sel, root = document) => root.querySelector(sel);
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const key = (a, f) => `${a}|${f}`;
const account = (id) => S.accounts.find((a) => a.id === id);
const colorOf = (s) => {
  let h = 0;
  for (const c of String(s)) h = (h * 31 + c.codePointAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length];
};
const initial = (s) => (String(s || "?").trim()[0] || "?").toUpperCase();
const avatar = (label, seed, cls = "") => `<div class="avatar ${cls}" style="background:${colorOf(seed)}">${esc(initial(label))}</div>`;

function fmtDate(ms, long = false) {
  if (!ms) return "";
  const d = new Date(ms);
  const now = new Date();
  const hm = `${d.getHours()}:${String(d.getMinutes()).padStart(2, "0")}`;
  if (long) {
    const y = d.getFullYear() === now.getFullYear() ? "" : `${d.getFullYear()}年`;
    return `${y}${d.getMonth() + 1}月${d.getDate()}日 ${hm}`;
  }
  const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = (day(now) - day(d)) / 86400000;
  if (diff === 0) return hm;
  if (diff === 1) return "昨天";
  if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日`;
  return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
}

function fmtSize(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

let toastTimer;
function toast(text, isErr = false) {
  const t = $("#toast");
  t.textContent = text;
  t.className = "toast" + (isErr ? " err" : "");
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), isErr ? 5000 : 2400);
}
const fail = (e) => toast(String(e), true);

// ---------------------------------------------------------------------------
// 数据
// ---------------------------------------------------------------------------
function folderInfo(accountId, path) {
  return (S.folders[accountId] || []).find((f) => f.path === path);
}
function folderByRole(accountId, role) {
  return (S.folders[accountId] || []).find((f) => f.role === role);
}

/** 当前视图里的邮件（已按时间排好、按搜索和「只看未读」过滤） */
function currentList() {
  const items0 = S.acct === "all"
    ? S.accounts.flatMap((a) => S.lists.get(key(a.id, S.folder)) || [])
    : (S.lists.get(key(S.acct, S.folder)) || []).slice();
  // 按收件时间排，不按 UID：导入/迁移过的邮箱里两者顺序可能不一致
  let items = items0.sort((a, b) => b.date - a.date || b.uid - a.uid);
  if (S.unreadOnly) items = items.filter((m) => !m.seen || isSel(m));
  const q = S.query.trim().toLowerCase();
  if (q) {
    items = items.filter((m) =>
      [m.fromName, m.fromAddr, m.subject, m.to, S.previews.get(key(key(m.accountId, m.folder), m.uid))]
        .some((x) => x && x.toLowerCase().includes(q)));
  }
  return items;
}
const isSel = (m) => S.sel && S.sel.accountId === m.accountId && S.sel.folder === m.folder && S.sel.uid === m.uid;

function findMsg(sel) {
  if (!sel) return null;
  for (const [k, list] of S.lists) {
    if (!k.startsWith(sel.accountId + "|")) continue;
    const m = list.find((x) => x.folder === sel.folder && x.uid === sel.uid);
    if (m) return m;
  }
  return null;
}

/** 本地修改某封邮件（所有列表里的副本一起改） */
function patchMsg(accountId, folder, uid, patch) {
  for (const [k, list] of S.lists) {
    if (!k.startsWith(accountId + "|")) continue;
    for (const m of list) if (m.folder === folder && m.uid === uid) Object.assign(m, patch);
  }
}
function dropMsg(accountId, folder, uid) {
  for (const [k, list] of S.lists) {
    if (!k.startsWith(accountId + "|")) continue;
    S.lists.set(k, list.filter((m) => !(m.folder === folder && m.uid === uid)));
  }
}

async function loadFolders(accountId) {
  try {
    S.folders[accountId] = await invoke("list_folders", { accountId });
    render();
  } catch (e) {
    console.warn("读取文件夹失败", e);
  }
}

async function loadList(accountId, folder, { more = false } = {}) {
  const k = key(accountId, folder);
  if (S.loading.has(k)) return;
  S.loading.add(k);
  render();
  try {
    const existing = S.lists.get(k) || [];
    const before = more && existing.length ? Math.min(...existing.map((m) => m.uid)) : null;
    const starred = folder === "__starred";
    const got = await invoke("list_messages", {
      accountId,
      folder: starred ? "INBOX" : folder,
      before,
      flaggedOnly: starred,
    });
    if (more) {
      S.lists.set(k, existing.concat(got));
      if (got.length === 0) S.exhausted.add(k);
    } else {
      S.lists.set(k, got);
      S.exhausted.delete(k);
    }
    loadPreviews(accountId, starred ? "INBOX" : folder, got);
  } catch (e) {
    fail(e);
  } finally {
    S.loading.delete(k);
    render();
  }
}

const previewQueue = new Set();
async function loadPreviews(accountId, folder, msgs) {
  const need = msgs.filter((m) => !S.previews.has(key(key(accountId, folder), m.uid))).map((m) => m.uid);
  for (let i = 0; i < need.length; i += 40) {
    const chunk = need.slice(i, i + 40).filter((u) => !previewQueue.has(key(key(accountId, folder), u)));
    if (!chunk.length) continue;
    chunk.forEach((u) => previewQueue.add(key(key(accountId, folder), u)));
    try {
      const got = await invoke("get_previews", { accountId, folder, uids: chunk });
      for (const p of got) S.previews.set(key(key(accountId, folder), p.uid), p.text);
      renderList();
    } catch (e) {
      console.warn("摘要加载失败", e);
    } finally {
      chunk.forEach((u) => previewQueue.delete(key(key(accountId, folder), u)));
    }
  }
}

function updateBadge() {
  const total = S.accounts.reduce((n, a) => n + (S.live[a.id]?.unread || 0), 0);
  invoke("set_badge", { count: total }).catch(() => {});
}

function onMailEvent(ev) {
  const a = ev.accountId;
  // 刚添加的账号：同步任务可能比 add_account 先返回结果，事件照样记下，不能丢
  const live = (S.live[a] ||= { state: "connecting", detail: "", unread: 0 });
  switch (ev.type) {
    case "status":
      live.state = ev.state;
      live.detail = ev.detail;
      if (ev.state === "online" && !S.folders[a]) loadFolders(a);
      break;
    case "inbox":
      S.lists.set(key(a, "INBOX"), ev.messages);
      loadPreviews(a, "INBOX", ev.messages);
      break;
    case "newMail": {
      const list = S.lists.get(key(a, "INBOX")) || [];
      const fresh = ev.messages.filter((m) => !list.some((x) => x.uid === m.uid));
      S.lists.set(key(a, "INBOX"), fresh.concat(list));
      loadPreviews(a, "INBOX", fresh);
      break;
    }
    case "flags": {
      const byUid = new Map(ev.items.map((f) => [f.uid, f]));
      const list = S.lists.get(key(a, "INBOX")) || [];
      const kept = [];
      for (const m of list) {
        const f = byUid.get(m.uid);
        if (f) {
          m.seen = f.seen;
          m.flagged = f.flagged;
          kept.push(m);
        } else if (m.uid < ev.minUid || isSel(m)) {
          kept.push(m);
        }
      }
      S.lists.set(key(a, "INBOX"), kept);
      break;
    }
    case "unread":
      live.unread = ev.count;
      updateBadge();
      break;
  }
  render();
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------
function render() {
  if (!S.accounts.length) {
    renderWelcome();
    return;
  }
  renderSidebar();
  renderList();
  renderReader();
}

function syncSummary() {
  const states = S.accounts.map((a) => S.live[a.id]?.state || "connecting");
  if (states.includes("error")) {
    const bad = S.accounts.find((a) => S.live[a.id]?.state === "error");
    return { cls: "error", text: `${bad.email} 登录失败` };
  }
  if (states.includes("offline")) return { cls: "offline", text: "网络断开，正在重连…" };
  if (states.includes("connecting")) return { cls: "connecting", text: "正在同步…" };
  return { cls: "", text: "已同步 · 实时推送" };
}

function navItem(label, iconName, active, attrs, count) {
  return `<button class="nav-item ${active ? "active" : ""}" ${attrs}>
    ${icon(iconName)}<span class="label">${esc(label)}</span>${count ? `<span class="count">${count}</span>` : ""}
  </button>`;
}

function renderSidebar() {
  const all = S.acct === "all";
  const a = account(S.acct);
  const card = all
    ? `<div class="avatar all">${icon("inbox")}</div><div class="meta"><div class="name">所有账户</div><div class="mail">${S.accounts.length} 个邮箱</div></div>`
    : `${avatar(a.name || a.email, a.email)}<div class="meta"><div class="name">${esc(a.name)}</div><div class="mail">${esc(a.email)}</div></div>`;

  const unread = all
    ? S.accounts.reduce((n, x) => n + (S.live[x.id]?.unread || 0), 0)
    : S.live[S.acct]?.unread || 0;

  let nav = navItem("收件箱", "inbox", S.folder === "INBOX", 'data-go="INBOX"', unread || "");
  nav += navItem("已加星标", "star", S.folder === "__starred", 'data-go="__starred"');
  let more = "";
  if (all) {
    more = `<div class="side-label">邮箱</div>` + S.accounts.map((x) =>
      `<button class="nav-item" data-switch="${esc(x.id)}">${avatar(x.name || x.email, x.email, "sm")}<span class="label">${esc(x.email)}</span>${S.live[x.id]?.unread ? `<span class="count">${S.live[x.id].unread}</span>` : ""}</button>`).join("");
  } else {
    const folders = S.folders[S.acct] || [];
    for (const role of ["drafts", "sent", "all", "archive", "junk", "trash"]) {
      const f = folders.find((x) => x.role === role);
      if (f) nav += navItem(f.name, ROLE_ICON[role], S.folder === f.path, `data-go="${esc(f.path)}"`);
    }
    const others = folders.filter((x) => x.role === "other");
    if (others.length) {
      more = `<div class="side-label">文件夹</div>` + others.map((f) => navItem(f.name, "folder", S.folder === f.path, `data-go="${esc(f.path)}"`)).join("");
    }
  }

  const sync = syncSummary();
  $("#sidebar").innerHTML = `
    <div class="side-top" data-tauri-drag-region></div>
    <div class="side-scroll">
      <button class="account-card" data-action="account-menu">${card}${icon("updown", "chev")}</button>
      <button class="compose-btn" data-action="compose">${icon("pen")}写邮件</button>
      ${navItem("搜索", "search", false, 'data-action="search"')}
      ${navItem("设置", "sliders", false, 'data-action="settings"')}
      <div class="side-sep"></div>
      <div class="side-label">邮件</div>
      ${nav}
      ${more}
    </div>
    <div class="side-foot" title="${esc(S.accounts.map((x) => `${x.email}：${S.live[x.id]?.detail || S.live[x.id]?.state || ""}`).join("\n"))}">
      <span class="dot ${sync.cls}"></span>${esc(sync.text)}
    </div>`;
}

function folderTitle() {
  if (S.folder === "INBOX") return S.acct === "all" ? "所有收件箱" : "收件箱";
  if (S.folder === "__starred") return "已加星标";
  return folderInfo(S.acct, S.folder)?.name || S.folder;
}

function renderList() {
  const items = currentList();
  const k = key(S.acct, S.folder);
  const loading = S.acct === "all" ? S.accounts.some((a) => S.loading.has(key(a.id, S.folder))) : S.loading.has(k);
  const waitingInbox = S.folder === "INBOX" && (S.acct === "all"
    ? S.accounts.every((a) => !S.lists.has(key(a.id, "INBOX")))
    : !S.lists.has(key(S.acct, "INBOX")));
  const multi = S.accounts.length > 1 && S.acct === "all";

  const head = S.searching
    ? `<div class="search-box">${icon("search")}<input id="q" placeholder="搜索发件人、主题、内容" value="${esc(S.query)}" autocomplete="off" spellcheck="false"></div>
       <button class="icon-btn" data-action="search-close" title="取消">${icon("x")}</button>`
    : `<h1 data-tauri-drag-region>${esc(folderTitle())}</h1>
       <button class="icon-btn ${S.unreadOnly ? "on" : ""}" data-action="unread-only" title="只看未读">${icon("filter")}</button>
       <button class="icon-btn" data-action="list-menu" title="更多">${icon("more")}</button>`;

  let body;
  if (!items.length && (loading || waitingInbox)) {
    body = Array.from({ length: 8 }, () => `<div class="skeleton"><i style="width:${40 + Math.random() * 25}%"></i><i style="width:${70 + Math.random() * 25}%"></i></div>`).join("");
  } else if (!items.length) {
    body = `<div class="list-empty">${S.query ? "没有找到匹配的邮件" : S.unreadOnly ? "没有未读邮件" : "这里没有邮件"}</div>`;
  } else {
    body = items.map((m) => {
      const who = S.folder !== "INBOX" && folderInfo(m.accountId, m.folder)?.role === "sent" ? (m.to ? `发给 ${m.to}` : "(无收件人)") : (m.fromName || m.fromAddr || "(未知发件人)");
      const prev = S.previews.get(key(key(m.accountId, m.folder), m.uid)) || "";
      return `<button class="row ${m.seen ? "" : "unread"} ${isSel(m) ? "sel" : ""}" data-open="${esc(m.accountId)}|${m.uid}|${esc(m.folder)}">
        ${m.seen ? "" : '<span class="unread-dot"></span>'}
        <div class="l1"><span class="from">${esc(who)}</span><span class="date">
          ${m.flagged ? icon("star", "star") : ""}${m.attachment ? icon("clip") : ""}
          ${multi ? `<span class="acct-dot" style="background:${colorOf(account(m.accountId)?.email)}"></span>` : ""}${esc(fmtDate(m.date))}</span></div>
        <div class="l2"><span class="subj">${esc(m.subject || "(无主题)")}</span><span class="prev">${esc(prev)}</span></div>
      </button>`;
    }).join("");
    const canMore = S.acct !== "all" && S.folder !== "__starred" && !S.query && !S.exhausted.has(k) && items.length >= 100;
    if (canMore) body += `<button class="list-more" data-action="more">${loading ? "正在加载…" : "加载更早的邮件"}</button>`;
  }

  const list = $("#list");
  const rows = $(".rows", list);
  const scroll = rows ? rows.scrollTop : 0;
  const hadFocus = document.activeElement?.id === "q";
  const caret = hadFocus ? document.activeElement.selectionStart : 0;
  list.innerHTML = `<div class="list-head" data-tauri-drag-region>${head}</div><div class="rows">${body}</div>`;
  $(".rows", list).scrollTop = scroll;
  if (hadFocus) {
    const q = $("#q");
    q.focus();
    q.setSelectionRange(caret, caret);
  }
}

function renderReader() {
  const r = $("#reader");
  const m = findMsg(S.sel);
  if (!S.sel || !m) {
    r.innerHTML = `<div class="toolbar" data-tauri-drag-region></div>
      <div class="reader-empty"><div>${icon("mail", "big")}<div>没有选中邮件</div></div></div>`;
    return;
  }
  const items = currentList();
  const idx = items.findIndex(isSel);
  const role = folderInfo(m.accountId, m.folder)?.role || (m.folder === "INBOX" ? "inbox" : "other");
  const hasArchive = folderByRole(m.accountId, "archive") || folderByRole(m.accountId, "all");
  const v = S.msg;

  const toolbar = `<div class="toolbar" data-tauri-drag-region>
      <button class="tb-btn plain" data-action="close" title="关闭 (Esc)">${icon("x")}</button>
      <div class="vsep"></div>
      <button class="tb-btn" data-action="prev" title="上一封 (K)" ${idx <= 0 ? "disabled" : ""}>${icon("left")}</button>
      <button class="tb-btn" data-action="next" title="下一封 (J)" ${idx < 0 || idx >= items.length - 1 ? "disabled" : ""}>${icon("right")}</button>
      <div class="spacer" data-tauri-drag-region></div>
      <button class="tb-btn" data-action="reply" title="回复 (R)">${icon("reply")}</button>
      <button class="tb-btn" data-action="reply-all" title="全部回复 (A)">${icon("replyAll")}</button>
      <button class="tb-btn" data-action="forward" title="转发 (F)">${icon("forward")}</button>
      <div class="vsep"></div>
      <button class="tb-btn" data-action="junk" title="${role === "junk" ? "不是垃圾邮件" : "标为垃圾邮件"}">${icon("junk")}</button>
      <button class="tb-btn" data-action="move-menu" title="移动到…">${icon("folder")}</button>
      <button class="tb-btn" data-action="archive" title="归档 (E)" ${hasArchive && role !== "archive" && role !== "all" ? "" : "disabled"}>${icon("archive")}</button>
      <button class="tb-btn" data-action="delete" title="${role === "trash" ? "彻底删除" : "删除 (⌫)"}">${icon("trash")}</button>
      <button class="tb-btn" data-action="msg-menu" title="更多">${icon("more")}</button>
    </div>`;

  const subject = v?.subject || m.subject || "(无主题)";
  let card;
  if (S.msgError) {
    card = `<div class="mail-card"><div class="list-empty">${esc(S.msgError)}<br><br><button class="secondary" data-action="retry">重试</button></div></div>`;
  } else if (!v) {
    card = `<div class="mail-card"><div class="mail-head">${avatar(m.fromName || m.fromAddr, m.fromAddr)}
      <div class="who"><div class="line1"><span class="name">${esc(m.fromName || m.fromAddr)}</span><span class="addr">${esc(m.fromAddr)}</span></div></div>
      <div class="when">${esc(fmtDate(m.date, true))}</div></div>
      <div class="mail-body"><div class="skeleton" style="border:0;padding:24px 26px"><i style="width:80%"></i><i style="width:92%"></i><i style="width:64%"></i></div></div></div>`;
  } else {
    const me = account(m.accountId)?.email.toLowerCase();
    const names = (list) => list.map((p) => (p.addr.toLowerCase() === me ? "我" : p.name || p.addr)).join("、");
    const toLine = [v.to.length ? `收件人：${esc(names(v.to))}` : "", v.cc.length ? `抄送：${esc(names(v.cc))}` : ""].filter(Boolean).join("　");
    const imgKey = key(key(m.accountId, m.folder), m.uid);
    const showImg = S.showImages.has(imgKey) || S.showImages.has("from:" + v.from.addr.toLowerCase());
    const imgBar = v.html && v.hasRemote && !showImg
      ? `<div class="img-bar"><button data-action="show-images">显示图片</button><button data-action="always-images" style="background:none;padding:0">始终显示此发件人的图片</button></div>` : "";
    const atts = v.attachments.length
      ? `<div class="atts">${v.attachments.map((x) => `<button class="att" data-att="${x.index}" title="保存到「下载」">${icon("file")}<span class="n">${esc(x.name)}</span><span class="s">${fmtSize(x.size)}</span></button>`).join("")}</div>` : "";
    const body = v.html
      ? `<iframe id="mail-frame" sandbox="allow-same-origin allow-popups" referrerpolicy="no-referrer"></iframe>`
      : `<div class="plain">${linkify(v.text)}</div>`;
    card = `<div class="mail-card">
      <div class="mail-head">${avatar(v.from.name || v.from.addr, v.from.addr)}
        <div class="who"><div class="line1"><span class="name">${esc(v.from.name || v.from.addr)}</span><span class="addr">${esc(v.from.name ? v.from.addr : "")}</span></div>
        <div class="to">${toLine}</div></div>
        <div class="when">${esc(fmtDate(v.date || m.date, true))}</div>
      </div>${imgBar}${atts}
      <div class="mail-body">${body}</div>
    </div>
    <div class="reply-bar">
      <button class="pill" data-action="reply">${icon("reply")}回复</button>
      <button class="pill" data-action="reply-all">${icon("replyAll")}全部回复</button>
      <button class="pill" data-action="forward">${icon("forward")}转发</button>
    </div>`;
  }

  // 正文 iframe 已经在显示同一封邮件时不重建，避免闪烁和滚动位置丢失
  const sig = `${key(key(m.accountId, m.folder), m.uid)}|${v ? "v" : ""}|${S.msgError}|${S.showImages.size}|${m.flagged}|${idx}|${items.length}|${role}`;
  if (r.dataset.sig === sig) return;
  r.dataset.sig = sig;
  r.innerHTML = `${toolbar}<div class="read-scroll">
    <div class="subject-line"><h2>${esc(subject)}</h2>
      <button class="star-btn ${m.flagged ? "on" : ""}" data-action="star" title="星标 (S)">${icon("star")}</button></div>
    ${card}</div>`;
  if (v?.html) mountFrame(v, m);
}

function linkify(text) {
  return esc(text).replace(/(https?:\/\/[^\s<>"']+[^\s<>"'.,;:!?)\]）。，])/g, '<a href="$1" data-link="$1">$1</a>');
}

function mountFrame(v, m) {
  const frame = $("#mail-frame");
  const imgKey = key(key(m.accountId, m.folder), m.uid);
  const remote = S.showImages.has(imgKey) || S.showImages.has("from:" + v.from.addr.toLowerCase());
  // 默认拦截远程图片（防止追踪像素），点「显示图片」后放开
  const csp = `default-src 'none'; style-src 'unsafe-inline' data:${remote ? " https: http:" : ""}; font-src data:${remote ? " https: http:" : ""}; img-src data: cid:${remote ? " https: http:" : ""}`;
  frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8">
    <meta http-equiv="Content-Security-Policy" content="${csp}">
    <base target="_blank">
    <style>html,body{margin:0;background:#fff;color:#1d1d1f}body{padding:22px 26px;font:14px/1.6 -apple-system,"PingFang SC",sans-serif;overflow-wrap:anywhere}
    img{max-width:100%;height:auto}table{max-width:100%!important}pre{white-space:pre-wrap}a{color:#2f6fec}</style></head><body>${v.html}</body></html>`;
  frame.onload = () => {
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => (frame.style.height = Math.max(120, doc.documentElement.scrollHeight) + "px");
    fit();
    new ResizeObserver(fit).observe(doc.documentElement);
    doc.addEventListener("click", (e) => {
      const a = e.target.closest && e.target.closest("a[href]");
      if (!a) return;
      e.preventDefault();
      invoke("open_link", { url: a.href }).catch(fail);
    });
    doc.addEventListener("keydown", (e) => onKey(e));
  };
}

// ---------------------------------------------------------------------------
// 操作
// ---------------------------------------------------------------------------
let openSeq = 0;
async function openMessage(sel) {
  S.sel = sel;
  S.msg = null;
  S.msgError = "";
  const m = findMsg(sel);
  const wasUnread = m && !m.seen;
  if (wasUnread) {
    patchMsg(sel.accountId, sel.folder, sel.uid, { seen: true });
    if (sel.folder === "INBOX" && S.live[sel.accountId]) {
      S.live[sel.accountId].unread = Math.max(0, S.live[sel.accountId].unread - 1);
      updateBadge();
    }
  }
  render();
  $(".row.sel")?.scrollIntoView({ block: "nearest" });
  const seq = ++openSeq;
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const v = await invoke("get_message", { accountId: sel.accountId, folder: sel.folder, uid: sel.uid });
      if (seq !== openSeq) return;
      S.msg = v;
      S.msgError = "";
      break;
    } catch (e) {
      if (seq !== openSeq) return;
      S.msgError = String(e);
    }
  }
  render();
}

function step(delta) {
  const items = currentList();
  if (!items.length) return;
  let i = items.findIndex(isSel);
  i = i < 0 ? 0 : Math.min(items.length - 1, Math.max(0, i + delta));
  const m = items[i];
  if (!isSel(m)) openMessage({ accountId: m.accountId, folder: m.folder, uid: m.uid });
}

/** 把当前邮件移走（删除/归档/垃圾邮件/移动），并自动打开下一封 */
async function moveCurrent(role, target) {
  const m = findMsg(S.sel);
  if (!m) return;
  const items = currentList();
  const i = items.findIndex(isSel);
  const next = items[i + 1] || items[i - 1];
  const sel = S.sel;
  dropMsg(m.accountId, m.folder, m.uid);
  if (next) openMessage({ accountId: next.accountId, folder: next.folder, uid: next.uid });
  else { S.sel = null; S.msg = null; render(); }
  try {
    await invoke("move_messages", { accountId: sel.accountId, folder: sel.folder, uids: [sel.uid], role, target: target || null });
    const words = { trash: "已删除", archive: "已归档", junk: "已移到垃圾邮件", inbox: "已移回收件箱" };
    toast(target ? `已移到「${folderInfo(sel.accountId, target)?.name || target}」` : words[role] || "已移动");
  } catch (e) {
    fail(e);
    loadList(sel.accountId, sel.folder === "INBOX" && S.folder === "__starred" ? "__starred" : sel.folder);
  }
}

async function setFlag(m, flag, value) {
  patchMsg(m.accountId, m.folder, m.uid, flag === "seen" ? { seen: value } : { flagged: value });
  if (flag === "seen" && m.folder === "INBOX" && S.live[m.accountId]) {
    S.live[m.accountId].unread = Math.max(0, S.live[m.accountId].unread + (value ? -1 : 1));
    updateBadge();
  }
  render();
  try {
    await invoke("set_flag", { accountId: m.accountId, folder: m.folder, uids: [m.uid], flag, value });
  } catch (e) {
    fail(e);
  }
}

async function markAllRead() {
  const items = currentList().filter((m) => !m.seen);
  const groups = new Map();
  for (const m of items) {
    const k = key(m.accountId, m.folder);
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(m.uid);
    m.seen = true;
  }
  render();
  for (const [k, uids] of groups) {
    const [accountId, folder] = [k.slice(0, k.indexOf("|")), k.slice(k.indexOf("|") + 1)];
    try {
      await invoke("set_flag", { accountId, folder, uids, flag: "seen", value: true });
    } catch (e) {
      fail(e);
    }
  }
  toast(`已将 ${items.length} 封标为已读`);
}

function go(folder, acct = S.acct) {
  S.acct = acct;
  S.folder = folder;
  S.sel = null;
  S.msg = null;
  S.unreadOnly = false;
  const targets = acct === "all" ? S.accounts.map((a) => a.id) : [acct];
  for (const id of targets) {
    if (folder !== "INBOX") loadList(id, folder);
    if (!S.folders[id]) loadFolders(id);
  }
  render();
}

// ---------------------------------------------------------------------------
// 弹层：菜单、写信、设置
// ---------------------------------------------------------------------------
function closeOverlay() {
  $("#overlay").innerHTML = "";
}

function showMenu(anchor, entries) {
  const r = anchor.getBoundingClientRect();
  const html = entries.map((e) => e === "-" ? '<div class="sep"></div>'
    : `<button data-mi="${entries.indexOf(e)}">${e.check !== undefined ? `<span class="check">${e.check ? icon("check") : ""}</span>` : e.icon ? icon(e.icon) : ""}<span>${esc(e.label)}</span>${e.sub ? `<span class="sub">${esc(e.sub)}</span>` : ""}</button>`).join("");
  $("#overlay").innerHTML = `<div class="scrim" style="background:none" data-close></div><div class="menu">${html}</div>`;
  const menu = $(".menu");
  const w = menu.offsetWidth, h = menu.offsetHeight;
  menu.style.left = Math.min(r.left, innerWidth - w - 8) + "px";
  menu.style.top = (r.bottom + h + 6 > innerHeight ? r.top - h - 6 : r.bottom + 6) + "px";
  menu.onclick = (ev) => {
    const b = ev.target.closest("[data-mi]");
    if (!b) return;
    closeOverlay();
    entries[+b.dataset.mi].run();
  };
}

function accountMenu(anchor) {
  const entries = [];
  if (S.accounts.length > 1) entries.push({ label: "所有账户", check: S.acct === "all", run: () => go("INBOX", "all") });
  for (const a of S.accounts) entries.push({ label: a.email, check: S.acct === a.id, run: () => go("INBOX", a.id) });
  entries.push("-", { label: "添加邮箱…", icon: "plus", run: () => openSettings(true) });
  showMenu(anchor, entries);
}

function quote(v) {
  const who = v.from.name ? `${v.from.name} <${v.from.addr}>` : v.from.addr;
  const body = (v.text || "").replace(/\r/g, "").split("\n").map((l) => "> " + l).join("\n");
  return `\n\n在 ${fmtDate(v.date, true)}，${who} 写道：\n${body}`;
}

function draftFor(mode) {
  const m = findMsg(S.sel);
  const v = S.msg;
  if (!m || !v) return null;
  const me = account(m.accountId).email.toLowerCase();
  const fmt = (p) => (p.name ? `${p.name} <${p.addr}>` : p.addr);
  const strip = (s) => s.replace(/^((re|fw|fwd|回复|答复|转发)\s*[:：]\s*)+/i, "");
  if (mode === "forward") {
    const head = `\n\n---------- 转发的邮件 ----------\n发件人：${fmt(v.from)}\n日期：${fmtDate(v.date, true)}\n主题：${v.subject}\n收件人：${v.to.map(fmt).join(", ")}\n\n`;
    return { accountId: m.accountId, to: "", cc: "", subject: "Fwd: " + strip(v.subject), body: head + (v.text || ""), note: v.attachments.length ? "转发暂不带原附件" : "" };
  }
  let to = [v.from];
  let cc = [];
  if (mode === "reply-all") {
    to = to.concat(v.to).filter((p) => p.addr.toLowerCase() !== me);
    cc = v.cc.filter((p) => p.addr.toLowerCase() !== me);
  }
  if (v.from.addr.toLowerCase() === me) to = v.to; // 回复自己发出的邮件 → 发给原收件人
  const uniq = (l) => l.filter((p, i) => l.findIndex((x) => x.addr.toLowerCase() === p.addr.toLowerCase()) === i);
  return {
    accountId: m.accountId,
    to: uniq(to).map(fmt).join(", "),
    cc: uniq(cc).map(fmt).join(", "),
    subject: "Re: " + strip(v.subject),
    body: quote(v),
    inReplyTo: v.messageId,
    references: v.references,
  };
}

function openCompose(d = {}) {
  const from = d.accountId || (S.acct !== "all" ? S.acct : S.accounts[0].id);
  const showCc = !!d.cc;
  $("#overlay").innerHTML = `<div class="scrim"><div class="sheet" style="height:min(640px, calc(100vh - 80px))">
    <div class="sheet-head"><h3>${esc(d.subject ? d.subject : "新邮件")}</h3><button class="icon-btn" data-action="compose-close" title="关闭">${icon("x")}</button></div>
    ${S.accounts.length > 1 ? `<div class="field"><label>发件人</label><select id="c-from">${S.accounts.map((a) => `<option value="${esc(a.id)}" ${a.id === from ? "selected" : ""}>${esc(a.name)} &lt;${esc(a.email)}&gt;</option>`).join("")}</select></div>` : ""}
    <div class="field"><label>收件人</label><input id="c-to" value="${esc(d.to || "")}" spellcheck="false">${showCc ? "" : '<button class="link" data-action="show-cc">抄送 / 密送</button>'}</div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>抄送</label><input id="c-cc" value="${esc(d.cc || "")}" spellcheck="false"></div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>密送</label><input id="c-bcc" spellcheck="false"></div>
    <div class="field"><label>主题</label><input id="c-subject" value="${esc(d.subject || "")}"></div>
    <textarea class="compose-body" id="c-body" placeholder="写点什么…">${esc(d.body || "")}</textarea>
    <div class="sheet-foot"><span class="hint">${esc(d.note || "多个收件人用逗号分隔 · ⌘↩ 发送")}</span>
      <button class="secondary" data-action="compose-close">取消</button>
      <button class="primary" data-action="send">${icon("sent")}发送</button></div>
  </div></div>`;
  const sheet = $(".sheet");
  sheet.dataset.from = from;
  sheet.dataset.inReplyTo = d.inReplyTo || "";
  sheet.dataset.references = JSON.stringify(d.references || []);
  const focus = d.to ? $("#c-body") : $("#c-to");
  focus.focus();
  if (focus.id === "c-body") focus.setSelectionRange(0, 0);
}

async function sendCompose() {
  const sheet = $(".sheet");
  const btn = $('[data-action="send"]');
  const draft = {
    accountId: $("#c-from")?.value || sheet.dataset.from,
    to: $("#c-to").value,
    cc: $("#c-cc").value,
    bcc: $("#c-bcc").value,
    subject: $("#c-subject").value,
    body: $("#c-body").value,
    inReplyTo: sheet.dataset.inReplyTo || null,
    references: JSON.parse(sheet.dataset.references || "[]"),
  };
  if (!draft.to.trim() && !draft.cc.trim() && !draft.bcc.trim()) return fail("请填写收件人");
  if (!draft.subject.trim() && !confirm("这封邮件没有主题，仍然发送？")) return;
  btn.disabled = true;
  btn.innerHTML = `${icon("sent")}发送中…`;
  try {
    await invoke("send_mail", { draft });
    closeOverlay();
    toast("已发送");
    if (draft.inReplyTo) {
      const m = findMsg(S.sel);
      if (m) patchMsg(m.accountId, m.folder, m.uid, { answered: true });
    }
  } catch (e) {
    btn.disabled = false;
    btn.innerHTML = `${icon("sent")}发送`;
    fail(e);
  }
}

let formKind = "qq";
function accountForm(err = "") {
  const k = KINDS.find((x) => x.id === formKind);
  const custom = formKind === "custom";
  return `<div class="kinds">${KINDS.map((x) => `<button class="kind ${x.id === formKind ? "on" : ""}" data-kind="${x.id}">${esc(x.label)}</button>`).join("")}</div>
    <form class="form-grid" id="acct-form" autocomplete="off">
      <label>邮箱地址</label><input name="email" type="email" placeholder="${formKind === "gmail" ? "you@gmail.com" : formKind === "exmail" ? "you@company.com" : "you@" + (formKind === "custom" ? "example.com" : formKind + ".com")}" required spellcheck="false">
      <label>授权码</label><input name="password" type="password" placeholder="不是网页登录密码" required>
      <div class="form-hint">${esc(k.hint)}</div>
      <label>显示名称</label><input name="name" placeholder="可选，发信时对方看到的名字">
      ${custom ? `<label>IMAP</label><div class="pair"><input name="imapHost" placeholder="imap.example.com" spellcheck="false"><input name="imapPort" value="993" inputmode="numeric"></div>
      <label>SMTP</label><div class="pair"><input name="smtpHost" placeholder="smtp.example.com" spellcheck="false"><input name="smtpPort" value="465" inputmode="numeric"></div>` : ""}
      ${err ? `<div class="form-err">${esc(err)}</div>` : ""}
      <div class="form-actions"><button class="primary" type="submit">登录并添加</button></div>
    </form>`;
}

async function submitAccount(form) {
  const f = new FormData(form);
  const input = { kind: formKind, email: f.get("email"), password: f.get("password"), name: f.get("name") || "" };
  if (formKind === "custom") {
    Object.assign(input, {
      imapHost: f.get("imapHost"), imapPort: +f.get("imapPort") || 993,
      smtpHost: f.get("smtpHost"), smtpPort: +f.get("smtpPort") || 465,
    });
  }
  const btn = $("button[type=submit]", form);
  btn.disabled = true;
  btn.textContent = "正在登录…";
  try {
    const a = await invoke("add_account", { input });
    S.accounts.push(a);
    S.live[a.id] = { state: "connecting", detail: "", unread: 0 };
    closeOverlay();
    go("INBOX", a.id);
    toast(`已添加 ${a.email}`);
  } catch (e) {
    const box = form.closest(".settings-body, .welcome-card");
    const keep = { email: input.email, name: input.name };
    $("#acct-form-wrap", box).innerHTML = accountForm(String(e));
    const nf = $("#acct-form", box);
    nf.email.value = keep.email;
    nf.name.value = keep.name;
    nf.password.focus();
  }
}

function openSettings(addOnly = false) {
  const rows = S.accounts.map((a) => {
    const l = S.live[a.id] || {};
    const st = { online: "已连接 · " + (l.detail || ""), connecting: "正在连接…", offline: "连接中断，正在重试：" + (l.detail || ""), error: l.detail }[l.state] || "";
    return `<div class="acct-row">${avatar(a.name || a.email, a.email)}
      <div class="meta"><div class="n">${esc(a.email)}</div><div class="s ${l.state === "error" ? "err" : ""}">${esc(a.name)} · ${esc(st)}</div></div>
      <button class="danger-link" data-remove="${esc(a.id)}">移除</button></div>`;
  }).join("");
  $("#overlay").innerHTML = `<div class="scrim"><div class="sheet">
    <div class="sheet-head"><h3>${addOnly ? "添加邮箱" : "设置"}</h3><button class="icon-btn" data-action="overlay-close">${icon("x")}</button></div>
    <div class="settings-body">
      ${addOnly ? "" : `<div class="form-title" style="margin-top:8px">邮箱账户</div>${rows}<div class="form-title">添加邮箱</div>`}
      <div id="acct-form-wrap">${accountForm()}</div>
    </div></div></div>`;
}

function renderWelcome() {
  $("#app").style.gridTemplateColumns = "1fr";
  $("#sidebar").hidden = $("#list").hidden = true;
  if ($(".welcome")) return;
  $("#reader").innerHTML = `<div class="toolbar" data-tauri-drag-region style="border:0;background:none"></div>
    <div class="welcome"><div class="welcome-card">
      <img src="icon.png" width="64" height="64" alt="">
      <h2>欢迎使用 MailPulse</h2>
      <p>新邮件到达服务器后立即推送到这里。先添加一个邮箱吧。</p>
      <div id="acct-form-wrap">${accountForm()}</div>
    </div></div>`;
}

function leaveWelcome() {
  $("#app").style.gridTemplateColumns = "";
  $("#sidebar").hidden = $("#list").hidden = false;
  $("#reader").dataset.sig = "";
}

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------
document.addEventListener("click", async (e) => {
  const t = e.target;
  if (t.closest("[data-close]")) return closeOverlay();
  if (t.classList.contains("scrim") && !t.querySelector(".sheet .compose-body")) return closeOverlay();

  const kind = t.closest("[data-kind]");
  if (kind) {
    formKind = kind.dataset.kind;
    const wrap = kind.closest("#acct-form-wrap");
    const email = $("#acct-form", wrap)?.email.value;
    wrap.innerHTML = accountForm();
    if (email) $("#acct-form", wrap).email.value = email;
    return;
  }
  const rm = t.closest("[data-remove]");
  if (rm) {
    const a = account(rm.dataset.remove);
    if (!confirm(`移除 ${a.email}？\n只会从 MailPulse 中移除，不影响服务器上的邮件。`)) return;
    try {
      await invoke("remove_account", { accountId: a.id });
      S.accounts = S.accounts.filter((x) => x.id !== a.id);
      for (const k of [...S.lists.keys()]) if (k.startsWith(a.id + "|")) S.lists.delete(k);
      delete S.live[a.id];
      if (S.acct === a.id || S.accounts.length < 2) S.acct = S.accounts.length === 1 ? S.accounts[0].id : "all";
      if (S.sel?.accountId === a.id) S.sel = null;
      S.folder = "INBOX";
      updateBadge();
      if (S.accounts.length) openSettings(); else closeOverlay();
      render();
    } catch (err) {
      fail(err);
    }
    return;
  }
  const openRow = t.closest("[data-open]");
  if (openRow) {
    const [accountId, uid, ...rest] = openRow.dataset.open.split("|");
    return openMessage({ accountId, uid: +uid, folder: rest.join("|") });
  }
  const goBtn = t.closest("[data-go]");
  if (goBtn) return go(goBtn.dataset.go);
  const sw = t.closest("[data-switch]");
  if (sw) return go("INBOX", sw.dataset.switch);
  const att = t.closest("[data-att]");
  if (att) {
    try {
      const path = await invoke("save_attachment", { accountId: S.sel.accountId, folder: S.sel.folder, uid: S.sel.uid, index: +att.dataset.att });
      toast(`已保存到 ${path.split("/").slice(-2).join("/")}`);
    } catch (err) {
      fail(err);
    }
    return;
  }
  const link = t.closest("[data-link]");
  if (link) {
    e.preventDefault();
    return invoke("open_link", { url: link.dataset.link }).catch(fail);
  }

  const btn = t.closest("[data-action]");
  if (!btn || btn.disabled) return;
  const m = findMsg(S.sel);
  switch (btn.dataset.action) {
    case "account-menu": return accountMenu(btn);
    case "compose": return openCompose();
    case "compose-close":
      if (($("#c-body").value.trim() || $("#c-to").value.trim()) && !confirm("放弃这封邮件？")) return;
      return closeOverlay();
    case "overlay-close": return closeOverlay();
    case "show-cc":
      document.querySelectorAll(".cc-row").forEach((x) => (x.hidden = false));
      btn.remove();
      return $("#c-cc").focus();
    case "send": return sendCompose();
    case "settings": return openSettings();
    case "search":
      S.searching = true;
      renderList();
      return $("#q").focus();
    case "search-close":
      S.searching = false;
      S.query = "";
      return render();
    case "unread-only":
      S.unreadOnly = !S.unreadOnly;
      return render();
    case "list-menu":
      return showMenu(btn, [
        { label: "全部标为已读", icon: "checkAll", run: markAllRead },
        { label: "刷新", icon: "refresh", run: () => (S.folder === "INBOX" ? toast("收件箱是实时推送的，无需刷新") : go(S.folder)) },
      ]);
    case "more": return loadList(S.acct, S.folder, { more: true });
    case "close": S.sel = null; S.msg = null; return render();
    case "prev": return step(-1);
    case "next": return step(1);
    case "retry": return openMessage(S.sel);
    case "star": return m && setFlag(m, "flagged", !m.flagged);
    case "delete": return moveCurrent("trash");
    case "archive": return moveCurrent("archive");
    case "junk": {
      const role = folderInfo(m.accountId, m.folder)?.role;
      return moveCurrent(role === "junk" ? "inbox" : "junk", role === "junk" ? "INBOX" : null);
    }
    case "move-menu": {
      const folders = (S.folders[m.accountId] || []).filter((f) => f.path !== m.folder);
      if (!folders.length) return toast("正在读取文件夹，请稍后再试");
      return showMenu(btn, folders.map((f) => ({ label: f.name, icon: ROLE_ICON[f.role], run: () => moveCurrent("other", f.path) })));
    }
    case "msg-menu":
      return showMenu(btn, [
        { label: m.seen ? "标为未读" : "标为已读", icon: "unread", sub: "U", run: () => setFlag(m, "seen", !m.seen) },
        { label: m.flagged ? "取消星标" : "加星标", icon: "star", sub: "S", run: () => setFlag(m, "flagged", !m.flagged) },
      ]);
    case "reply": case "reply-all": case "forward": {
      const d = draftFor(btn.dataset.action);
      return d ? openCompose(d) : toast("邮件还在加载");
    }
    case "show-images":
    case "always-images": {
      const v = S.msg;
      S.showImages.add(btn.dataset.action === "always-images" ? "from:" + v.from.addr.toLowerCase() : key(key(m.accountId, m.folder), m.uid));
      if (btn.dataset.action === "always-images") saveImagePrefs();
      return renderReader();
    }
  }
});

document.addEventListener("submit", (e) => {
  if (e.target.id === "acct-form") {
    e.preventDefault();
    submitAccount(e.target);
  }
});

document.addEventListener("input", (e) => {
  if (e.target.id === "q") {
    S.query = e.target.value;
    renderList();
  }
});

function onKey(e) {
  const typing = e.target.closest?.("input, textarea, select");
  const overlay = $("#overlay").innerHTML !== "";
  if (e.metaKey && e.key === "Enter" && $("#c-body")) {
    e.preventDefault();
    return sendCompose();
  }
  if (e.key === "Escape") {
    if (overlay) return $("#c-body") ? $('[data-action="compose-close"]').click() : closeOverlay();
    if (S.searching) { S.searching = false; S.query = ""; return render(); }
    if (S.sel) { S.sel = null; S.msg = null; return render(); }
  }
  if (e.metaKey && e.key.toLowerCase() === "n") { e.preventDefault(); return openCompose(); }
  if (e.metaKey && e.key.toLowerCase() === "f") { e.preventDefault(); S.searching = true; renderList(); return $("#q").focus(); }
  if (typing || overlay || e.metaKey || e.ctrlKey || e.altKey || !S.accounts.length) return;
  const m = findMsg(S.sel);
  const click = (a) => $(`[data-action="${a}"]`)?.click();
  switch (e.key) {
    case "ArrowDown": case "j": e.preventDefault(); return step(1);
    case "ArrowUp": case "k": e.preventDefault(); return step(-1);
    case "/": e.preventDefault(); return click("search");
    case "r": return m && click("reply");
    case "a": return m && click("reply-all");
    case "f": return m && click("forward");
    case "e": return m && click("archive");
    case "s": return m && setFlag(m, "flagged", !m.flagged);
    case "u": return m && setFlag(m, "seen", !m.seen);
    case "Backspace": case "Delete": case "#": return m && click("delete");
  }
}
document.addEventListener("keydown", onKey);

function saveImagePrefs() {
  try {
    localStorage.setItem("mp.images", JSON.stringify([...S.showImages].filter((x) => x.startsWith("from:"))));
  } catch {}
}
function loadImagePrefs() {
  try {
    for (const x of JSON.parse(localStorage.getItem("mp.images") || "[]")) S.showImages.add(x);
  } catch {}
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------
(async function boot() {
  loadImagePrefs();
  await listen("mail", (e) => onMailEvent(e.payload));
  const snap = await invoke("snapshot");
  S.accounts = snap.accounts;
  for (const [id, l] of Object.entries(snap.live)) {
    S.live[id] = { state: l.state || "connecting", detail: l.detail, unread: l.unread };
    if (l.inbox?.length) {
      S.lists.set(key(id, "INBOX"), l.inbox);
      loadPreviews(id, "INBOX", l.inbox);
    }
  }
  if (S.accounts.length === 1) S.acct = S.accounts[0].id;
  for (const a of S.accounts) loadFolders(a.id);
  updateBadge();
  render();
})();

// 第一次添加账号后从欢迎页切到三栏
const _render = render;
render = function () {
  if (S.accounts.length && $("#sidebar").hidden) leaveWelcome();
  _render();
};
