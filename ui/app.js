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
  download: '<path d="M12 3v12m-5-5 5 5 5-5M4 16v4h16v-4"/>',
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
  autoDownloads: {},   // accountId → 自动下载的运行状态，与表单草稿分开
  bulkDownloads: {},   // accountId → 手动全量下载的运行状态
  bulkPending: new Map(),
  bulkEventSeq: new Map(),
  bulkErrors: new Map(),
  bulkDirectories: new Map(),
  bulkInheritFilters: new Map(), // accountId → 本次会话的全量下载过滤选择
  settingsExpanded: new Map(), // 折叠只隐藏现有表单，保留未保存输入
  folders: {},         // accountId → [{ path, name, role }]
  folderSeq: new Map(),
  lists: new Map(),    // "acct|folder" → [summary]（folder 也可能是 "__starred"）
  loading: new Set(),  // 正在加载的 "acct|folder"
  exhausted: new Set(),// 已经翻到底的 "acct|folder"
  listSeq: new Map(),  // 忽略已经被后续加载替代的列表请求
  moving: new Set(),
  downloading: new Set(),
  removed: new Set(),  // 已经被服务器确认移走的 UID，拦截旧推送快照
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
const messageKey = (m) => key(key(m.accountId, m.folder), m.uid);
const keepPresent = (messages) => messages.filter((m) => !S.removed.has(messageKey(m)));
const selectionOf = (m) => ({ accountId: m.accountId, folder: m.folder, uid: m.uid });
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

async function loadFolders(accountId, { report = false } = {}) {
  const seq = (S.folderSeq.get(accountId) || 0) + 1;
  S.folderSeq.set(accountId, seq);
  try {
    const folders = await invoke("list_folders", { accountId });
    if (S.folderSeq.get(accountId) === seq) {
      S.folders[accountId] = folders;
      render();
    }
    return folders;
  } catch (e) {
    if (report) throw e;
    console.warn("读取文件夹失败", e);
  }
}

async function loadList(accountId, folder, { more = false, force = false } = {}) {
  const k = key(accountId, folder);
  if (S.loading.has(k) && !force) return;
  const seq = (S.listSeq.get(k) || 0) + 1;
  S.listSeq.set(k, seq);
  S.loading.add(k);
  render();
  try {
    const existing = S.lists.get(k) || [];
    const before = more && existing.length ? Math.min(...existing.map((m) => m.uid)) : null;
    const starred = folder === "__starred";
    const response = await invoke("list_messages", {
      accountId,
      folder: starred ? "INBOX" : folder,
      before,
      flaggedOnly: starred,
    });
    if (S.listSeq.get(k) !== seq) return;
    // 这次请求已通过代次检查，是移动完成后重新读取的服务器结果。
    // 若 UID 被服务器重新使用，以服务器为准，不永久隐藏它。
    const got = response;
    for (const m of got) S.removed.delete(messageKey(m));
    if (more) {
      S.lists.set(k, keepPresent(existing).concat(got));
      if (got.length === 0) S.exhausted.add(k);
    } else {
      S.lists.set(k, got);
      S.exhausted.delete(k);
    }
    loadPreviews(accountId, starred ? "INBOX" : folder, got);
  } catch (e) {
    fail(e);
  } finally {
    if (S.listSeq.get(k) === seq) S.loading.delete(k);
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
  if (ev.type === "bulkDownload") {
    S.bulkEventSeq.set(a, (S.bulkEventSeq.get(a) || 0) + 1);
    S.bulkDownloads[a] = ev;
    renderBulkDownloadStatus(a);
    return;
  }
  if (ev.type === "autoDownload") {
    S.autoDownloads[a] = { state: ev.state, detail: ev.detail, lastPath: ev.lastPath, savedCount: ev.savedCount };
    renderAutoDownloadStatus(a);
    return;
  }
  // 刚添加的账号：同步任务可能比 add_account 先返回结果，事件照样记下，不能丢
  const live = (S.live[a] ||= { state: "connecting", detail: "", unread: 0 });
  switch (ev.type) {
    case "status":
      if (ev.state === "connecting" || (ev.state === "online" && live.state !== "online")) {
        for (const id of S.removed) if (id.startsWith(a + "|")) S.removed.delete(id);
      }
      live.state = ev.state;
      live.detail = ev.detail;
      if (ev.state === "online" && !S.folders[a]) loadFolders(a);
      break;
    case "inbox":
      S.lists.set(key(a, "INBOX"), keepPresent(ev.messages));
      loadPreviews(a, "INBOX", keepPresent(ev.messages));
      break;
    case "newMail": {
      const list = S.lists.get(key(a, "INBOX")) || [];
      const fresh = keepPresent(ev.messages).filter((m) => !list.some((x) => x.uid === m.uid));
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
    more = others.map((f) => navItem(f.name, "folder", S.folder === f.path, `data-go="${esc(f.path)}"`)).join("");
  }
  const folderHeading = `<div class="side-label folder-label"><span>文件夹</span><button class="icon-btn" data-action="create-folder" title="新建文件夹" aria-label="新建文件夹">${icon("plus")}</button></div>`;
  more = all ? more + folderHeading : folderHeading + more;

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
    r.dataset.sig = "";
    r.innerHTML = `<div class="toolbar" data-tauri-drag-region></div>
      <div class="reader-empty"><div>${icon("mail", "big")}<div>没有选中邮件</div></div></div>`;
    return;
  }
  const items = currentList();
  const idx = items.findIndex(isSel);
  const role = folderInfo(m.accountId, m.folder)?.role || (m.folder === "INBOX" ? "inbox" : "other");
  const hasArchive = folderByRole(m.accountId, "archive") || folderByRole(m.accountId, "all");
  const v = S.msg;
  const moving = S.moving.has(messageKey(m));

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
      <button class="tb-btn" data-action="junk" ${moving ? "disabled" : ""} title="${role === "junk" ? "不是垃圾邮件" : "标为垃圾邮件"}">${icon("junk")}</button>
      <button class="tb-btn" data-action="move-menu" ${moving ? "disabled" : ""} title="移动到…">${icon("folder")}</button>
      <button class="tb-btn" data-action="archive" title="归档 (E)" ${!moving && hasArchive && role !== "archive" && role !== "all" ? "" : "disabled"}>${icon("archive")}</button>
      <button class="tb-btn" data-action="delete" ${moving ? "disabled" : ""} title="${role === "trash" ? "彻底删除" : "删除 (⌫)"}">${icon("trash")}</button>
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
      ? `<div class="atts">${v.attachments.map((x) => `<button class="att" data-att="${x.index}" data-attachment-source="${esc(JSON.stringify({ ...selectionOf(m), uidValidity: v.uidValidity, index: x.index }))}" title="保存到「下载」">${icon("file")}<span class="n">${esc(x.name)}</span><span class="s">${fmtSize(x.size)}</span></button>`).join("")}</div>` : "";
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
  const sig = `${key(key(m.accountId, m.folder), m.uid)}|${v ? "v" : ""}|${S.msgError}|${S.showImages.size}|${m.flagged}|${idx}|${items.length}|${role}|${moving}`;
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

function fitMailFrame(frame, doc) {
  // A mail using vh/% plus padding can otherwise grow its iframe on every
  // ResizeObserver notification. Long mail stays available via iframe scrolling.
  const height = Math.min(8000, Math.max(120, doc.documentElement.scrollHeight));
  const value = `${height}px`;
  if (frame.style.height !== value) frame.style.height = value;
}

function mountFrame(v, m) {
  const frame = $("#mail-frame");
  const imgKey = key(key(m.accountId, m.folder), m.uid);
  const remote = S.showImages.has(imgKey) || S.showImages.has("from:" + v.from.addr.toLowerCase());
  // 默认拦截远程图片（防止追踪像素），点「显示图片」后放开
  const csp = `default-src 'none'; style-src 'unsafe-inline' data:${remote ? " https: http:" : ""}; font-src data:${remote ? " https: http:" : ""}; img-src data: cid:${remote ? " https: http:" : ""}`;
  // 保留邮件自己的表格宽度；break-word 不会像 anywhere 一样压缩列的最小内容宽度。
  frame.srcdoc = `<!doctype html><html><head><meta charset="utf-8">
    <meta http-equiv="Content-Security-Policy" content="${csp}">
    <base target="_blank">
    <style>html,body{margin:0;background:#fff;color:#1d1d1f}body{padding:22px 26px;font:14px/1.6 -apple-system,"PingFang SC",sans-serif;overflow-wrap:break-word}
    img{max-width:100%;height:auto}pre{white-space:pre-wrap}a{color:#2f6fec}</style></head><body>${v.html}</body></html>`;
  frame.onload = () => {
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => fitMailFrame(frame, doc);
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

/** 服务器确认后移除源邮件；菜单操作使用点击时捕获的邮件。 */
async function moveCurrent(role, target, source = S.sel) {
  if (!source) return false;
  const sel = selectionOf(source);
  const id = messageKey(sel);
  if (!findMsg(sel) || S.moving.has(id) || target === sel.folder) return false;
  const destination = target || (role === "inbox" ? "INBOX" :
    (folderByRole(sel.accountId, role) || (role === "archive" && folderByRole(sel.accountId, "all")))?.path);
  S.moving.add(id);
  renderReader();
  toast("正在移动邮件…");
  try {
    await invoke("move_messages", { accountId: sel.accountId, folder: sel.folder, uids: [sel.uid], role, target: target || null });
    const stillSelected = isSel(sel);
    const items = currentList();
    const i = items.findIndex((m) => messageKey(m) === id);
    const next = i >= 0 ? items[i + 1] || items[i - 1] : null;
    S.removed.add(id);
    dropMsg(sel.accountId, sel.folder, sel.uid);
    // 移动后的 UID 由目标邮箱重新分配，必须重新读取目标列表。
    for (const folder of [destination, "__starred"].filter(Boolean)) {
      if (folder === sel.folder) continue;
      const k = key(sel.accountId, folder);
      S.lists.delete(k);
      S.loading.delete(k);
      S.exhausted.delete(k);
      S.listSeq.set(k, (S.listSeq.get(k) || 0) + 1);
    }
    if (stillSelected) {
      if (next) openMessage(selectionOf(next));
      else { ++openSeq; S.sel = null; S.msg = null; }
    }
    loadList(sel.accountId, sel.folder, { force: true });
    if (S.folder !== sel.folder && (S.acct === sel.accountId || S.acct === "all")) {
      loadList(sel.accountId, S.folder, { force: true });
    }
    const words = { trash: "已删除", archive: "已归档", junk: "已移到垃圾邮件", inbox: "已移回收件箱" };
    toast(target ? `已移到「${folderInfo(sel.accountId, target)?.name || target}」` : words[role] || "已移动");
    return true;
  } catch (e) {
    fail(e);
    // 请求失败也可能已有部分服务器操作完成；重新读取，不自动重试移动。
    loadList(sel.accountId, sel.folder, { force: true });
    if (destination && destination !== sel.folder) loadList(sel.accountId, destination, { force: true });
    if (S.folder === "__starred") loadList(sel.accountId, "__starred", { force: true });
    return false;
  } finally {
    S.moving.delete(id);
    render();
  }
}

async function downloadMessage(source = S.sel) {
  if (!source) return;
  const sel = selectionOf(source);
  const id = messageKey(sel);
  if (S.downloading.has(id)) return;
  S.downloading.add(id);
  toast("正在下载邮件…");
  try {
    const path = await invoke("save_message", sel);
    toast(`邮件已保存到 ${path.split("/").slice(-2).join("/")}`);
  } catch (e) {
    fail(e);
  } finally {
    S.downloading.delete(id);
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
  ++openSeq;
  S.acct = acct;
  S.folder = folder;
  S.sel = null;
  S.msg = null;
  S.unreadOnly = false;
  const targets = acct === "all" ? S.accounts.map((a) => a.id) : [acct];
  for (const id of targets) {
    if (folder !== "INBOX" || !S.lists.has(key(id, folder))) loadList(id, folder);
    if (!S.folders[id]) loadFolders(id);
  }
  render();
}

// ---------------------------------------------------------------------------
// 弹层：菜单、写信、设置
// ---------------------------------------------------------------------------
let menuReturnFocus = null;
function closeOverlay() {
  if ($('.sheet[data-busy="true"]')) return;
  $(".compose-sheet")?._quoteObserver?.disconnect();
  $("#overlay").innerHTML = "";
  const focus = menuReturnFocus;
  menuReturnFocus = null;
  if (focus?.isConnected) focus.focus();
}

function showMenu(anchor, entries, point = null) {
  const r = anchor.getBoundingClientRect();
  menuReturnFocus = anchor;
  const html = entries.map((e, i) => e === "-" ? '<div class="sep" role="separator"></div>'
    : `<button role="menuitem" tabindex="-1" data-mi="${i}" ${e.disabled ? "disabled" : ""}>${e.check !== undefined ? `<span class="check">${e.check ? icon("check") : ""}</span>` : e.icon ? icon(e.icon) : ""}<span>${esc(e.label)}</span>${e.sub ? `<span class="sub">${esc(e.sub)}</span>` : ""}</button>`).join("");
  $("#overlay").innerHTML = `<div class="scrim" style="background:none" data-close></div><div class="menu" role="menu" aria-label="邮件操作">${html}</div>`;
  const menu = $(".menu");
  const w = menu.offsetWidth, h = menu.offsetHeight;
  const left = point ? point.x : r.left;
  const top = point ? point.y : (r.bottom + h + 6 > innerHeight ? r.top - h - 6 : r.bottom + 6);
  menu.style.left = Math.max(8, Math.min(left, innerWidth - w - 8)) + "px";
  menu.style.top = Math.max(8, Math.min(top, innerHeight - h - 8)) + "px";
  const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
  buttons[0]?.focus();
  menu.onkeydown = (ev) => {
    if (ev.key === "Tab") { ev.preventDefault(); closeOverlay(); return; }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(ev.key) || !buttons.length) return;
    ev.preventDefault();
    ev.stopPropagation();
    const i = buttons.indexOf(document.activeElement);
    const next = ev.key === "Home" ? 0 : ev.key === "End" ? buttons.length - 1
      : (i + (ev.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
    buttons[next].focus();
  };
  menu.onclick = (ev) => {
    const b = ev.target.closest("[data-mi]");
    if (!b || b.disabled) return;
    closeOverlay();
    entries[+b.dataset.mi].run();
  };
}

function showMoveMenu(anchor, source, point = null) {
  if (!source || !findMsg(source)) return;
  const sel = selectionOf(source);
  const moving = S.moving.has(messageKey(sel));
  const folders = (S.folders[sel.accountId] || []).filter((f) => f.path !== sel.folder);
  showMenu(anchor, [
    ...folders.map((f) => ({ label: f.name, icon: ROLE_ICON[f.role] || "folder", disabled: moving, run: () => moveCurrent("other", f.path, sel) })),
    ...(folders.length ? ["-"] : []),
    { label: "新建文件夹并移动…", icon: "plus", disabled: moving, run: () => openCreateFolder(sel) },
  ], point);
}

function showMessageMenu(anchor, source, point = null) {
  const m = findMsg(source);
  if (!m) return;
  const sel = selectionOf(m);
  const seen = m.seen, flagged = m.flagged;
  const moving = S.moving.has(messageKey(sel));
  const downloading = S.downloading.has(messageKey(sel));
  showMenu(anchor, [
    { label: seen ? "标为未读" : "标为已读", icon: "unread", sub: "U", disabled: moving, run: () => setFlag(sel, "seen", !seen) },
    { label: flagged ? "取消星标" : "加星标", icon: "star", sub: "S", disabled: moving, run: () => setFlag(sel, "flagged", !flagged) },
    "-",
    { label: downloading ? "正在下载邮件…" : "下载邮件（.eml）", icon: "download", disabled: downloading || moving, run: () => downloadMessage(sel) },
    { label: "移动到…", icon: "folder", disabled: moving, run: () => showMoveMenu(anchor, sel, point) },
  ], point);
}

function rowSelection(row) {
  const [accountId, uid, ...rest] = row.dataset.open.split("|");
  return { accountId, uid: +uid, folder: rest.join("|") };
}

function accountMenu(anchor) {
  const entries = [];
  if (S.accounts.length > 1) entries.push({ label: "所有账户", check: S.acct === "all", run: () => go("INBOX", "all") });
  for (const a of S.accounts) entries.push({ label: a.email, check: S.acct === a.id, run: () => go("INBOX", a.id) });
  entries.push("-", { label: "添加邮箱…", icon: "plus", run: () => openSettings(true) });
  showMenu(anchor, entries);
}

function openCreateFolder(source = null) {
  const moving = source ? selectionOf(source) : null;
  const selectedAccount = moving?.accountId || (S.acct !== "all" ? S.acct : S.accounts[0]?.id);
  if (!selectedAccount) return;
  $("#overlay").innerHTML = `<div class="scrim"><div class="sheet folder-sheet" role="dialog" aria-modal="true" aria-labelledby="folder-title">
    <div class="sheet-head"><h3 id="folder-title">${moving ? "新建文件夹并移动邮件" : "新建文件夹"}</h3><button class="icon-btn" data-action="overlay-close" aria-label="关闭">${icon("x")}</button></div>
    <form id="folder-form" class="folder-form">
      <label for="folder-account">邮箱</label>
      <select id="folder-account" ${moving || S.accounts.length === 1 ? "disabled" : ""}>${S.accounts.map((a) => `<option value="${esc(a.id)}" ${a.id === selectedAccount ? "selected" : ""}>${esc(a.email)}</option>`).join("")}</select>
      <label for="folder-name">文件夹名称</label><input id="folder-name" placeholder="例如：工作资料" maxlength="255" required autocomplete="off">
      ${moving ? `<p class="hint">将移动「${esc(findMsg(moving)?.subject || "当前邮件")}」</p>` : ""}
      <p class="folder-error" role="alert" hidden></p>
      <div class="folder-actions"><button class="secondary" type="button" data-action="overlay-close">取消</button><button class="primary" type="submit">${moving ? "创建并移动" : "创建"}</button></div>
    </form>
  </div></div>`;
  $("#folder-form")._source = moving;
  $("#folder-name").focus();
}

async function submitFolder(form) {
  if (form.dataset.busy === "true" || form.dataset.created === "true") return;
  const accountId = $("#folder-account", form).value;
  const name = $("#folder-name", form).value.trim();
  const error = $(".folder-error", form);
  if (!name) { error.hidden = false; error.textContent = "请填写文件夹名称"; return; }
  const button = $('button[type="submit"]', form);
  const sheet = form.closest(".sheet");
  form.dataset.busy = sheet.dataset.busy = "true";
  button.disabled = true;
  button.textContent = "正在创建…";
  error.hidden = true;
  try {
    const folder = await invoke("create_folder", { accountId, name });
    form.dataset.created = "true";
    // 创建已完成，即便后续刷新/移动失败也保留真实存在的文件夹。
    const folders = S.folders[accountId] ||= [];
    if (!folders.some((f) => f.path === folder.path)) folders.push(folder);
    renderSidebar();
    try {
      await loadFolders(accountId, { report: true });
    } catch (e) {
      error.hidden = false;
      error.textContent = `文件夹「${folder.name}」已创建，但列表刷新失败：${String(e)}。请关闭此窗口后刷新。`;
      button.hidden = true;
      return;
    }
    if (form._source) {
      button.textContent = "正在移动…";
      const moved = await moveCurrent("other", folder.path, form._source);
      if (!moved) {
        error.hidden = false;
        error.textContent = `文件夹「${folder.name}」已创建。邮件移动未确认完成，请关闭此窗口并检查邮件所在位置。`;
        button.hidden = true;
        return;
      }
    } else {
      go(folder.path, accountId);
      toast(`已创建「${folder.name}」`);
    }
    sheet.dataset.busy = "false";
    if (form.isConnected) closeOverlay();
  } catch (e) {
    error.hidden = false;
    error.textContent = String(e);
  } finally {
    form.dataset.busy = sheet.dataset.busy = "false";
    button.disabled = false;
    button.textContent = form._source ? "创建并移动" : "创建";
  }
}

function quoteHeader(v) {
  const who = v.from.name ? `${v.from.name} <${v.from.addr}>` : v.from.addr;
  return `在 ${fmtDate(v.date, true)}，${who} 写道：`;
}

function draftFor(mode) {
  const m = findMsg(S.sel);
  const v = S.msg;
  if (!m || !v) return null;
  const me = account(m.accountId).email.toLowerCase();
  const fmt = (p) => (p.name ? `${p.name} <${p.addr}>` : p.addr);
  const strip = (s) => s.replace(/^((re|fw|fwd|回复|答复|转发)\s*[:：]\s*)+/i, "");
  const original = {
    quote: { html: v.replyHtml || v.html, text: v.text || "", header: quoteHeader(v) },
    quotePreviewHtml: v.html,
    quoteSource: { ...selectionOf(m), uidValidity: v.uidValidity },
    quoteHasRemote: v.hasRemote,
    quoteRemoteImages: S.showImages.has(key(key(m.accountId, m.folder), m.uid)) || S.showImages.has("from:" + v.from.addr.toLowerCase()),
  };
  if (mode === "forward") {
    original.quote.header = `---------- 转发的邮件 ----------\n发件人：${fmt(v.from)}\n日期：${fmtDate(v.date, true)}\n主题：${v.subject}\n收件人：${v.to.map(fmt).join(", ")}`;
    return { ...original, accountId: m.accountId, to: "", cc: "", subject: "Fwd: " + strip(v.subject), body: "", note: v.attachments.length ? "转发暂不带原附件" : "" };
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
    ...original,
    accountId: m.accountId,
    to: uniq(to).map(fmt).join(", "),
    cc: uniq(cc).map(fmt).join(", "),
    subject: "Re: " + strip(v.subject),
    body: "",
    inReplyTo: v.messageId,
    references: v.references,
  };
}

function openCompose(d = {}) {
  // A second shortcut must not replace an unfinished (or sending) draft.
  const existing = $("#c-body");
  if (existing) { existing.focus(); return; }
  const from = d.accountId || (S.acct !== "all" ? S.acct : S.accounts[0].id);
  const showCc = !!d.cc;
  const signatureDraft = window.MailPulseSignature.createSignatureDraft(d.body || "", account(from)?.signature || "");
  const preservedQuote = d.quote ? window.MailPulseQuote.createQuote(d.quote) : null;
  const quoteMarkup = preservedQuote ? `<section class="compose-quote" aria-label="原邮件引用">
    <div class="compose-quote-head"><span>原邮件</span>${preservedQuote.rich && d.quoteHasRemote && !d.quoteRemoteImages ? '<button class="link" data-action="quote-images">显示引用中的图片</button>' : ""}</div>
    ${preservedQuote.notice ? `<p class="compose-quote-notice" role="status">${esc(preservedQuote.notice)}</p>` : ""}
    <div class="compose-quote-header">${esc(preservedQuote.header)}</div>
    ${preservedQuote.rich ? '<iframe id="compose-quote-frame" title="保留格式的原邮件" sandbox="allow-same-origin" referrerpolicy="no-referrer"></iframe>' : `<pre class="compose-quote-plain">${esc(preservedQuote.text)}</pre>`}
  </section>` : "";
  $("#overlay").innerHTML = `<div class="scrim"><div class="sheet compose-sheet" role="dialog" aria-modal="true" aria-labelledby="compose-title" style="height:min(640px, calc(100vh - 80px))">
    <div class="sheet-head"><h3 id="compose-title">${esc(d.subject ? d.subject : "新邮件")}</h3><button class="icon-btn" data-action="compose-close" title="关闭" aria-label="关闭写信窗口">${icon("x")}</button></div>
    ${S.accounts.length > 1 ? `<div class="field"><label>发件人</label><select id="c-from">${S.accounts.map((a) => `<option value="${esc(a.id)}" ${a.id === from ? "selected" : ""}>${esc(a.name)} &lt;${esc(a.email)}&gt;</option>`).join("")}</select></div>` : ""}
    <div class="field"><label>收件人</label><input id="c-to" value="${esc(d.to || "")}" spellcheck="false">${showCc ? "" : '<button class="link" data-action="show-cc">抄送 / 密送</button>'}</div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>抄送</label><input id="c-cc" value="${esc(d.cc || "")}" spellcheck="false"></div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>密送</label><input id="c-bcc" spellcheck="false"></div>
    <div class="field"><label>主题</label><input id="c-subject" value="${esc(d.subject || "")}"></div>
    ${preservedQuote ? '<div class="compose-editor">' : ""}
    <textarea class="compose-body" id="c-body" placeholder="${preservedQuote ? "在这里写回复…" : "写点什么…"}" aria-label="邮件正文">${esc(signatureDraft.text)}</textarea>
    ${quoteMarkup}${preservedQuote ? "</div>" : ""}
    <div class="compose-confirm" role="group" aria-label="邮件确认" hidden>
      <p class="compose-confirm-message" role="alert"></p>
      <div class="compose-confirm-actions"><button class="secondary" data-action="compose-continue">继续编辑</button>
        <button class="danger-link" data-action="compose-discard" hidden>放弃修改</button>
        <button class="primary" data-action="compose-send-confirm" hidden>仍然发送</button></div>
    </div>
    <div class="sheet-foot"><span class="hint">${esc(d.note || "多个收件人用逗号分隔 · ⌘↩ 发送")}</span>
      <button class="secondary" data-action="compose-close">取消</button>
      <button class="primary" data-action="send">${icon("sent")}发送</button></div>
  </div></div>`;
  const sheet = $(".sheet");
  sheet.dataset.from = from;
  sheet.dataset.inReplyTo = d.inReplyTo || "";
  sheet.dataset.references = JSON.stringify(d.references || []);
  sheet._signatureDraft = signatureDraft;
  sheet._quote = preservedQuote;
  sheet._quoteSource = d.quoteSource ? { ...d.quoteSource } : null;
  sheet._quoteRemoteImages = !!d.quoteRemoteImages;
  if (preservedQuote?.rich) {
    sheet._quotePreview = window.MailPulseQuote.createQuote({ ...d.quote, html: d.quotePreviewHtml || d.quote.html });
    mountComposeQuote(sheet);
  }
  // Assign after HTML parsing: textarea markup strips its first leading newline.
  $("#c-body", sheet).value = signatureDraft.text;
  sheet._initialDraft = composeValues(sheet);
  const focus = d.to ? $("#c-body") : $("#c-to");
  focus.focus();
  if (focus.id === "c-body") focus.setSelectionRange(0, 0);
}

function mountComposeQuote(sheet) {
  const frame = $("#compose-quote-frame", sheet);
  if (!frame) return;
  sheet._quoteObserver?.disconnect();
  frame.onload = () => {
    if (!frame.isConnected) return;
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => fitMailFrame(frame, doc);
    fit();
    sheet._quoteObserver = new ResizeObserver(fit);
    sheet._quoteObserver.observe(doc.documentElement);
    doc.addEventListener("click", (e) => {
      const link = e.target.closest?.("a[href]");
      if (!link) return;
      e.preventDefault();
      invoke("open_link", { url: link.href }).catch(fail);
    });
    doc.addEventListener("keydown", onKey);
  };
  frame.srcdoc = window.MailPulseQuote.previewDocument(sheet._quotePreview, { allowRemoteImages: sheet._quoteRemoteImages });
}

function composeValues(sheet) {
  return {
    accountId: $("#c-from", sheet)?.value || sheet.dataset.from,
    to: $("#c-to", sheet).value,
    cc: $("#c-cc", sheet).value,
    bcc: $("#c-bcc", sheet).value,
    subject: $("#c-subject", sheet).value,
    body: $("#c-body", sheet).value,
  };
}

function showComposeConfirmation(sheet, kind) {
  const box = $(".compose-confirm", sheet);
  if (box.hidden) {
    const active = document.activeElement;
    const input = active?.closest(".sheet") === sheet && active.matches("input,textarea,select") ? active : $("#c-body", sheet);
    sheet._confirmFocus = { input, start: input.selectionStart, end: input.selectionEnd };
  }
  box.dataset.kind = kind;
  $(".compose-confirm-message", box).textContent = kind === "discard" ? "放弃尚未发送的修改？" : "这封邮件没有主题，仍然发送？";
  $('[data-action="compose-discard"]', box).hidden = kind !== "discard";
  $('[data-action="compose-send-confirm"]', box).hidden = kind !== "send-without-subject";
  box.hidden = false;
  $('[data-action="send"]', sheet).disabled = true;
  $('[data-action="compose-continue"]', box).focus();
}

function hideComposeConfirmation(sheet) {
  $(".compose-confirm", sheet).hidden = true;
  $('[data-action="send"]', sheet).disabled = false;
  const focus = sheet._confirmFocus;
  delete sheet._confirmFocus;
  if (focus?.input.isConnected) {
    focus.input.focus();
    if (focus.input.matches("input,textarea") && typeof focus.start === "number") focus.input.setSelectionRange(focus.start, focus.end);
  }
}

function requestComposeClose() {
  const sheet = $("#c-body")?.closest(".sheet");
  if (!sheet || sheet.dataset.busy === "true") return;
  const current = composeValues(sheet);
  if (Object.keys(current).some((key) => current[key] !== sheet._initialDraft[key])) {
    showComposeConfirmation(sheet, "discard");
  } else closeOverlay();
}

async function sendCompose({ allowEmptySubject = false } = {}) {
  const sheet = $(".sheet");
  const btn = $('[data-action="send"]');
  if (!sheet || !btn || sheet.dataset.busy === "true") return;
  const confirmation = $(".compose-confirm", sheet);
  if (!confirmation.hidden) {
    if (!allowEmptySubject || confirmation.dataset.kind !== "send-without-subject") return;
    hideComposeConfirmation(sheet);
  }
  const draft = {
    ...composeValues(sheet),
    inReplyTo: sheet.dataset.inReplyTo || null,
    references: JSON.parse(sheet.dataset.references || "[]"),
  };
  if (sheet._quote) {
    Object.assign(draft, window.MailPulseQuote.composeBody(draft.body, sheet._quote), {
      quoteSource: sheet._quoteSource,
      quoteCids: sheet._quote.cids || [],
    });
  }
  if (!draft.to.trim() && !draft.cc.trim() && !draft.bcc.trim()) return fail("请填写收件人");
  if (!draft.subject.trim() && !allowEmptySubject) return showComposeConfirmation(sheet, "send-without-subject");
  const source = sheet._quoteSource;
  const controls = [...sheet.querySelectorAll("input,textarea,select,button")].map((control) => ({ control, disabled: control.disabled }));
  sheet.dataset.busy = "true";
  for (const { control } of controls) control.disabled = true;
  const hint = $(".sheet-foot .hint", sheet);
  const previousHint = hint.textContent;
  hint.textContent = "正在发送，请稍候…";
  btn.innerHTML = `${icon("sent")}发送中…`;
  try {
    await invoke("send_mail", { draft });
    sheet.dataset.busy = "false";
    if (sheet.isConnected) closeOverlay();
    toast("已发送");
    if (draft.inReplyTo) {
      const m = findMsg(source);
      if (m) patchMsg(m.accountId, m.folder, m.uid, { answered: true });
    }
  } catch (e) {
    fail(e);
  } finally {
    sheet.dataset.busy = "false";
    for (const { control, disabled } of controls) control.disabled = disabled;
    btn.innerHTML = `${icon("sent")}发送`;
    hint.textContent = previousHint;
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
  const initialAccount = account(S.acct)?.id || S.accounts[0]?.id;
  for (const a of S.accounts) {
    if (!S.settingsExpanded.has(a.id)) S.settingsExpanded.set(a.id, a.id === initialAccount);
  }
  const rows = S.accounts.map((a) => {
    const l = S.live[a.id] || {};
    const st = { online: "已连接 · " + (l.detail || ""), connecting: "正在连接…", offline: "连接中断，正在重试：" + (l.detail || ""), error: l.detail }[l.state] || "";
    const expanded = S.settingsExpanded.get(a.id);
    return `<section class="settings-account" data-settings-account="${esc(a.id)}"><div class="acct-row">
      <button class="settings-account-toggle" type="button" data-action="toggle-settings-account" aria-expanded="${expanded}" aria-controls="settings-account-details-${esc(a.id)}">
        <span class="avatar" style="background:${colorOf(a.email)}">${esc(initial(a.name || a.email))}</span>
        <span class="meta"><span class="n">${esc(a.email)}</span><span class="s ${l.state === "error" ? "err" : ""}">${esc(a.name)} · ${esc(st)}</span><span class="account-bulk-summary" hidden></span></span>
        ${icon("right", "account-disclosure")}
      </button>
      <button class="danger-link" data-remove="${esc(a.id)}">移除</button></div>
      <div class="settings-account-details" id="settings-account-details-${esc(a.id)}" ${expanded ? "" : "hidden"}>
      <form class="signature-form" data-signature-account="${esc(a.id)}">
        <label for="signature-${esc(a.id)}">邮件签名</label>
        <textarea id="signature-${esc(a.id)}" name="signature" rows="3" placeholder="姓名、职务或联系方式">${esc(a.signature || "")}</textarea>
        <div class="signature-actions"><span class="hint">新邮件、回复和转发时自动加入；留空保存可关闭。</span><button class="secondary" type="submit">保存签名</button></div>
        <p class="signature-status" role="status" hidden></p>
      </form>
      ${autoDownloadForm(a)}
      ${bulkDownloadSection(a)}</div></section>`;
  }).join("");
  $("#overlay").innerHTML = `<div class="scrim"><div class="sheet">
    <div class="sheet-head"><h3>${addOnly ? "添加邮箱" : "设置"}</h3><button class="icon-btn" data-action="overlay-close">${icon("x")}</button></div>
    <div class="settings-body">
      ${addOnly ? "" : `<div class="form-title" style="margin-top:8px">邮箱账户</div>${rows}<div class="form-title">添加邮箱</div>`}
      <div id="acct-form-wrap">${accountForm()}</div>
    </div></div></div>`;
  renderAutoDownloadStatus();
  renderBulkDownloadStatus();
  renderSettingsAccounts();
}

function toggleSettingsAccount(section) {
  const id = section?.dataset.settingsAccount;
  if (!id) return;
  S.settingsExpanded.set(id, !S.settingsExpanded.get(id));
  renderSettingsAccounts(id);
}

function renderSettingsAccounts(accountId) {
  const overlay = $("#overlay");
  if (!overlay) return;
  for (const section of overlay.querySelectorAll(".settings-account")) {
    const id = section.dataset.settingsAccount;
    if (!id || (accountId && accountId !== id)) continue;
    const expanded = S.settingsExpanded.get(id) === true;
    $(".settings-account-toggle", section).setAttribute("aria-expanded", String(expanded));
    $(".settings-account-details", section).hidden = !expanded;
    const summary = $(".account-bulk-summary", section), runtime = S.bulkDownloads[id];
    const progress = bulkDownloadProgress(runtime);
    const label = { preparing: "正在准备", scanning: "正在统计", downloading: "正在下载", cancelling: "正在取消", cancelled: "已取消", complete: "下载完成", error: "下载未全部完成" }[runtime?.state];
    summary.hidden = !label;
    summary.textContent = label ? `${label} · ${progress.text}` : "";
    summary.classList.toggle("err", progress.error);
  }
}

function bulkDownloadSection(a) {
  return `<section class="bulk-download-section" data-bulk-download-account="${esc(a.id)}" aria-labelledby="bulk-title-${esc(a.id)}">
    <h4 id="bulk-title-${esc(a.id)}">下载此账号全部邮件</h4>
    <p class="hint">下载所有文件夹中的历史邮件，包括收件箱、草稿、已发送、垃圾邮件、已删除和自建文件夹，保存为完整 .eml（含附件）。</p>
    <label class="bulk-download-toggle"><input class="bulk-download-inherit" type="checkbox" aria-describedby="bulk-filter-hint-${esc(a.id)}">继承该账号的黑白名单规则</label>
    <p class="bulk-download-saved-rule hint"></p>
    <p class="hint" id="bulk-filter-hint-${esc(a.id)}">未勾选时不做过滤；勾选后使用任务开始时已保存的规则，即使自动下载关闭也生效。未启用黑白名单时不做过滤。修改规则请先在上方保存，已开始的任务不受后续修改影响。</p>
    <div class="bulk-download-actions"><button class="secondary" type="button" data-action="start-bulk-download">选择文件夹并开始…</button><button class="secondary" type="button" data-action="cancel-bulk-download" hidden>取消下载</button></div>
    <div class="bulk-download-progress" hidden><p class="bulk-download-progress-label"></p><progress class="bulk-download-progress-bar" max="100" aria-label="邮件处理进度"></progress></div>
    <p class="bulk-download-runtime" role="status" aria-live="polite"></p>
    <p class="bulk-download-task-rule" hidden></p>
    <p class="bulk-download-counts" hidden></p>
    <p class="bulk-download-folder" hidden></p>
    <p class="bulk-download-directory" hidden></p>
    <p class="bulk-download-error err" role="alert" hidden></p>
    <p class="hint">中断后已保存的文件会保留；再次选择同一目录可继续核验并保存，不重复保存已核验的邮件。关闭 MailPulse 会中断本次下载，重新打开后需手动开始。</p>
  </section>`;
}

function bulkDownloadActive(status) {
  return ["preparing", "scanning", "downloading", "cancelling"].includes(status?.state);
}

function bulkDownloadProgress(runtime) {
  const visible = !!runtime && runtime.state !== "idle";
  const error = runtime?.state === "error" || (runtime?.failedCount || 0) > 0;
  const complete = runtime?.state === "complete" && !error;
  if (runtime?.totalCount == null) {
    const scanning = ["preparing", "scanning"].includes(runtime?.state);
    return { visible, error, complete: false, percent: null,
      text: `${scanning ? "正在统计邮件总数" : `已处理 ${runtime?.processedCount || 0} 封 · 总数未能完整统计`} · 已统计文件夹 ${runtime?.foldersScanned || 0} / ${runtime?.folderCount || 0}` };
  }
  const total = Math.max(0, runtime.totalCount), processed = Math.max(0, runtime.processedCount || 0);
  const percent = total === 0 ? (complete ? 100 : 0) : Math.min(100, Math.floor(processed * 100 / total));
  return { visible, error, complete, percent, text: `已处理 ${processed} / 总计 ${total} 封 · ${percent}%` };
}

function bulkDownloadInheritance(accountId) {
  return S.bulkInheritFilters.get(accountId) ?? (S.bulkDownloads[accountId]?.inheritFilters === true);
}

function changeBulkDownloadInheritance(section, enabled) {
  const accountId = section?.dataset.bulkDownloadAccount;
  if (!accountId) return;
  if (!S.bulkPending.has(accountId) && !bulkDownloadActive(S.bulkDownloads[accountId])) {
    S.bulkInheritFilters.set(accountId, enabled === true);
  }
  renderBulkDownloadStatus(accountId);
}

function renderBulkDownloadStatus(accountId) {
  const overlay = $("#overlay");
  if (!overlay) return;
  for (const section of overlay.querySelectorAll(".bulk-download-section")) {
    const id = section.dataset.bulkDownloadAccount;
    if (!id || (accountId && accountId !== id)) continue;
    const runtime = S.bulkDownloads[id], pending = S.bulkPending.get(id);
    const active = bulkDownloadActive(runtime);
    const inherit = $(".bulk-download-inherit", section);
    inherit.checked = active ? runtime.inheritFilters === true : bulkDownloadInheritance(id);
    inherit.disabled = !!pending || active;
    const saved = autoDownloadSettings(account(id));
    const modeLabel = { blacklist: "黑名单：命中则跳过", whitelist: "白名单：命中才下载" };
    const keywords = saved.keywords.slice(0, 3).map(word => word.length > 60 ? word.slice(0, 60) + "…" : word).join("、");
    $(".bulk-download-saved-rule", section).textContent = modeLabel[saved.filterMode]
      ? `该账号已保存${modeLabel[saved.filterMode]}；关键词：${keywords}${saved.keywords.length > 3 ? ` 等 ${saved.keywords.length} 个` : ""}`
      : "该账号已保存的设置未启用黑白名单，继承后仍不做过滤。";
    const taskRule = $(".bulk-download-task-rule", section);
    taskRule.hidden = !runtime || runtime.state === "idle";
    taskRule.textContent = runtime?.inheritFilters
      ? `本次任务：继承${modeLabel[runtime.filterMode] || "规则（未启用黑白名单，不做过滤）"}，使用任务开始时的已保存规则。`
      : "本次任务：未继承黑白名单，不做过滤。";
    const start = $('[data-action="start-bulk-download"]', section);
    const cancel = $('[data-action="cancel-bulk-download"]', section);
    start.disabled = !!pending || active;
    const activeLabel = { preparing: "正在准备…", scanning: "正在统计…", downloading: "正在下载…", cancelling: "正在取消…" }[runtime?.state];
    start.textContent = pending === "choosing" ? "正在选择文件夹…" : pending === "starting" ? "正在开始…" : activeLabel || (runtime && runtime.state !== "idle" ? "再次选择文件夹下载…" : "选择文件夹并开始…");
    cancel.hidden = !active;
    cancel.disabled = !!pending || runtime?.state === "cancelling";
    cancel.textContent = pending === "cancelling" || runtime?.state === "cancelling" ? "正在取消…" : "取消下载";
    const labels = { idle: "尚未开始全量下载", preparing: "正在读取所有文件夹…", scanning: "正在统计邮件总数…", downloading: "正在下载…", cancelling: "正在取消，已保存的邮件会保留…", cancelled: "已取消，已保存的邮件已保留", complete: modeLabel[runtime?.filterMode] ? "符合规则的邮件下载完成" : "全部邮件下载完成", error: "下载未全部完成，可选择同一目录重试" };
    const status = $(".bulk-download-runtime", section);
    status.textContent = runtime?.detail || labels[runtime?.state] || labels.idle;
    status.classList.toggle("err", runtime?.state === "error");
    const progress = bulkDownloadProgress(runtime), progressBox = $(".bulk-download-progress", section), progressBar = $(".bulk-download-progress-bar", section);
    progressBox.hidden = !progress.visible;
    progressBox.classList.toggle("err", progress.error);
    progressBox.classList.toggle("complete", progress.complete);
    progressBox.classList.toggle("active", active);
    $(".bulk-download-progress-label", section).textContent = progress.text;
    progressBar.setAttribute("aria-valuetext", progress.text);
    if (progress.percent == null) progressBar.removeAttribute("value");
    else progressBar.setAttribute("value", String(progress.percent));
    const counts = $(".bulk-download-counts", section);
    counts.hidden = !runtime || runtime.state === "idle";
    counts.textContent = runtime ? `已保存 ${runtime.savedCount || 0} 封 · 已有 ${runtime.existingCount || 0} 封 · 跳过 ${runtime.skippedCount || 0} 封${runtime.vanishedCount ? ` · 已从服务器移除 ${runtime.vanishedCount} 封` : ""} · 失败 ${runtime.failedCount || 0} 项 · 文件夹 ${runtime.foldersDone || 0} / ${runtime.folderCount || 0}` : "";
    const folder = $(".bulk-download-folder", section);
    folder.hidden = !runtime?.currentFolder;
    folder.textContent = runtime?.currentFolder ? `当前文件夹：${runtime.currentFolder}` : "";
    const directory = $(".bulk-download-directory", section);
    directory.hidden = !runtime?.directory;
    directory.textContent = runtime?.directory ? `保存位置：${runtime.directory}` : "";
    const error = $(".bulk-download-error", section);
    error.textContent = S.bulkErrors.get(id) || "";
    error.hidden = !error.textContent;
  }
  renderSettingsAccounts(accountId);
}

async function startBulkDownload(section) {
  const accountId = section.dataset.bulkDownloadAccount;
  if (S.bulkPending.has(accountId) || bulkDownloadActive(S.bulkDownloads[accountId])) return;
  // 在选择文件夹前锁定本次选择；重新打开设置也不能改变待启动请求。
  const inheritFilters = $(".bulk-download-inherit", section).checked === true;
  S.bulkInheritFilters.set(accountId, inheritFilters);
  S.bulkPending.set(accountId, "choosing");
  S.bulkErrors.delete(accountId);
  renderBulkDownloadStatus(accountId);
  try {
    const directory = await invoke("choose_download_directory", { directory: S.bulkDirectories.get(accountId) || autoDownloadSettings(account(accountId)).directory || null });
    if (directory == null || !account(accountId)) return;
    S.bulkDirectories.set(accountId, directory);
    S.bulkPending.set(accountId, "starting");
    renderBulkDownloadStatus(accountId);
    const eventSeq = S.bulkEventSeq.get(accountId);
    const status = await invoke("start_bulk_download", { accountId, directory, inheritFilters });
    if (account(accountId) && S.bulkEventSeq.get(accountId) === eventSeq) S.bulkDownloads[accountId] = status;
  } catch (e) {
    if (account(accountId)) S.bulkErrors.set(accountId, String(e));
  } finally {
    S.bulkPending.delete(accountId);
    renderBulkDownloadStatus(accountId);
  }
}

async function cancelBulkDownload(section) {
  const accountId = section.dataset.bulkDownloadAccount;
  const runtime = S.bulkDownloads[accountId];
  if (S.bulkPending.has(accountId) || !bulkDownloadActive(runtime) || runtime.state === "cancelling") return;
  S.bulkPending.set(accountId, "cancelling");
  S.bulkErrors.delete(accountId);
  renderBulkDownloadStatus(accountId);
  const eventSeq = S.bulkEventSeq.get(accountId);
  try {
    const status = await invoke("cancel_bulk_download", { accountId });
    if (account(accountId) && S.bulkEventSeq.get(accountId) === eventSeq) S.bulkDownloads[accountId] = status;
  } catch (e) {
    if (account(accountId)) S.bulkErrors.set(accountId, String(e));
  } finally {
    S.bulkPending.delete(accountId);
    renderBulkDownloadStatus(accountId);
  }
}

async function saveSignature(form) {
  const button = $('button[type="submit"]', form);
  if (button.disabled) return;
  const status = $(".signature-status", form);
  button.disabled = true;
  button.textContent = "正在保存…";
  status.hidden = true;
  try {
    const updated = await invoke("save_signature", { accountId: form.dataset.signatureAccount, signature: $("textarea", form).value });
    const old = account(updated.id);
    if (old) old.signature = updated.signature;
    status.textContent = updated.signature ? "签名已保存" : "签名已关闭";
    status.classList.remove("err");
  } catch (e) {
    status.textContent = String(e);
    status.classList.add("err");
  } finally {
    status.hidden = false;
    button.disabled = false;
    button.textContent = "保存签名";
  }
}

function autoDownloadSettings(a) {
  const config = a?.autoDownload;
  return {
    enabled: config?.enabled === true,
    directory: config?.directory || "",
    filterMode: ["whitelist", "blacklist"].includes(config?.filterMode) ? config.filterMode : "none",
    keywords: Array.isArray(config?.keywords) ? config.keywords : [],
  };
}

function autoDownloadKeywords(text, mode) {
  if (!["none", "whitelist", "blacklist"].includes(mode)) throw new Error("请选择有效的关键词过滤模式");
  const seen = new Set(), keywords = [];
  for (const line of text.split(/\r?\n/)) {
    const keyword = line.trim();
    if (!keyword) continue;
    if ([...keyword].length > 200) throw new Error("每个关键词最多 200 个字符");
    const key = keyword.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    keywords.push(keyword);
  }
  if (keywords.length > 100) throw new Error("最多可设置 100 个关键词");
  if (mode !== "none" && !keywords.length) throw new Error("请至少填写一个关键词，或选择不过滤");
  return keywords;
}

function autoDownloadForm(a) {
  const config = autoDownloadSettings(a);
  return `<form class="auto-download-form" data-auto-download-account="${esc(a.id)}" aria-label="${esc(a.email)} 的自动下载设置">
    <label class="auto-download-toggle"><input type="checkbox" name="enabled" ${config.enabled ? "checked" : ""}>自动下载新邮件</label>
    <label class="auto-download-path-label" for="download-directory-${esc(a.id)}">本地保存位置</label>
    <div class="auto-download-path"><input type="text" id="download-directory-${esc(a.id)}" name="directory" value="${esc(config.directory)}" placeholder="选择文件夹或输入完整路径" spellcheck="false" autocomplete="off"><button class="secondary" type="button" data-action="choose-download-directory">选择文件夹…</button></div>
    <label class="auto-download-path-label" for="download-filter-${esc(a.id)}">关键词过滤</label>
    <select class="auto-download-filter" id="download-filter-${esc(a.id)}" name="filterMode">
      <option value="none" ${config.filterMode === "none" ? "selected" : ""}>不过滤</option>
      <option value="whitelist" ${config.filterMode === "whitelist" ? "selected" : ""}>白名单：命中才下载</option>
      <option value="blacklist" ${config.filterMode === "blacklist" ? "selected" : ""}>黑名单：命中则跳过</option>
    </select>
    <label class="auto-download-path-label" for="download-keywords-${esc(a.id)}">关键词（每行一个）</label>
    <textarea class="auto-download-keywords" id="download-keywords-${esc(a.id)}" name="keywords" rows="3" placeholder="例如：GitHub&#10;发票" spellcheck="false" aria-describedby="download-filter-hint-${esc(a.id)}" ${config.filterMode === "none" ? "disabled" : ""}>${esc(config.keywords.join("\n"))}</textarea>
    <p class="hint" id="download-filter-hint-${esc(a.id)}">任一关键词包含于主题、发件人名称或地址、正文即视为命中；不区分大小写，不匹配附件。最多 100 个关键词，每个最多 200 个字符。选择不过滤时，已填写的关键词会保留，暂不生效。</p>
    <p class="hint">仅将启用后新到收件箱的邮件保存为完整 .eml（含附件），不下载历史邮件。MailPulse 运行时自动保存，重新打开后补下关闭期间的新邮件。更改路径或有效过滤规则，仅影响保存后的新邮件，不追溯旧邮件。</p>
    <div class="auto-download-actions"><p class="auto-download-runtime" role="status"></p><button class="secondary" type="submit">保存设置</button></div>
    <p class="auto-download-last" hidden></p>
    <p class="auto-download-status" role="status" hidden></p>
  </form>`;
}

function renderAutoDownloadStatus(accountId) {
  const overlay = $("#overlay");
  if (!overlay) return;
  for (const form of overlay.querySelectorAll(".auto-download-form")) {
    const id = form.dataset.autoDownloadAccount;
    if (accountId && accountId !== id) continue;
    const config = autoDownloadSettings(account(id));
    const runtime = S.autoDownloads[id];
    const status = $(".auto-download-runtime", form);
    const labels = { disabled: "未开启自动下载", waiting: "等待新邮件", downloading: "正在下载新邮件…", error: "自动下载失败，稍后重试" };
    status.textContent = runtime?.detail || labels[runtime?.state] || (config.enabled ? "等待自动下载状态…" : labels.disabled);
    if (runtime?.savedCount > 0) status.textContent += ` · 已保存 ${runtime.savedCount} 封`;
    status.classList.toggle("err", runtime?.state === "error");
    const last = $(".auto-download-last", form);
    last.hidden = !runtime?.lastPath;
    last.textContent = runtime?.lastPath ? `最近保存：${runtime.lastPath}` : "";
  }
}

function autoDownloadChanged(form) {
  $('[name="keywords"]', form).disabled = form._autoDownloadBusy || $('[name="filterMode"]', form).value === "none";
  const status = $(".auto-download-status", form);
  status.textContent = "更改尚未保存";
  status.hidden = false;
  status.classList.remove("err");
}

function autoDownloadBusy(form, busy) {
  form._autoDownloadBusy = busy;
  for (const input of form.querySelectorAll("input, select, textarea, button")) input.disabled = busy;
  $('[name="keywords"]', form).disabled = busy || $('[name="filterMode"]', form).value === "none";
}

async function chooseAutoDownloadDirectory(form) {
  if (form._autoDownloadBusy) return;
  const input = $('[name="directory"]', form);
  const status = $(".auto-download-status", form);
  autoDownloadBusy(form, true);
  try {
    const directory = await invoke("choose_download_directory", { directory: input.value || null });
    if (directory != null) {
      input.value = directory;
      autoDownloadChanged(form);
    }
  } catch (e) {
    status.textContent = String(e);
    status.classList.add("err");
    status.hidden = false;
  } finally {
    autoDownloadBusy(form, false);
  }
}

async function saveAutoDownload(form) {
  if (form._autoDownloadBusy) return;
  const accountId = form.dataset.autoDownloadAccount;
  const button = $('button[type="submit"]', form);
  const status = $(".auto-download-status", form);
  const enabled = $('[name="enabled"]', form).checked;
  const directory = $('[name="directory"]', form).value;
  const filterMode = $('[name="filterMode"]', form).value;
  autoDownloadBusy(form, true);
  button.textContent = "正在保存…";
  status.hidden = true;
  try {
    const keywords = autoDownloadKeywords($('[name="keywords"]', form).value, filterMode);
    const updated = await invoke("save_auto_download", { accountId, enabled, directory, filterMode, keywords });
    const old = account(accountId);
    // 其他设置可能同时保存，只合并本次更改的字段。
    if (old) old.autoDownload = updated.autoDownload;
    const saved = autoDownloadSettings(updated);
    $('[name="enabled"]', form).checked = saved.enabled;
    $('[name="directory"]', form).value = saved.directory;
    $('[name="filterMode"]', form).value = saved.filterMode;
    $('[name="keywords"]', form).value = saved.keywords.join("\n");
    status.textContent = saved.enabled ? "自动下载已开启，设置已保存" : "自动下载已关闭，设置已保存";
    status.classList.remove("err");
    renderAutoDownloadStatus(accountId);
    renderBulkDownloadStatus(accountId);
  } catch (e) {
    status.textContent = String(e);
    status.classList.add("err");
  } finally {
    status.hidden = false;
    autoDownloadBusy(form, false);
    button.textContent = "保存设置";
  }
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
document.addEventListener("contextmenu", (e) => {
  const row = e.target.closest?.(".row[data-open]");
  if (!row) return;
  e.preventDefault();
  // 不打开邮件，避免右键操作把未读邮件变成已读。
  showMessageMenu(row, rowSelection(row), { x: e.clientX, y: e.clientY });
});

document.addEventListener("click", async (e) => {
  const t = e.target;
  if (t.closest("[data-close]")) return closeOverlay();
  if (t.classList.contains("scrim") && !t.querySelector(".sheet .compose-body")) return closeOverlay();

  const kind = t.closest(".kind[data-kind]");
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
      delete S.autoDownloads[a.id];
      delete S.bulkDownloads[a.id];
      S.bulkPending.delete(a.id);
      S.bulkEventSeq.delete(a.id);
      S.bulkErrors.delete(a.id);
      S.bulkDirectories.delete(a.id);
      S.bulkInheritFilters.delete(a.id);
      S.settingsExpanded.delete(a.id);
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
    return openMessage(rowSelection(openRow));
  }
  const goBtn = t.closest("[data-go]");
  if (goBtn) return go(goBtn.dataset.go);
  const sw = t.closest("[data-switch]");
  if (sw) return go("INBOX", sw.dataset.switch);
  const att = t.closest("[data-att]");
  if (att) {
    try {
      const path = await invoke("save_attachment", JSON.parse(att.dataset.attachmentSource));
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
    case "toggle-settings-account": return toggleSettingsAccount(btn.closest(".settings-account"));
    case "choose-download-directory": return chooseAutoDownloadDirectory(btn.closest(".auto-download-form"));
    case "start-bulk-download": return startBulkDownload(btn.closest(".bulk-download-section"));
    case "cancel-bulk-download": return cancelBulkDownload(btn.closest(".bulk-download-section"));
    case "account-menu": return accountMenu(btn);
    case "compose": return openCompose();
    case "compose-close": return requestComposeClose();
    case "compose-continue": return hideComposeConfirmation(btn.closest(".sheet"));
    case "compose-discard": {
      const box = btn.closest(".compose-confirm");
      if (!box.hidden && box.dataset.kind === "discard") return closeOverlay();
      return;
    }
    case "compose-send-confirm": return sendCompose({ allowEmptySubject: true });
    case "quote-images": {
      const sheet = btn.closest(".sheet");
      sheet._quoteRemoteImages = true;
      btn.hidden = true;
      return mountComposeQuote(sheet);
    }
    case "overlay-close": return closeOverlay();
    case "show-cc":
      document.querySelectorAll(".cc-row").forEach((x) => (x.hidden = false));
      btn.remove();
      return $("#c-cc").focus();
    case "send": return sendCompose();
    case "settings": return openSettings();
    case "create-folder": return openCreateFolder();
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
        { label: "刷新", icon: "refresh", run: () => {
          const targets = S.acct === "all" ? S.accounts.map((a) => a.id) : [S.acct];
          for (const id of targets) { loadFolders(id); loadList(id, S.folder, { force: true }); }
        } },
      ]);
    case "more": return loadList(S.acct, S.folder, { more: true });
    case "close": ++openSeq; S.sel = null; S.msg = null; return render();
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
    case "move-menu": return showMoveMenu(btn, m);
    case "msg-menu": return showMessageMenu(btn, m);
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
  } else if (e.target.id === "folder-form") {
    e.preventDefault();
    submitFolder(e.target);
  } else if (e.target.matches(".signature-form")) {
    e.preventDefault();
    saveSignature(e.target);
  } else if (e.target.matches(".auto-download-form")) {
    e.preventDefault();
    saveAutoDownload(e.target);
  }
});

document.addEventListener("beforeinput", (e) => {
  if (e.target.id !== "c-body") return;
  delete e.target.closest(".sheet")._signatureEdit;
  let start = e.target.selectionStart, end = e.target.selectionEnd;
  if (start === end) {
    if (e.inputType === "deleteContentBackward") start = Math.max(0, start - 1);
    else if (e.inputType === "deleteContentForward") end = Math.min(e.target.value.length, end + 1);
    else if (!e.inputType.startsWith("insert")) return;
  }
  e.target.closest(".sheet")._signatureEdit = { start, end };
});

document.addEventListener("input", (e) => {
  const downloadForm = e.target.closest(".auto-download-form");
  if (downloadForm) return autoDownloadChanged(downloadForm);
  if (e.target.id === "q") {
    S.query = e.target.value;
    renderList();
  } else if (e.target.id === "c-body") {
    const sheet = e.target.closest(".sheet");
    sheet._signatureDraft = window.MailPulseSignature.editSignatureDraft(sheet._signatureDraft, e.target.value, sheet._signatureEdit);
    delete sheet._signatureEdit;
  }
});

document.addEventListener("change", (e) => {
  if (e.target.classList?.contains("bulk-download-inherit")) {
    return changeBulkDownloadInheritance(e.target.closest(".bulk-download-section"), e.target.checked);
  }
  const downloadForm = e.target.closest(".auto-download-form");
  if (downloadForm) return autoDownloadChanged(downloadForm);
  if (e.target.id !== "c-from") return;
  const sheet = e.target.closest(".sheet");
  const body = $("#c-body", sheet);
  const helper = window.MailPulseSignature;
  const current = helper.editSignatureDraft(sheet._signatureDraft, body.value);
  sheet._signatureDraft = helper.switchSignature(current, account(e.target.value)?.signature || "");
  body.value = sheet._signatureDraft.text;
});

function onKey(e) {
  const typing = e.target.closest?.("input, textarea, select");
  const overlay = $("#overlay").innerHTML !== "";
  const row = e.target.closest?.(".row[data-open]");
  if (!overlay && row && (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10"))) {
    e.preventDefault();
    return showMessageMenu(row, rowSelection(row));
  }
  if (e.metaKey && e.key === "Enter" && $("#c-body")) {
    e.preventDefault();
    return sendCompose();
  }
  if (e.key === "Escape") {
    if (overlay) {
      const sheet = $("#c-body")?.closest(".sheet");
      if (!sheet) return closeOverlay();
      e.preventDefault();
      if (sheet.dataset.busy === "true") return;
      return !$(".compose-confirm", sheet).hidden ? hideComposeConfirmation(sheet) : requestComposeClose();
    }
    if (S.searching) { S.searching = false; S.query = ""; return render(); }
    if (S.sel) { ++openSeq; S.sel = null; S.msg = null; return render(); }
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
  S.autoDownloads = { ...(snap.autoDownloads || {}), ...S.autoDownloads };
  S.bulkDownloads = { ...(snap.bulkDownloads || {}), ...S.bulkDownloads };
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
