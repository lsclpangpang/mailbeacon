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
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  template: '<rect x="4" y="3" width="16" height="18" rx="2"/><path d="M8 8h8M8 12h8M8 16h5"/>',
  mail: '<rect x="2" y="4" width="20" height="16" rx="2"/><path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7"/>',
  building: '<rect x="5" y="3" width="14" height="18" rx="2"/><path d="M9 7h1m4 0h1M9 11h1m4 0h1M9 15h1m4 0h1M10 21v-3h4v3"/>',
  junk: '<circle cx="12" cy="12" r="9.5"/><path d="M12 7.5v5M12 16.5h.01"/>',
  trash: '<path d="M3 6h18M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>',
  archive: '<rect x="2" y="3" width="20" height="5" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8M10 12h4"/>',
  folder: '<path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z"/>',
  x: '<path d="M18 6 6 18M6 6l12 12"/>',
  left: '<path d="m15 18-6-6 6-6"/>',
  right: '<path d="m9 18 6-6-6-6"/>',
  updown: '<path d="m7 15 5 5 5-5M7 9l5-5 5 5"/>',
  up: '<path d="m18 15-6-6-6 6"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  more: '<circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/><circle cx="5" cy="12" r="1"/>',
  sidebar: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9.5 4v16"/>',
  filter: '<path d="M22 3H2l8 9.46V19l4 2v-8.54z"/>',
  reply: '<path d="M9 17 4 12l5-5"/><path d="M20 18v-2a4 4 0 0 0-4-4H4"/>',
  replyAll: '<path d="m7 17-5-5 5-5M12 17l-5-5 5-5"/><path d="M22 18v-2a4 4 0 0 0-4-4H7"/>',
  forward: '<path d="m15 17 5-5-5-5"/><path d="M4 18v-2a4 4 0 0 1 4-4h12"/>',
  list: '<path d="M8 6h13"/><path d="M8 12h13"/><path d="M8 18h13"/><path d="M3 6h.01"/><path d="M3 12h.01"/><path d="M3 18h.01"/>',
  listOrdered: '<path d="M10 6h11"/><path d="M10 12h11"/><path d="M10 18h11"/><path d="M4 6h1v4"/><path d="M4 10h2"/><path d="M6 18H4c0-1 2-2 2-3s-1-1.5-2-1"/>',
  alignLeft: '<path d="M21 6H3"/><path d="M15 12H3"/><path d="M17 18H3"/>',
  alignCenter: '<path d="M21 6H3"/><path d="M17 12H7"/><path d="M19 18H5"/>',
  alignRight: '<path d="M21 6H3"/><path d="M21 12H9"/><path d="M21 18H7"/>',
  link: '<path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>',
  image: '<rect width="18" height="18" x="3" y="3" rx="2"/><circle cx="9" cy="9" r="2"/><path d="m21 15-3.09-3.09a2 2 0 0 0-2.82 0L6 21"/>',
  table: '<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M3 15h18"/><path d="M9 3v18"/><path d="M15 3v18"/>',
  eraser: '<path d="m7 21-4.3-4.3c-1-1-1-2.5 0-3.4l9.6-9.6c1-1 2.5-1 3.4 0l5.6 5.6c1 1 1 2.5 0 3.4L13 21"/><path d="M22 21H7"/><path d="m5 11 9 9"/>',
  users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
  edit: '<path d="M12 20h9"/><path d="M16.38 3.62a2.12 2.12 0 1 1 3 3L7.37 18.64a2 2 0 0 1-.86.5l-2.87.84.84-2.87a2 2 0 0 1 .5-.86z"/>',
  clip: '<path d="m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48"/>',
  app: '<rect x="3" y="3" width="18" height="18" rx="4"/><path d="M8 3v18M8 8h13"/>',
  file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
  unread: '<path d="M21.2 8.4c.5.38.8.97.8 1.6v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V10a2 2 0 0 1 .8-1.6l8-6a2 2 0 0 1 2.4 0z"/><path d="m22 10-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 10"/>',
  plus: '<path d="M5 12h14M12 5v14"/>',
  check: '<path d="M20 6 9 17l-5-5"/>',
  checkAll: '<path d="M18 6 7 17l-5-5M22 10l-7.5 7.5L13 16"/>',
  refresh: '<path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16"/><path d="M16 16h5v5"/>',
  download: '<path d="M12 3v12m-5-5 5 5 5-5M4 16v4h16v-4"/>',
};
const icon = (name, cls = "") => `<svg class="i ${cls}" viewBox="0 0 24 24">${ICON[name]}</svg>`;

const ROLE_ICON = { inbox: "inbox", drafts: "draft", sent: "sent", all: "mail", junk: "junk", trash: "trash", archive: "archive", other: "folder", outbox: "clock" };
const X = () => window.MailBeaconExtras;
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
  bodyCache: {},       // accountId → 邮件缓存的占用和后台预热进度
  archives: {},        // accountId → 当缓存用的下载目录
  bulkPending: new Map(),
  bulkEventSeq: new Map(),
  bulkErrors: new Map(),
  bulkDirectories: new Map(),
  bulkInheritFilters: new Map(), // accountId → 本次会话的全量下载过滤选择
  accountRepairEvents: new Map(),
  settingsExpanded: new Map(), // 折叠只隐藏现有表单，保留未保存输入
  folders: {},         // accountId → [{ path, name, role }]
  folderSeq: new Map(),
  lists: new Map(),    // "acct|folder" → [summary]（folder 也可能是 "__starred"）
  results: new Map(),  // "acct|folder" → { query, items }：服务器搜索结果，不覆盖 lists
  loading: new Set(),  // 正在加载的 "acct|folder"
  exhausted: new Set(),// 已经翻到底的 "acct|folder"
  morePages: new Map(),// "acct|folder" → 后端说后面还有更早的邮件
  listSeq: new Map(),  // 忽略已经被后续加载替代的列表请求
  moving: new Set(),
  batch: new Set(),
  batchAnchor: null,
  batchScope: "",
  archiveErrors: {},
  lastOperation: null,
  downloading: new Set(),
  removed: new Set(),  // 已经被服务器确认移走的 UID，拦截旧推送快照
  previews: new Map(), // "acct|folder|uid" → 摘要
  acct: "all",         // 当前账号，或 "all"
  folder: "INBOX",     // 当前文件夹 path，或 "__starred"
  sel: null,           // { accountId, folder, uid }
  msg: null,           // 打开的邮件正文
  msgLoading: false,
  msgProgress: 0,      // 正在下载的原文已收到的字节数
  msgTotal: null,      // 整封大小（后端查到才有）
  msgError: "",
  readerHintUntil: 0,  // 本次打开的快捷键提示截止时间；普通重绘不重新计时
  showImages: new Set(),
  searching: false,
  searchComposing: false, // 中文/日文输入法组字期间不要重建输入框
  searchSeq: 0,
  searchErrors: [],
  checking: false, // 「收取」进行中
  indexing: new Map(),  // "acct|folder" → { query, running, complete, indexed, total, error }
  query: "",
  filter: "all",       // 列表筛选：all | unread | flagged | attachment | mentions
  ui: { listWidth: 400, collapsed: false }, // 列表宽度、侧栏是否收起（本地保存）
  prefs: null,         // 通知、托盘、开机启动（get_prefs）；保存成功后才更新
  prefsWanted: null,   // 还在保存中的设置：连续改动在它的基础上叠加
  prefsQueue: Promise.resolve(),
  prefsSeq: 0,
  folderErrors: new Map(), // accountId → 读取文件夹失败的原因（设置里的提醒文件夹）
  rules: [],           // 收信规则（服务器确认保存过的）
  rulesUi: null,       // 收信规则窗口：{ editing: { rule, index } | null, saving, focus }
  ruleErrors: new Map(), // 收信规则错误 → 上次提示的时间，同样的错误不反复弹
  templates: [],       // 邮件模板
  outbox: [],          // 定时发送的邮件（outbox_list）
  receiptsHandled: new Set(), // 本机已经回过或拒绝过回执的邮件（服务器不支持 $MDNSent 时也不再问）
  threads: false,      // 会话模式：列表按会话分组，阅读区列出整个会话
  thread: null,        // 正在看的会话：{ key, items }（含已发送等其他文件夹里的邮件）
  threadSeq: 0,
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

// 账户色独立于联系人头像；先用差异较大的色相，并为每个账户记住色槽。
const ACCOUNT_PALETTE = [
  "#2f6fe4", "#c47710", "#168b55", "#8d5bd6", "#d34d75", "#148ba6",
  "#dc1818", "#809210", "#e119b3", "#12a012", "#b419e4", "#c77258",
  "#846d2c", "#6788cc", "#cb64b6", "#f15c4b",
];
const accountColorSlots = new Map();
const accountSlotColors = new Map(ACCOUNT_PALETTE.map((color, slot) => [slot, color]));
let accountColorsLoaded = false;
function accountSlotColor(slot) {
  if (accountSlotColors.has(slot)) return accountSlotColors.get(slot);
  const hue = ((slot - ACCOUNT_PALETTE.length) * 137.507764 + 55) % 360;
  // 补充色保持相近的感知亮度，在浅色与深色背景上都能看清。
  let low = 0, high = 1;
  for (let i = 0; i < 16; i++) {
    const light = (low + high) / 2, amplitude = .65 * Math.min(light, 1 - light);
    const rgb = [0, 8, 4].map((n) => {
      const k = (n + hue / 30) % 12;
      const c = light - amplitude * Math.max(-1, Math.min(k - 3, 9 - k, 1));
      return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4;
    });
    const luminance = rgb.reduce((sum, c, index) => sum + c * [.2126, .7152, .0722][index], 0);
    if (luminance < .2) low = light; else high = light;
  }
  const color = `hsl(${hue.toFixed(3)}, 65%, ${((low + high) * 50).toFixed(2)}%)`;
  accountSlotColors.set(slot, color);
  return color;
}
function accountColor(a) {
  if (!accountColorsLoaded) {
    accountColorsLoaded = true;
    try {
      const saved = JSON.parse(localStorage.getItem("mb.account-colors") || "[]");
      const used = new Set();
      if (Array.isArray(saved)) for (const entry of saved) {
        if (!Array.isArray(entry)) continue;
        const [id, slot] = entry;
        if (typeof id !== "string" || !id || !Number.isSafeInteger(slot) || slot < 0 || slot >= 10000 || used.has(slot) || accountColorSlots.has(id)) continue;
        accountColorSlots.set(id, slot); used.add(slot);
      }
    } catch {} // 本地存储不可用时，在当前会话内保持配色。
  }
  const used = new Set(accountColorSlots.values());
  let changed = false, next = 0;
  for (const item of [...S.accounts, a]) {
    if (!item?.id || accountColorSlots.has(item.id)) continue;
    while (used.has(next)) next++;
    accountColorSlots.set(item.id, next); used.add(next); changed = true;
  }
  if (changed) {
    try { localStorage.setItem("mb.account-colors", JSON.stringify([...accountColorSlots])); } catch {}
  }
  return accountSlotColor(accountColorSlots.get(a?.id) ?? 0);
}

// 账户用邮箱类型识别；发件人与联系人继续使用姓名头像。
function accountBadge(a, cls = "") {
  const provider = a ? KINDS.find((x) => x.id === a.kind) : null;
  const kind = a ? provider?.id || "custom" : "all";
  let mark = icon(kind === "all" ? "inbox" : kind === "exmail" ? "building" : "mail");
  if (kind === "gmail") mark = `<svg class="account-mark" viewBox="0 0 24 24" fill="none">
    <path fill="#4285f4" d="M2 6.5 5.5 9v11H3a1 1 0 0 1-1-1z"/>
    <path fill="#34a853" d="m18.5 9 3.5-2.5V19a1 1 0 0 1-1 1h-2.5z"/>
    <path fill="#ea4335" d="m5.5 4 6.5 5 6.5-5v5L12 14 5.5 9z"/>
    <path fill="#c5221f" d="M2 6.5V5a2 2 0 0 1 3.5-1v5z"/>
    <path fill="#fbbc04" d="M18.5 4A2 2 0 0 1 22 5v1.5L18.5 9z"/>
  </svg>`;
  else if (["qq", "163", "126"].includes(kind)) mark = `<span class="account-wordmark">${kind === "qq" ? "QQ" : kind}</span>`;
  return `<span class="account-badge ${cls}" data-provider="${kind}" aria-hidden="true" title="${esc(kind === "all" ? "所有账户" : provider?.label || "邮箱")}">${mark}${a ? `<span class="account-color" style="background:${accountColor(a)}"></span>` : ""}</span>`;
}

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
  t.innerHTML = `<span>${esc(text)}</span>${isErr ? '<button data-action="dismiss-toast" aria-label="关闭提示">×</button>' : ""}`;
  t.className = "toast" + (isErr ? " err" : "");
  t.hidden = false;
  clearTimeout(toastTimer);
  if (!isErr) toastTimer = setTimeout(() => (t.hidden = true), 2400);
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
  const query = S.query.trim();
  const q = query.toLocaleLowerCase();
  const ids = S.acct === "all" ? S.accounts.map((a) => a.id) : [S.acct];
  let items = ids.flatMap((id) => {
    const k = key(id, S.folder);
    // 服务器已按正文等字段匹配过，不再用本地摘要二次过滤；结果到达前先在已加载邮件中即时筛选。
    const result = S.results.get(k);
    if (q && result?.query === query) return result.items;
    const list = S.lists.get(k) || [];
    if (!q) return list;
    return list.filter((m) =>
      [m.fromName, m.fromAddr, m.subject, m.to, S.previews.get(key(key(m.accountId, m.folder), m.uid))]
        .some((x) => typeof x === "string" && x.toLocaleLowerCase().includes(q)));
  });
  // 按收件时间排，不按 UID：导入/迁移过的邮箱里两者顺序可能不一致
  items = items.slice().sort((a, b) => b.date - a.date || b.uid - a.uid);
  // 「所有未读」里刚读过的邮件移出列表（正在看的那封除外）
  if (S.folder === "__unread") items = items.filter((m) => !m.seen || isSel(m));
  if (S.folder === "__starred") items = items.filter((m) => m.flagged || isSel(m));
  // 搜索时筛选栏隐藏，筛选也不生效，避免结果被看不见的条件过滤
  if (S.searching || q) return items;
  return window.MailBeaconViews.filterMessages(items, S.filter, (id) => [account(id)?.email].filter(Boolean), isSel);
}
/** 所有缓存的邮件列表，包括搜索结果 */
const allLists = () => [...S.lists.values(), ...[...S.results.values()].map((r) => r.items), ...(S.thread ? [S.thread.items] : [])];
const isSel = (m) => S.sel && S.sel.accountId === m.accountId && S.sel.folder === m.folder && S.sel.uid === m.uid;

const sameMsg = (m, accountId, folder, uid) => m.accountId === accountId && m.folder === folder && m.uid === uid;
function findMsg(sel) {
  if (!sel) return null;
  for (const list of allLists()) {
    const m = list.find((x) => sameMsg(x, sel.accountId, sel.folder, sel.uid));
    if (m) return m;
  }
  return null;
}

/** 本地修改某封邮件（所有列表里的副本一起改） */
function patchMsg(accountId, folder, uid, patch) {
  for (const list of allLists()) {
    for (const m of list) if (sameMsg(m, accountId, folder, uid)) Object.assign(m, patch);
  }
}
function dropMsg(accountId, folder, uid) {
  // 会话里删掉、移走的邮件不再显示，下次打开时重新查
  if (S.thread) {
    S.thread.items = S.thread.items.filter((m) => !sameMsg(m, accountId, folder, uid));
    S.thread.stale = true;
  }
  for (const [k, list] of S.lists) {
    if (!k.startsWith(accountId + "|")) continue;
    S.lists.set(k, list.filter((m) => !sameMsg(m, accountId, folder, uid)));
  }
  for (const r of S.results.values()) r.items = r.items.filter((m) => !sameMsg(m, accountId, folder, uid));
}

async function loadFolders(accountId, { report = false } = {}) {
  const seq = (S.folderSeq.get(accountId) || 0) + 1;
  S.folderSeq.set(accountId, seq);
  try {
    const folders = await invoke("list_folders", { accountId });
    if (S.folderSeq.get(accountId) === seq) {
      S.folders[accountId] = folders;
      S.folderErrors.delete(accountId);
      redrawNotifySection(accountId);
      // 之前因为缺少文件夹列表没能加载的聚合视图，现在补上
      const view = S.folder;
      const k = key(accountId, view);
      if (window.MailBeaconViews.isVirtual(view) && (S.acct === accountId || S.acct === "all") && !S.lists.has(k) && !S.loading.has(k)) {
        loadList(accountId, view, { force: true });
      }
      render();
    }
    return folders;
  } catch (e) {
    if (report) throw e;
    console.warn("读取文件夹失败", e);
  }
}

/**
 * list_messages 的返回值：{ items, complete }。`complete` 表示后端已经把整个
 * 文件夹一次给全了（摘要早就索引到本地），界面不用再显示「加载更早的邮件」。
 * 旧版本后端和测试里的假数据只有数组，按“还有更早的”处理。
 */
function listedPage(res) {
  if (Array.isArray(res)) return { items: res, complete: false, more: null };
  if (Array.isArray(res?.items)) {
    return {
      items: res.items,
      complete: !!res.complete,
      more: typeof res.more === "boolean" ? res.more : null,
    };
  }
  // 不是约定的结构（旧后端、测试里的空返回值）：不动当前列表
  return { items: null, complete: false, more: null };
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
    const virtual = window.MailBeaconViews.isVirtual(folder);
    let response;
    if (virtual) {
      // 聚合视图：读取该账号对应的真实文件夹后合并，邮件仍带各自的真实文件夹
      if (more) return;
      const folders = S.folders[accountId] || (await loadFolders(accountId));
      // 读不到文件夹列表时不能当成「没有邮件」缓存下来；列表读到后会自动重新加载
      if (!folders && window.MailBeaconViews.needsFolders(folder)) return;
      const parts = await Promise.all(window.MailBeaconViews.sources(folder, folders || []).map((src) => invoke("list_messages", {
        accountId, folder: src.folder, before: null, flaggedOnly: !!src.flaggedOnly, unreadOnly: !!src.unreadOnly,
      })));
      response = parts
        .flatMap((part) => listedPage(part).items || [])
        .sort((x, y) => y.date - x.date || y.uid - x.uid);
    } else {
      response = await invoke("list_messages", { accountId, folder, before, flaggedOnly: false, unreadOnly: false });
    }
    if (S.listSeq.get(k) !== seq) return;
    // 这次请求已通过代次检查，是移动完成后重新读取的服务器结果。
    // 若 UID 被服务器重新使用，以服务器为准，不永久隐藏它。
    const page = virtual ? { items: response, complete: false } : listedPage(response);
    const got = page.items;
    if (!got) return;
    for (const m of got) S.removed.delete(messageKey(m));
    if (more) {
      S.lists.set(k, keepPresent(existing).concat(got));
      if (got.length === 0) S.exhausted.add(k);
    } else {
      S.lists.set(k, got);
    }
    if (page.complete || page.more === false) {
      // 整个文件夹都在本地了，或者服务器说没有更早的了
      S.exhausted.add(k);
      S.morePages.delete(k);
    } else if (page.more === true) {
      S.exhausted.delete(k);
      S.morePages.set(k, true);
    } else if (!more) {
      // 旧后端/测试里的数组：沿用原来的「加载满 100 封才显示加载更早」
      S.exhausted.delete(k);
      S.morePages.delete(k);
    }
    const byFolder = new Map();
    for (const m of got) byFolder.set(m.folder, (byFolder.get(m.folder) || []).concat(m));
    for (const [real, msgs] of byFolder) loadPreviews(accountId, real, msgs);
  } catch (e) {
    fail(e);
  } finally {
    if (S.listSeq.get(k) === seq) S.loading.delete(k);
    render();
  }
}

let searchTimer = null;
let searchRefreshTimer = null;

/**
 * 后台索引刚建完（或失败）时，本地库里才有最终结果，需要再查一次。
 * 多个账号的完成事件合并成一次请求；用户改了关键词或重新搜索时取消。
 */
function scheduleSearchRefresh() {
  const query = S.query.trim();
  if (!query) return;
  const seq = S.searchSeq;
  clearTimeout(searchRefreshTimer);
  searchRefreshTimer = setTimeout(() => {
    // 这次事件已经过期（用户又输入/发起了新搜索）就不要打扰列表
    if (seq !== S.searchSeq || query !== S.query.trim()) return;
    requestServerSearch();
  }, 150);
}

function requestServerSearch() {
  clearTimeout(searchTimer);
  const query = S.query.trim();
  const seq = ++S.searchSeq;
  S.searchErrors = [];
  // 进度/失败提示是跟一次搜索走的，关键词清空后不再保留
  if (!query) S.indexing.clear();
  // 聚合视图只在已加载的邮件里本地筛选；退出搜索时重新读取。
  if (window.MailBeaconViews.isVirtual(S.folder)) {
    if (!query) {
      const targets = S.acct === "all" ? S.accounts : [account(S.acct)].filter(Boolean);
      targets.forEach((a) => loadList(a.id, S.folder, { force: true }));
    }
    return;
  }
  if (!query) {
    // 搜索结果单独存放，退出搜索时原列表和已翻页的缓存都还在。
    S.results.clear();
    // POP3 首次搜索可能要读很久，退出搜索时让后台停下，释放邮箱连接。
    S.accounts.forEach((a) => invoke("cancel_search", { accountId: a.id }).catch(() => {}));
    return;
  }
  searchTimer = setTimeout(async () => {
    const accounts = S.acct === "all" ? S.accounts : [account(S.acct)].filter(Boolean);
    // 各账号分别结算：一个账号失败不影响其他账号显示服务器结果
    const settled = await Promise.allSettled(accounts.map((a) => invoke("search_messages", {
      accountId: a.id, folder: S.folder, query,
    })));
    if (seq !== S.searchSeq || query !== S.query.trim()) return;
    const errors = [];
    settled.forEach((r, i) => {
      const a = accounts[i];
      const k = key(a.id, S.folder);
      if (r.status === "fulfilled") {
        // 契约：{ items, indexing }；indexing 为 null（POP3）时就是普通结果。
        // 后端灰度期间还可能返回旧数组，按旧结构兜底。
        const items = Array.isArray(r.value) ? r.value : (r.value?.items ?? []);
        const indexing = Array.isArray(r.value) ? null : (r.value?.indexing ?? null);
        S.results.set(k, { query, items });
        loadPreviews(a.id, S.folder, items);
        if (indexing) S.indexing.set(k, { query, ...indexing });
        else S.indexing.delete(k);
      } else {
        // 失败的账号没有服务器结果，列表里只剩已加载邮件的本地匹配，要留着提示；
        // 之前同一关键词的旧结果也清掉，免得提示和列表对不上
        S.results.delete(k);
        errors.push({ email: a.email, message: String(r.reason?.message ?? r.reason) });
      }
    });
    S.searchErrors = errors.map((e) => ({ ...e, query }));
    renderList();
  }, 180);
}

/**
 * 列表顶部的常驻提示，按重要性拼接：
 * 索引建立失败 → 正在建立索引（进度）→ 服务器搜索失败。
 * 只显示当前视图里、与当前关键词匹配的那次搜索。
 */
function searchNotice() {
  const query = S.query.trim();
  if (!query) return "";
  const ids = S.acct === "all" ? S.accounts.map((a) => a.id) : [S.acct];
  const failed = [];
  const running = [];
  for (const id of ids) {
    const state = S.indexing.get(key(id, S.folder));
    if (!state || state.query !== query) continue;
    const email = account(id)?.email || id;
    if (state.error) failed.push({ email, error: String(state.error) });
    else if (state.running && !state.complete) running.push({ email, state });
  }
  const notes = [];
  if (failed.length) {
    const detail = failed.map((e) => `${esc(e.email)}：${esc(e.error)}`).join("；");
    notes.push(`<div class="list-note">本地索引建立失败（${detail}），下面只列出已索引邮件中的匹配。</div>`);
  }
  if (running.length) {
    const detail = running.map(({ email, state }) => {
      const total = Number(state.total) || 0;
      const indexed = Math.max(0, Number(state.indexed) || 0);
      const bodyTotal = Number(state.bodyTotal) || 0;
      const bodyIndexed = Math.max(0, Number(state.bodyIndexed) || 0);
      let counts;
      // 还不知道总数时只是在检查有没有新邮件，不要显示 0/0 封
      if (total === 0) counts = "正在检查新邮件…";
      else if (bodyTotal > 0 && bodyIndexed < bodyTotal) counts = `摘要 ${indexed}/${total} 封，正文 ${bodyIndexed}/${bodyTotal} 封`;
      else counts = `${indexed}/${total} 封`;
      return `${esc(email)}：${counts}`;
    }).join("；");
    notes.push(`<div class="list-note">正在建立本地搜索索引（${detail}），完成后会显示全部匹配；首次需要把历史邮件的摘要和正文开头下载一遍（耗时取决于邮箱大小和服务器速度），之后只做增量更新。</div>`);
  }
  // 服务器搜索失败的提示：这时列表里只是已加载邮件中的匹配，更早的邮件可能没搜到
  const errors = (S.searchErrors || []).filter((e) => e.query === query);
  if (errors.length) {
    const detail = errors.map((e) => `${e.email}：${e.message}`).join("；");
    notes.push(`<div class="list-note">服务器搜索失败（${esc(detail)}），下面只列出已加载邮件中的匹配，更早的邮件可能没有搜到。</div>`);
  }
  return notes.join("");
}

/** 「收取」：让账号（「全部账号」时是每个账号）的收信连接立刻检查新邮件，结果用提示告诉用户。
 * 收件箱的新邮件照常由推送事件加进列表；看着别的文件夹或聚合视图时重新读一遍。 */
async function checkMail() {
  const targets = S.acct === "all" ? S.accounts : [account(S.acct)].filter(Boolean);
  if (S.checking || !targets.length) return;
  // 收取期间可能切换了文件夹：重新读的是开始时那个文件夹
  const folder = S.folder;
  S.checking = true;
  renderList();
  const settled = await Promise.allSettled(targets.map((a) => invoke("check_mail", { accountId: a.id })));
  S.checking = false;
  const received = settled.reduce((n, r) => n + (r.status === "fulfilled" ? r.value : 0), 0);
  const failed = settled.flatMap((r, i) => (r.status === "rejected" ? [`${targets[i].email}：${String(r.reason?.message ?? r.reason)}`] : []));
  if (folder !== "INBOX") for (const a of targets) loadList(a.id, folder, { force: true });
  renderList();
  const news = received ? `收到 ${received} 封新邮件` : "没有新邮件";
  if (!failed.length) return toast(news);
  toast(`${failed.length < targets.length ? `${news}；` : ""}收取失败：${failed.join("；")}`, true);
}

const previewQueue = new Set();
const PREVIEW_BATCH = 20;
// 一次最多给这么多封补正文摘要：长列表只渲染可见窗口，滚动时按窗口分批
// 下载（见 loadWindowPreviews），不会一次为几千封各下载 16KB
const PREVIEW_EAGER = 120;
// 正在下载的邮件正文 { accountId, done }：同一账号的摘要批次先等它（最多几秒），不和读信抢带宽
let opening = null;
const PREVIEW_YIELD_MS = 5000;

const previewKey = (accountId, folder, uid) => key(key(accountId, folder), uid);

async function loadPreviews(accountId, folder, msgs) {
  const need = msgs
    .filter((m) => !S.previews.has(previewKey(accountId, folder, m.uid)) && !previewQueue.has(previewKey(accountId, folder, m.uid)))
    .map((m) => m.uid)
    .slice(0, PREVIEW_EAGER);
  need.forEach((u) => previewQueue.add(previewKey(accountId, folder, u)));
  try {
    for (let i = 0; i < need.length; i += PREVIEW_BATCH) {
      if (opening?.accountId === accountId) {
        let timer;
        await Promise.race([opening.done, new Promise((r) => { timer = setTimeout(r, PREVIEW_YIELD_MS); })]);
        clearTimeout(timer);
      }
      const chunk = need.slice(i, i + PREVIEW_BATCH);
      if (!chunk.length) continue;
      try {
        const got = await invoke("get_previews", { accountId, folder, uids: chunk });
        const texts = new Map(got.map((p) => [p.uid, p.text]));
        // 服务器没给摘要的（有的服务器不支持这段部分 FETCH）记成空串，
        // 免得每次重画列表都再请求一遍
        for (const uid of chunk) S.previews.set(previewKey(accountId, folder, uid), texts.get(uid) || "");
        paintPreviews();
      } catch (e) {
        console.warn("摘要加载失败", e);
      }
    }
  } finally {
    need.forEach((u) => previewQueue.delete(previewKey(accountId, folder, u)));
  }
}

/** 把已经拿到的摘要直接填进列表行，不重画整个列表（几千行时重画很贵）。 */
function paintPreviews() {
  if (typeof document.querySelectorAll !== "function") return;
  for (const row of document.querySelectorAll("#list .rows .row[data-open]")) {
    const prev = row.querySelector(".prev");
    if (!prev || prev.textContent) continue;
    const { accountId, folder, uid } = rowSelection(row);
    const text = S.previews.get(previewKey(accountId, folder, uid));
    if (text) prev.textContent = text;
  }
}

// 行高固定（与 style.css 的 .row 高度一致）：几千封的长列表只渲染可见的
// 一小段，上下用空白占位撑出滚动条，滚动时平移到下一段。
const ROW_H = 66;
const OVERSCAN = 6;
/** 超过这么多行就不再整表渲染（一次性建几千个节点会卡住滚动，界面发白） */
const VIRTUAL_MIN = 120;

let listWindow = null;
let listScrollRaf = 0;

/** 长列表当前该渲染哪一段 [start, end)：由滚动位置和可视高度决定。 */
function listWindowRange(total, scrollTop, height) {
  const count = Math.min(total, Math.ceil((height || 600) / ROW_H) + OVERSCAN * 2);
  const start = Math.max(0, Math.min(Math.floor((scrollTop || 0) / ROW_H) - OVERSCAN, total - count));
  return { start, end: start + count };
}

/** 给窗口里的行按文件夹分批取正文摘要。 */
function loadWindowPreviews(w) {
  const byFolder = new Map();
  for (const g of w.units.slice(w.start, w.end)) {
    const m = g.latest;
    const k = key(m.accountId, m.folder);
    if (!byFolder.has(k)) byFolder.set(k, []);
    byFolder.get(k).push(m);
  }
  for (const [k, msgs] of byFolder) {
    const i = k.indexOf("|");
    loadPreviews(k.slice(0, i), k.slice(i + 1), msgs);
  }
}

/** 只重画可见窗口里的行：DOM 里始终只有几十行。 */
function updateListWindow() {
  const w = listWindow;
  if (!w || !w.rowsEl?.isConnected) return;
  const total = w.units.length;
  const { start, end } = listWindowRange(total, w.rowsEl.scrollTop, w.rowsEl.clientHeight || 600);
  if (start === w.start && end === w.end) return;
  w.start = start;
  w.end = end;
  const win = w.rowsEl.querySelector(".rows-window");
  if (!win) return;
  win.innerHTML = w.units.slice(start, end).map((g) => rowHtml(g, w.grouped, w.multi)).join("");
  const top = w.rowsEl.querySelector(".rows-top");
  const bottom = w.rowsEl.querySelector(".rows-bottom");
  if (top) top.style.height = `${start * ROW_H}px`;
  if (bottom) bottom.style.height = `${Math.max(0, total - end) * ROW_H}px`;
  loadWindowPreviews(w);
}

function onListScroll() {
  if (listScrollRaf) return;
  const run = () => { listScrollRaf = 0; updateListWindow(); };
  listScrollRaf = typeof requestAnimationFrame === "function" ? requestAnimationFrame(run) : setTimeout(run, 16);
}

/** 选中的邮件不在窗口里时（键盘上下、跳转），先把它滚进可见范围。 */
function revealSelectedRow() {
  const row = $(".row.sel");
  if (row) { row.scrollIntoView({ block: "nearest" }); return; }
  const w = listWindow;
  if (!w || !S.sel) return;
  const idx = w.units.findIndex((g) => (w.grouped ? groupSelected(g) : isSel(g.latest)));
  if (idx < 0) return;
  const height = w.rowsEl.clientHeight || 600;
  w.rowsEl.scrollTop = Math.max(0, idx * ROW_H - height / 2);
  updateListWindow();
  $(".row.sel")?.scrollIntoView({ block: "nearest" });
}

function updateBadge() {
  const total = S.accounts.reduce((n, a) => n + (S.live[a.id]?.unread || 0), 0);
  invoke("set_badge", { count: total }).catch(() => {});
}

function onMailEvent(ev) {
  const a = ev.accountId;
  const configured = account(a);
  if (window.MailBeaconAccountRepair && !window.MailBeaconAccountRepair.acceptEvent(configured, ev)) {
    // 新连接可能先于保存命令的答复发布状态；等账户快照更新后再处理。
    if (configured && ev.connectionRevision > (configured.connectionRevision || 1)) {
      const queue = S.accountRepairEvents.get(a) || [];
      queue.push(ev);
      S.accountRepairEvents.set(a, queue.slice(-64));
    }
    return;
  }
  const opened = S.sel && S.sel.accountId === a && S.sel.folder === ev.folder && S.sel.uid === ev.uid;
  if (ev.type === "messageProgress") {
    if (!opened) return;
    S.msgProgress = ev.received;
    S.msgTotal = ev.total ?? null;
    const el = $(".dl-progress");
    if (el) el.textContent = progressText(S.msgProgress, S.msgTotal);
    return;
  }
  // 大邮件先出的文字版：整封还没到时先显示
  if (ev.type === "messagePartial") {
    if (opened && !S.msg && !S.msgError) {
      S.msg = ev.view;
      render();
    }
    return;
  }
  if (ev.type === "listIndex") {
    // 后台把这个文件夹的摘要索引补完了：当前正看着它（或包含它的聚合视图）
    // 就重新读一遍，列表从服务器分页换成一次显示全部
    const folder = ev.folder;
    if (!folder || !(S.acct === "all" || S.acct === a)) return;
    const sources = window.MailBeaconViews.isVirtual(S.folder)
      ? window.MailBeaconViews.sources(S.folder, S.folders[a] || [])
      : [];
    if (S.folder === folder || sources.some((src) => src.folder === folder)) loadList(a, S.folder);
    return;
  }
  if (ev.type === "bodyCache") {
    bodyCacheSeq.set(a, (bodyCacheSeq.get(a) || 0) + 1);
    S.bodyCache[a] = ev;
    renderBodyCacheStatus(a);
    return;
  }
  if (ev.type === "bulkDownload") {
    S.bulkEventSeq.set(a, (S.bulkEventSeq.get(a) || 0) + 1);
    S.bulkDownloads[a] = ev;
    renderBulkDownloadStatus(a);
    // 全量下载开始时目录会被记成缓存来源：设置页上已经列着的话刷新一下
    if (ev.state === "preparing" && S.archives[a]) loadArchives(a);
    return;
  }
  if (ev.type === "autoDownload") {
    S.autoDownloads[a] = { state: ev.state, detail: ev.detail, lastPath: ev.lastPath, savedCount: ev.savedCount };
    renderAutoDownloadStatus(a);
    return;
  }
  if (ev.type === "searchIndex") {
    const folder = ev.folder;
    if (!folder) return;
    const k = key(a, folder);
    const prev = S.indexing.get(k);
    const next = ev.indexing || null;
    if (next) {
      // 事件本身不带关键词：沿用当前搜索词；没有搜索时保留上次的关键词，退出搜索后 clear
      S.indexing.set(k, { query: S.query.trim() || prev?.query || "", ...next });
    } else {
      S.indexing.delete(k);
    }
    const inView = (S.acct === "all" || a === S.acct) && folder === S.folder;
    if (S.searching && inView) renderList();
    // 索引刚建完/失败：本地库这时才有最终结果，防抖再查一次。只处理从“未完成”到
    // “完成/失败”的跳变，避免同一状态的事件反复触发刷新（requestServerSearch 里
    // 还会校验 searchSeq 和 query，用户改了关键词就放弃这次刷新）。
    const done = !!next && (!!next.complete || !!next.error);
    const wasDone = !!prev && (!!prev.complete || !!prev.error);
    // 索引每前进一批，本地库里的匹配就多一批：搜索开着的时候要跟着刷新，
    // 否则用户会一直看到点开搜索那一刻的旧结果。相同进度的事件不重复请求。
    const changed = !!next && (!prev
      || prev.indexed !== next.indexed
      || prev.bodyIndexed !== next.bodyIndexed
      || prev.total !== next.total
      || !!prev.complete !== !!next.complete
      || String(prev.error || "") !== String(next.error || ""));
    if (inView && S.searching && (changed || (done && !wasDone))) scheduleSearchRefresh();
    // 不在搜索时：后台索引刚跑完，本地库里的已读/星标和新邮件已经是最新的，
    // 重新读一遍列表即可；读本地索引不碰网络，也不会再触发索引任务
    if (inView && !S.searching && done && !wasDone) loadList(a, folder, { force: true });
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
      const repair = [...(document.querySelectorAll?.('.account-repair-form') || [])].find(form => form.dataset.accountRepair === a);
      if (repair?.dataset.awaitConnection === 'true') {
        const status = $('.connection-status', repair);
        status.textContent = ev.state === 'online' ? '收信连接已恢复；发信状态请用“检测收发信”确认。' : ev.state === 'connecting' ? '设置已保存，正在重新连接…' : `设置已保存，连接仍失败：${ev.detail || ev.state}`;
        status.hidden = false;
      }
      if (ev.state === "online" && !S.folders[a]) loadFolders(a);
      break;
    case "inbox": {
      // 首屏只有最新 100 封（服务器分页的第一页）：保留已经从本地索引导入的
      // 更早邮件，重连后不要把用户翻出来的列表打回第一页
      const k = key(a, "INBOX");
      const fresh = keepPresent(ev.messages);
      const current = S.lists.get(k) || [];
      const minUid = fresh.length ? Math.min(...fresh.map((m) => m.uid)) : Infinity;
      const started = new Set(fresh.map((m) => m.uid));
      const older = current.filter((m) => m.uid < minUid && !started.has(m.uid));
      S.lists.set(k, fresh.concat(older));
      loadPreviews(a, "INBOX", fresh);
      break;
    }
    case "newMail": {
      if (S.thread) S.thread.stale = true;
      const list = S.lists.get(key(a, "INBOX")) || [];
      const fresh = keepPresent(ev.messages);
      const freshUids = new Set(fresh.map((m) => m.uid));
      // POP3 的序号可能在删除后重用；同 UID 的新摘要应替换旧行。
      S.lists.set(key(a, "INBOX"), fresh.concat(list.filter((m) => !freshUids.has(m.uid))));
      // 已加载的聚合视图也收到新邮件
      for (const [view, match] of [["__unread", (m) => !m.seen], ["__starred", (m) => m.flagged]]) {
        const k = key(a, view);
        if (!S.lists.has(k)) continue;
        const add = fresh.filter(match);
        const uids = new Set(add.map((m) => m.uid));
        S.lists.set(k, add.map((m) => ({ ...m })).concat(S.lists.get(k).filter((m) => !(m.folder === "INBOX" && uids.has(m.uid)))));
      }
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
      // 搜索结果里的同一封邮件也同步已读和星标
      for (const m of S.results.get(key(a, "INBOX"))?.items || []) {
        const f = byUid.get(m.uid);
        if (f) Object.assign(m, { seen: f.seen, flagged: f.flagged });
      }
      // 聚合视图里收件箱的邮件同样同步；在别处删除的移出
      for (const view of ["__unread", "__starred"]) {
        const k = key(a, view);
        const virtual = S.lists.get(k);
        if (!virtual) continue;
        S.lists.set(k, virtual.filter((m) => {
          if (m.folder !== "INBOX") return true;
          const f = byUid.get(m.uid);
          if (f) Object.assign(m, { seen: f.seen, flagged: f.flagged });
          return f || m.uid < ev.minUid || isSel(m);
        }));
      }
      break;
    }
    case "unread":
      live.unread = ev.count;
      updateBadge();
      break;
    case "ruleError": {
      // 目标文件夹被删了时每封新邮件都会失败一次；十分钟内同样的错误只提示一次
      const text = `${account(ev.accountId)?.email || ""}：${ev.detail}`;
      const last = S.ruleErrors.get(text);
      if (last && Date.now() - last < 10 * 60 * 1000) return;
      S.ruleErrors.set(text, Date.now());
      toast(text, true);
      return;
    }
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
    ? `${accountBadge(null)}<div class="meta"><div class="name">所有账户</div><div class="mail">${S.accounts.length} 个邮箱</div></div>`
    : `${accountBadge(a)}<div class="meta"><div class="name">${esc(a.name)}</div><div class="mail">${esc(a.email)}</div></div>`;

  const unread = all
    ? S.accounts.reduce((n, x) => n + (S.live[x.id]?.unread || 0), 0)
    : S.live[S.acct]?.unread || 0;

  let nav = navItem("收件箱", "inbox", S.folder === "INBOX", 'data-go="INBOX"', unread || "");
  // 所有账户下显示全部聚合视图；单个账号的草稿、已发送就是它自己的文件夹
  for (const v of window.MailBeaconViews.VIEWS) {
    if (!all && (v.key === "__drafts" || v.key === "__sent")) continue;
    // POP3 没有服务器端星标
    if (!all && v.key === "__starred" && a?.receiveProtocol === "pop3") continue;
    const label = !all && v.key === "__unread" ? "未读" : v.label;
    nav += navItem(label, v.icon, S.folder === v.key, `data-go="${v.key}"`, v.key === "__unread" ? unread || "" : "");
  }
  // 有定时邮件时才显示「定时发送」
  const scheduled = S.outbox.filter((i) => all || i.accountId === S.acct).length;
  if (scheduled || S.folder === "__outbox") nav += navItem("定时发送", "clock", S.folder === "__outbox", 'data-go="__outbox"', scheduled || "");
  nav += navItem("发送记录", "sent", false, 'data-action="send-records"');
  let more = "";
  if (all) {
    more = `<div class="side-label">邮箱</div>` + S.accounts.map((x) =>
      `<button class="nav-item account-nav" data-switch="${esc(x.id)}" title="${esc(x.email)}">${accountBadge(x, "sm")}<span class="label">${esc(x.email)}</span>${S.live[x.id]?.unread ? `<span class="count">${S.live[x.id].unread}</span>` : ""}</button>`).join("");
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
    <div class="side-top" data-tauri-drag-region><button class="icon-btn" data-action="toggle-sidebar" title="收起侧栏" aria-label="收起侧栏" aria-pressed="false">${icon("sidebar")}</button></div>
    <div class="side-scroll">
      <button class="account-card" data-action="account-menu">${card}${icon("updown", "chev")}</button>
      <button class="compose-btn" data-action="compose">${icon("pen")}写邮件</button>
      ${navItem("搜索", "search", false, 'data-action="search"')}
      ${navItem("通讯录", "users", false, 'data-action="contacts"')}
      ${navItem("收信规则", "filter", false, 'data-action="rules"')}
      <div class="side-sep"></div>
      <div class="side-label">邮件</div>
      ${nav}
      ${more}
    </div>
    <div class="side-bottom">
      ${navItem("设置", "sliders", false, 'data-action="settings"')}
      <div class="side-foot" ${sync.cls === "error" || sync.cls === "offline" ? 'role="button" tabindex="0" data-action="repair-account" aria-label="修复账户连接"' : ''} title="${esc([sync.text, ...S.accounts.map((x) => `${x.email}：${S.live[x.id]?.detail || S.live[x.id]?.state || ""}`)].join("\n"))}">
        <span class="dot ${sync.cls}"></span><span class="sync-text">${esc(sync.text)}</span>
      </div>
    </div>`;
}

/** 列表顶部的筛选：全部、未读、红旗、附件、@我（只在已加载的邮件里筛选）。 */
function filterBar() {
  return `<div class="filter-bar" role="toolbar" aria-label="筛选">${window.MailBeaconViews.FILTERS.map((f) =>
    `<button class="filter-chip ${S.filter === f.key ? "on" : ""}" data-action="filter" data-filter="${f.key}" aria-pressed="${S.filter === f.key}">${f.label}</button>`).join("")}</div>`;
}

function folderTitle() {
  if (S.folder === "INBOX") return S.acct === "all" ? "所有收件箱" : "收件箱";
  if (window.MailBeaconViews.isVirtual(S.folder)) return window.MailBeaconViews.label(S.folder);
  if (S.folder === "__outbox") return "定时发送";
  return folderInfo(S.acct, S.folder)?.name || S.folder;
}

// ---------------------------------------------------------------------------
// 布局：列表宽度可拖动，侧栏可收起（收起后鼠标移到左边缘临时呼出）
// ---------------------------------------------------------------------------
const UI_STORE = "mp.ui";
const LIST_MIN_W = 280;
const LIST_DEFAULT_W = 400;

function clampListWidth(width) {
  const max = Math.max(LIST_MIN_W, (window.innerWidth || 1200) - 520);
  return Math.round(Math.max(LIST_MIN_W, Math.min(width, max)));
}

function loadUiPrefs() {
  try {
    const saved = JSON.parse(localStorage.getItem(UI_STORE) || "{}");
    return {
      listWidth: clampListWidth(Number(saved.listWidth) || LIST_DEFAULT_W),
      collapsed: !!saved.collapsed,
    };
  } catch {
    return { listWidth: LIST_DEFAULT_W, collapsed: false };
  }
}

function saveUiPrefs() {
  try {
    localStorage.setItem(UI_STORE, JSON.stringify({ listWidth: S.ui.listWidth, collapsed: S.ui.collapsed }));
  } catch {}
}

/** 把宽度和收起状态应用到三栏网格上。 */
function applyLayout() {
  const app = document.querySelector(".app");
  if (!app) return;
  app.style.setProperty("--list-w", `${clampListWidth(S.ui.listWidth)}px`);
  app.classList.toggle("rail-collapsed", S.ui.collapsed);
  if (!S.ui.collapsed) app.classList.remove("rail-peek");
  document.body.classList.toggle("rail-collapsed", !!S.ui.collapsed);
}

function setRailPeek(on) {
  if (!S.ui.collapsed) return;
  document.querySelector(".app")?.classList.toggle("rail-peek", !!on);
}

function toggleSidebar() {
  S.ui.collapsed = !S.ui.collapsed;
  applyLayout();
  saveUiPrefs();
  renderList();
}

/** 拖动列表右边缘调整宽度；双击恢复默认。 */
function startListResize(e) {
  if (e.button != null && e.button !== 0) return;
  // 有弹层（设置、收信规则等）盖在上面时，不能去拖被遮住的列表分隔条
  if ($("#overlay")?.childElementCount) return;
  e.preventDefault();
  const handle = e.currentTarget;
  const startX = e.clientX;
  const startWidth = S.ui.listWidth;
  handle.classList.add("dragging");
  document.body.classList.add("resizing-list");
  const move = (ev) => {
    S.ui.listWidth = clampListWidth(startWidth + ev.clientX - startX);
    applyLayout();
  };
  const up = () => {
    handle.classList.remove("dragging");
    document.body.classList.remove("resizing-list");
    window.removeEventListener("pointermove", move);
    saveUiPrefs();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up, { once: true });
}

function resetListWidth() {
  S.ui.listWidth = LIST_DEFAULT_W;
  applyLayout();
  saveUiPrefs();
}

/** 列表里的一行（会话模式下是一整个会话）。 */
function rowHtml(g, grouped, multi) {
  const m = g.latest;
  const sentView = S.folder !== "INBOX" && ["sent", "outbox"].includes(folderInfo(m.accountId, m.folder)?.role || (m.folder === "__outbox" ? "outbox" : ""));
  const who = sentView ? (m.to ? `发给 ${m.to}` : "(无收件人)")
    : g.count > 1 ? (g.participants.length > 3 ? `${g.participants.slice(0, 3).join("、")} 等` : g.participants.join("、"))
    : (m.fromName || m.fromAddr || "(未知发件人)");
  const prev = S.previews.get(key(key(m.accountId, m.folder), m.uid)) || "";
  const unread = g.unread > 0, sel = grouped ? groupSelected(g) : isSel(m);
  const checked = MS().state(g,S.batch);
  return `<div class="mail-row ${checked!=="none"?"batch-selected":""}"><button class="mail-checkbox" role="checkbox" aria-checked="${checked==="mixed"?"mixed":checked==="all"}" aria-label="选择 ${esc(m.subject||"无主题")}" data-select-mail="${esc(MS().key(m))}">${checked==="all"?"✓":checked==="mixed"?"−":""}</button><button class="row ${unread ? "unread" : ""} ${sel ? "sel" : ""}" data-open="${esc(m.accountId)}|${m.uid}|${esc(m.folder)}"${g.count > 1 ? ` aria-label="${esc(`${who}，${m.subject || "(无主题)"}，会话共 ${g.count} 封${unread ? `，${g.unread} 封未读` : ""}`)}"` : ""}>
    ${unread ? '<span class="unread-dot"></span>' : ""}
    <div class="l1"><span class="from">${esc(who)}${g.count > 1 ? `<span class="thread-count" aria-hidden="true">${g.count}</span>` : ""}</span><span class="date">
      ${g.flagged ? icon("star", "star") : ""}${g.attachment ? icon("clip") : ""}
      ${multi ? `<span class="acct-dot" style="background:${accountColor(account(m.accountId))}" title="${esc(account(m.accountId)?.email || "")}"></span>` : ""}${esc(fmtDate(m.date))}</span></div>
    <div class="l2"><span class="subj">${esc(m.subject || "(无主题)")}</span><span class="prev">${esc(prev)}</span></div>
  </button></div>`;
}

function renderList() {
  const items = currentList();
  const scope = JSON.stringify([S.acct,S.folder,S.query,S.filter,S.threads]);
  if (S.batchScope !== scope) { clearBatch(); S.batchScope=scope; }
  S.batch=MS().retain(S.batch,selectionGroups());
  const k = key(S.acct, S.folder);
  const loading = S.acct === "all" ? S.accounts.some((a) => S.loading.has(key(a.id, S.folder))) : S.loading.has(k);
  const waitingInbox = S.folder === "INBOX" && (S.acct === "all"
    ? S.accounts.every((a) => !S.lists.has(key(a.id, "INBOX")))
    : !S.lists.has(key(S.acct, "INBOX")));
  const multi = S.accounts.length > 1 && S.acct === "all";

  const activeQuery = document.activeElement?.id === "q" ? document.activeElement.value : null;
  const queryValue = activeQuery ?? S.query;
  const head = S.searching
    ? `<div class="search-box">${icon("search")}<input id="q" placeholder="搜索发件人、主题、内容" value="${esc(queryValue)}" autocomplete="off" spellcheck="false"></div>
       <button class="icon-btn" data-action="search-close" title="取消">${icon("x")}</button>`
    : `<h1 data-tauri-drag-region>${esc(folderTitle())}</h1>
       <button class="icon-btn${S.checking ? " spin" : ""}" data-action="check-mail" title="收取新邮件 (⇧⌘N)"${S.checking ? " disabled" : ""}>${icon("refresh")}</button>
       <button class="icon-btn" data-action="list-menu" title="更多">${icon("more")}</button>`;
  const filters = S.searching ? "" : filterBar();

  // 记住原来的滚动位置和可视高度，长列表按它算出这次要渲染的行
  const list = $("#list");
  const oldRows = $(".rows", list);
  const scroll = oldRows ? oldRows.scrollTop : 0;
  const viewHeight = oldRows ? oldRows.clientHeight : 600;

  let body = "";
  let virtual = false;
  if (!items.length && (loading || waitingInbox)) {
    body = Array.from({ length: 8 }, () => `<div class="skeleton"><i style="width:${40 + Math.random() * 25}%"></i><i style="width:${70 + Math.random() * 25}%"></i></div>`).join("");
  } else if (!items.length) {
    const noFolders = window.MailBeaconViews.needsFolders(S.folder) &&
      (S.acct === "all" ? S.accounts : [account(S.acct)].filter(Boolean)).some((a) => !S.folders[a.id]);
    body = `<div class="list-empty">${S.query ? "没有找到匹配的邮件" : noFolders ? "还没读到邮箱的文件夹列表，连接后会自动显示" : S.filter !== "all" ? "没有符合筛选的邮件" : "这里没有邮件"}</div>`;
  } else {
    const grouped = listGroups();
    const units = grouped || items.map((m) => ({ latest: m, items: [m], count: 1, unread: m.seen ? 0 : 1, flagged: m.flagged, attachment: m.attachment }));
    virtual = units.length > VIRTUAL_MIN;
    if (virtual) {
      // 几千封只渲染可见的一段，上下空白占位撑出滚动条；滚动时 updateListWindow 平移
      const { start, end } = listWindowRange(units.length, scroll, viewHeight || 600);
      listWindow = { units, grouped: !!grouped, multi, rowsEl: null, start, end };
      body = `<div class="rows-top" style="height:${start * ROW_H}px"></div>
        <div class="rows-window">${units.slice(start, end).map((g) => rowHtml(g, !!grouped, multi)).join("")}</div>
        <div class="rows-bottom" style="height:${Math.max(0, units.length - end) * ROW_H}px"></div>`;
    } else {
      listWindow = null;
      body = units.map((g) => rowHtml(g, !!grouped, multi)).join("");
    }
    const canMore = S.acct !== "all" && !window.MailBeaconViews.isVirtual(S.folder) && !S.query && !S.exhausted.has(k)
      && (S.morePages.get(k) === true || items.length >= 100);
    if (canMore) body += `<button class="list-more" data-action="more">${loading ? "正在加载…" : "加载更早的邮件"}</button>`;
  }

  const focused = document.activeElement;
  const hadFocus = focused?.id === "q";
  const listFocused = focused?.closest?.("#list") === list;
  const focusedData = listFocused ? { ...focused.dataset } : null;
  const caret = hadFocus ? focused.selectionStart : 0;
  list.innerHTML = `<div class="list-head" data-tauri-drag-region>${head}</div>${filters}${S.searching || S.query ? searchNotice() : ""}${bulkBar()}<div class="rows" tabindex="-1">${body}</div>`;
  const rowsEl = $(".rows", list);
  rowsEl.scrollTop = scroll;
  if (virtual && listWindow) {
    listWindow.rowsEl = rowsEl;
    rowsEl.addEventListener("scroll", onListScroll, { passive: true });
    loadWindowPreviews(listWindow);
  }
  if (hadFocus) {
    const q = $("#q");
    q.focus();
    q.value = queryValue;
    q.setSelectionRange(caret, caret);
  } else if (listFocused) {
    let replacement;
    for (const [field, selector] of [["selectMail", "[data-select-mail]"], ["open", "[data-open]"], ["action", "[data-action]"]]) {
      if (!focusedData[field]) continue;
      replacement = [...list.querySelectorAll(selector)].find(node => node.dataset[field] === focusedData[field] && !node.disabled);
      break;
    }
    (replacement || rowsEl)?.focus({ preventScroll: true });
  }
}

// ---------------------------------------------------------------------------
// 已读回执、定时发送
// ---------------------------------------------------------------------------
const receiptKey = (m, v) => `${messageKey(m)}|${v?.messageId || ""}`;

function loadReceiptPrefs() {
  try { for (const k of JSON.parse(localStorage.getItem("mb.receipts") || "[]")) S.receiptsHandled.add(k); } catch {}
}
function saveReceiptPrefs() {
  try { localStorage.setItem("mb.receipts", JSON.stringify([...S.receiptsHandled].slice(-500))); } catch {}
}

/** 发件人请求了已读回执：问用户要不要发，绝不自动发送。 */
function receiptBar(v, m, role) {
  if (!X().offerReceipt(v, role, account(m.accountId)?.email, S.receiptsHandled.has(receiptKey(m, v)))) return "";
  return `<div class="img-bar receipt-bar" role="group" aria-label="已读回执">发件人请求已读回执${esc(X().receiptNote(v))}
    <button data-action="receipt-send">发送回执</button><button data-action="receipt-ignore" style="background:none;padding:0">不发送</button></div>`;
}

async function answerReceipt(send) {
  const m = findMsg(S.sel), v = S.msg;
  if (!m || !v || S.receiptBusy) return;
  const k = receiptKey(m, v);
  S.receiptBusy = true;
  for (const b of document.querySelectorAll(".receipt-bar button")) b.disabled = true;
  try {
    await invoke(send ? "send_receipt" : "ignore_receipt", { accountId: m.accountId, folder: m.folder, uid: m.uid });
    S.receiptsHandled.add(k);
    saveReceiptPrefs();
    if (send) toast("已发送已读回执");
  } catch (e) {
    fail(e);
  } finally {
    S.receiptBusy = false;
  }
  renderReader();
}

const outboxItem = (m) => S.outbox.find((i) => i.accountId === m.accountId && i.id === m.uid);

function outboxBar(m, role) {
  if (role !== "outbox") return "";
  const item = outboxItem(m);
  if (!item) return "";
  const when = X().fmtSendAt(item.sendAt);
  const problem = item.lastError
    ? `<div class="outbox-error">发送需要处理：${esc(item.lastError)}${item.attempts >= 5 ? '（已停止自动重试）<button class="secondary" data-action="send-records">查看发送记录</button>' : "（稍后重试）"}</div>` : "";
  const editing = S.outboxEditing === `${m.accountId}|${m.uid}`;
  return `<div class="img-bar outbox-bar">${icon("clock")}<span>将于 ${esc(when)} 发送${item.sendAt < Date.now() && !item.lastError ? "（MailBeacon 运行时到点发出）" : ""}</span>
    ${editing ? `<input type="datetime-local" id="outbox-send-at" value="${X().toLocalInput(item.sendAt)}" aria-label="新的发送时间"><button data-action="outbox-reschedule-ok">确定</button><button data-action="outbox-reschedule-cancel" style="background:none;padding:0">取消</button>` : ""}
    ${problem}</div>`;
}

async function refreshOutbox() {
  try { S.outbox = await invoke("outbox_list", {}); } catch (e) { console.warn("读取定时发送失败", e); }
}

/** 后台发出、取消或改了时间：更新文件夹（「定时发送」只在有邮件时出现）和列表。 */
async function onOutboxEvent(ev) {
  await refreshOutbox();
  if (ev.state === "sent") toast(`定时邮件「${ev.detail || "(无主题)"}」已发出`);
  if (ev.state === "failed") toast(`定时发送需要处理：${ev.detail}`, true);
  if ((ev.state === "sent" || ev.state === "cancelled") && S.sel?.folder === "__outbox" && S.sel.accountId === ev.accountId && S.sel.uid === ev.id) {
    S.sel = null;
    S.msg = null;
  }
  await loadFolders(ev.accountId);
  if (S.folder === "__outbox" && (S.acct === ev.accountId || S.acct === "all")) await loadList(ev.accountId, "__outbox", { force: true });
  if (S.folder === "__outbox" && !S.outbox.some((i) => S.acct === "all" || i.accountId === S.acct)) S.folder = "INBOX";
  render();
}

async function outboxAction(action) {
  const m = findMsg(S.sel);
  if (!m) return;
  const where = { accountId: m.accountId, id: m.uid };
  try {
    if (action === "send-now") {
      await invoke("outbox_send_now", where);
    } else if (action === "cancel") {
      if (!confirm("取消定时发送？邮件会存进草稿，可以继续编辑。")) return;
      const draft = await invoke("outbox_cancel", where);
      toast(draft?.folder === "__local_drafts" ? "已取消定时，邮件已存进本地草稿" : "已取消定时，邮件已存进草稿");
      loadFolders(m.accountId);
    } else if (action === "reschedule-ok") {
      const at = X().fromLocalInput($("#outbox-send-at")?.value);
      if (Number.isNaN(at)) return toast("请选择发送时间", true);
      await invoke("outbox_reschedule", { ...where, sendAt: at });
      S.outboxEditing = null;
      toast(`改为 ${X().fmtSendAt(at)} 发送`);
    }
  } catch (e) {
    fail(e);
  }
  await refreshOutbox();
  renderReader();
}

/** 收件人/抄送里的一个名字：点开能看完整地址、复制或直接写邮件。 */
function recipientChips(list, me) {
  return list.map((p) => {
    const self = Boolean(me) && p.addr.toLowerCase() === me;
    const label = self ? "我" : (p.name || p.addr);
    const full = p.name ? `${p.name} <${p.addr}>` : p.addr;
    return `<button class="person" data-action="show-person" data-name="${esc(p.name)}" data-addr="${esc(p.addr)}" title="${esc(full)}">${esc(label)}</button>`;
  }).join("、");
}

let readerHintTimer = null;
let readerHintGeneration = 0;
let readerHintOpening = null;
let readerHintOwner = null;
let readerEntry = null;
let readerScrollMotion = null;

function prefersReducedMotion() {
  return !!window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
}

function getReaderScrollMotion() {
  return readerScrollMotion ||= window.MailBeaconReaderMotion.createScrollMotion({
    requestFrame: callback => window.requestAnimationFrame(callback),
    cancelFrame: id => window.cancelAnimationFrame(id),
    now: () => Date.now(),
    reducedMotion: prefersReducedMotion,
  });
}

function cancelReaderScroll() { readerScrollMotion?.cancel(); }

function watchReaderScrollInput(doc) {
  for (const name of ["wheel", "touchstart", "pointerdown"]) {
    doc.addEventListener(name, cancelReaderScroll, { capture: true, passive: true });
  }
}

function applyReaderEntry(element, startedAt, duration) {
  const elapsed = Math.max(0, Date.now() - startedAt);
  if (!element || elapsed >= duration || prefersReducedMotion()) return;
  element.classList.add("is-entering");
  // 正文下载、星标等会重建节点：沿用原来的动画进度，避免同一封信反复闪入。
  element.style.animationDelay = `${-elapsed}ms`;
}

function hideReaderHint({ immediate = false } = {}) {
  S.readerHintUntil = 0;
  readerHintOpening = null;
  clearTimeout(readerHintTimer);
  readerHintTimer = null;
  const generation = ++readerHintGeneration;
  const hint = $(".reader-hint", $("#reader") || document);
  if (!hint) return;
  if (immediate) { hint.remove(); return; }
  hint.classList.remove("is-entering");
  hint.classList.add("is-hiding");
  readerHintTimer = setTimeout(() => {
    if (generation !== readerHintGeneration) return;
    hint.remove();
    readerHintTimer = null;
  }, 150);
}

function showReaderHint() {
  hideReaderHint({ immediate: true });
  if (!S.sel || !findMsg(S.sel)) return;
  readerHintOwner = { key: messageKey(S.sel), seq: openSeq, startedAt: Date.now() };
  S.readerHintUntil = Date.now() + 2000;
  const generation = readerHintGeneration;
  renderReaderHint();
  readerHintTimer = setTimeout(() => {
    if (generation === readerHintGeneration) hideReaderHint();
  }, 2000);
}

function renderReaderHint() {
  const reader = $("#reader");
  if (!reader) return;
  if (!S.sel) { hideReaderHint({ immediate: true }); return; }
  const id = messageKey(S.sel);
  // 通知可能先选中、后收到列表；等阅读区真正能呈现这封信时再开始两秒计时。
  if (readerHintOpening?.seq === openSeq && readerHintOpening.key === id && findMsg(S.sel)) {
    showReaderHint();
    return;
  }
  if (!S.readerHintUntil || S.readerHintUntil <= Date.now() ||
      readerHintOwner?.key !== id || readerHintOwner.seq !== openSeq) return;
  const archive = $('[data-action="archive"]', reader);
  const reply = $('[data-action="reply"]', reader);
  const labels = window.MailBeaconReaderNav.readerHintLabels({
    canArchive: !!archive && !archive.disabled,
    canReply: !!S.msg && !S.msg.partial && !!reply && !reply.disabled,
  });
  let hint = $(".reader-hint", reader);
  if (!hint) {
    hint = document.createElement("div");
    hint.className = "reader-hint";
    hint.setAttribute("aria-label", "阅读快捷键");
    reader.appendChild(hint);
    applyReaderEntry(hint, readerHintOwner.startedAt, 180);
  }
  const html = labels.map((x) => `<span class="reader-hint-item${x.disabled ? " is-disabled" : ""}"><kbd>${esc(x.key)}</kbd><span>${esc(x.label)}</span></span>`).join('<span class="reader-hint-sep" aria-hidden="true">·</span>');
  if (hint.innerHTML !== html) hint.innerHTML = html;
}

function renderReader() {
  const r = $("#reader");
  const m = findMsg(S.sel);
  if (!S.sel || !m) {
    cancelReaderScroll();
    r.dataset.sig = "";
    r.innerHTML = `<div class="toolbar" data-tauri-drag-region></div>
      <div class="reader-empty"><div>${icon("mail", "big")}<div>没有选中邮件</div></div></div>`;
    if (!S.sel || readerHintOpening?.seq !== openSeq || readerHintOpening.key !== messageKey(S.sel)) {
      hideReaderHint({ immediate: true });
    }
    return;
  }
  const { items, index: idx } = listEntries();
  const role = folderInfo(m.accountId, m.folder)?.role || (m.folder === "INBOX" ? "inbox" : "other");
  const hasArchive = folderByRole(m.accountId, "archive") || folderByRole(m.accountId, "all");
  const v = S.msg;
  const moving = S.moving.has(messageKey(m));
  const canArchive = !moving && !!hasArchive && role !== "archive" && role !== "all";
  // 先出的文字版：回复、转发、更多菜单等要等整封原文
  const wait = v?.partial ? " disabled" : "";

  const toolbar = `<div class="toolbar" data-tauri-drag-region>
      <button class="tb-btn plain" data-action="close" title="关闭 (Esc)">${icon("x")}</button>
      <div class="vsep"></div>
      <button class="tb-btn" data-action="prev" title="上一封 (K)" ${idx <= 0 ? "disabled" : ""}>${icon("left")}</button>
      <button class="tb-btn" data-action="next" title="下一封 (J)" ${idx < 0 || idx >= items.length - 1 ? "disabled" : ""}>${icon("right")}</button>
      <div class="spacer" data-tauri-drag-region></div>
      ${role === "drafts" || role === "local-drafts" ? `<button class="tb-btn" data-action="edit-draft" title="继续编辑" ${v && !v.partial ? "" : "disabled"}>${icon("edit")}</button>` : ""}
      ${role === "outbox" ? `<button class="tb-btn" data-action="outbox-send-now" title="立即发送">${icon("sent")}</button>
      <button class="tb-btn" data-action="outbox-reschedule" title="修改发送时间">${icon("clock")}</button>
      <button class="tb-btn" data-action="outbox-cancel" title="取消定时（转为草稿）">${icon("trash")}</button>`
      : role === "local-drafts" ? `<button class="tb-btn" data-action="delete-local-draft" title="删除草稿">${icon("trash")}</button>` : `<button class="tb-btn" data-action="reply" title="回复 (R)"${wait}>${icon("reply")}</button>
      <button class="tb-btn" data-action="reply-all" title="全部回复 (A)"${wait}>${icon("replyAll")}</button>
      <button class="tb-btn" data-action="forward" title="转发 (F)"${wait}>${icon("forward")}</button>
      <div class="vsep"></div>
      <button class="tb-btn" data-action="junk" ${moving ? "disabled" : ""} title="${role === "junk" ? "不是垃圾邮件" : "标为垃圾邮件"}">${icon("junk")}</button>
      <button class="tb-btn" data-action="move-menu" ${moving ? "disabled" : ""} title="移动到…">${icon("folder")}</button>
      <button class="tb-btn" data-action="archive" title="归档 (E)" ${canArchive ? "" : "disabled"}>${icon("archive")}</button>
      <button class="tb-btn" data-action="delete" ${moving ? "disabled" : ""} title="${role === "trash" ? "彻底删除" : "删除 (⌫)"}">${icon("trash")}</button>
      <button class="tb-btn" data-action="msg-menu" title="更多"${wait}>${icon("more")}</button>`}
    </div>`;

  const subject = v?.subject || m.subject || "(无主题)";
  let card;
  if (S.msgError) {
    card = `<div class="mail-card"><div class="list-empty">${esc(S.msgError)}<br><br><button class="secondary" data-action="retry">重试</button></div></div>`;
  } else if (!v) {
    card = `<div class="mail-card"><div class="mail-head">${avatar(m.fromName || m.fromAddr, m.fromAddr)}
      <div class="who"><div class="line1"><span class="name">${esc(m.fromName || m.fromAddr)}</span><span class="addr">${esc(m.fromAddr)}</span></div></div>
      <div class="when">${esc(fmtDate(m.date, true))}</div></div>
      <div class="mail-body"><div class="dl-progress" role="status">${esc(progressText(S.msgProgress, S.msgTotal))}</div><div class="skeleton" style="border:0;padding:0 26px 24px"><i style="width:80%"></i><i style="width:92%"></i><i style="width:64%"></i></div></div></div>`;
  } else {
    const me = account(m.accountId)?.email.toLowerCase();
    const toLine = [
      v.to.length ? `收件人：${recipientChips(v.to, me)}` : "",
      v.cc.length ? `抄送：${recipientChips(v.cc, me)}` : "",
      v.bcc?.length ? `密送：${recipientChips(v.bcc, me)}` : "",
    ].filter(Boolean).join("　");
    const imgKey = key(key(m.accountId, m.folder), m.uid);
    const showImg = S.showImages.has(imgKey) || S.showImages.has("from:" + v.from.addr.toLowerCase());
    const imgBar = (v.html && v.hasRemote && !showImg
      ? `<div class="img-bar"><button data-action="show-images">显示图片</button><button data-action="always-images" style="background:none;padding:0">始终显示此发件人的图片</button></div>` : "")
      + (v.partial ? "" : receiptBar(v, m, role)) + outboxBar(m, role)
      + (v.partial ? `<div class="partial-bar" role="status">${icon("clock")}<span>图片和附件还在下载</span><span class="dl-progress">${esc(progressText(S.msgProgress, S.msgTotal ?? v.size))}</span></div>` : "");
    const atts = v.attachments.length
      ? `<div class="atts">${v.attachments.map((x) => `<button class="att" data-att="${x.index}" data-attachment-source="${esc(JSON.stringify({ ...selectionOf(m), uidValidity: v.uidValidity, index: x.index }))}" title="双击打开，右键显示更多">${icon("file")}<span class="n">${esc(x.name)}</span><span class="s">${fmtSize(x.size)}</span></button>`).join("")}</div>` : "";
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
    ${role === "outbox" ? "" : `<div class="reply-bar">
      <button class="pill" data-action="reply"${wait}>${icon("reply")}回复</button>
      <button class="pill" data-action="reply-all"${wait}>${icon("replyAll")}全部回复</button>
      <button class="pill" data-action="forward"${wait}>${icon("forward")}转发</button>
    </div>`}`;
  }

  // 正文 iframe 已经在显示同一封邮件时不重建，避免闪烁和滚动位置丢失
  const strip = threadStrip(m);
  const extra = v ? `${v.partial ? "" : receiptBar(v, m, role)}${outboxBar(m, role)}` : "";
  const sig = `${extra}|${key(key(m.accountId, m.folder), m.uid)}|${v ? (v.partial ? "p" : "v") : ""}|${S.msgError}|${S.showImages.size}|${m.flagged}|${idx}|${items.length}|${role}|${moving}|${strip}`;
  if (r.dataset.sig === sig) {
    // 文件夹可能在正文之后才加载完成；更新按钮状态时保留正文和滚动位置。
    const archiveButton = $('[data-action="archive"]', r);
    if (archiveButton) archiveButton.disabled = !canArchive;
    renderReaderHint();
    return;
  }
  cancelReaderScroll();
  r.dataset.sig = sig;
  r.innerHTML = `${toolbar}<div class="read-scroll">
    <div class="subject-line"><h2>${esc(subject)}</h2>
      <button class="star-btn ${m.flagged ? "on" : ""}" data-action="star" title="星标 (S)">${icon("star")}</button></div>
    ${strip}${card}</div>`;
  if (readerEntry?.seq === openSeq && readerEntry.key === messageKey(m)) {
    readerEntry.startedAt ??= Date.now();
    applyReaderEntry($(".read-scroll", r), readerEntry.startedAt, 200);
  }
  if (v?.html) mountFrame(v, m);
  renderReaderHint();
}

function linkify(text) {
  return esc(text).replace(/((?:https?:\/\/|mailto:|tel:)[^\s<>"']+[^\s<>"'.,;:!?)\]）。，])/gi, '<a href="$1" data-link="$1">$1</a>');
}

function fitMailFrame(frame, doc) {
  // A mail using vh/% plus padding can otherwise grow its iframe on every
  // ResizeObserver notification. Long mail stays available via iframe scrolling.
  const height = Math.min(8000, Math.max(120, doc.documentElement.scrollHeight));
  const value = `${height}px`;
  if (frame.style.height !== value) frame.style.height = value;
}

function keepMailFragment(e, link) {
  const href = link.getAttribute?.("href");
  if (!href?.startsWith("#")) return false;
  e.preventDefault();
  let id = href.slice(1);
  try { id = decodeURIComponent(id); } catch {}
  link.ownerDocument?.getElementById?.(id)?.scrollIntoView?.({ block: "nearest" });
  return true;
}


function openMailImage(e) {
  const img = e.target.closest?.("img");
  if (!img) return;
  const src = (img.currentSrc || img.src || "").trim();
  if (!src) return;
  e.preventDefault();
  invoke("open_inline_image", { src }).catch(fail);
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
    img{max-width:100%;height:auto}pre{white-space:pre-wrap}a{color:#2f6fec}${v.partial ? 'img[src^="cid:"]{visibility:hidden}' : ""}</style></head><body>${v.html}</body></html>`;
  frame.onload = () => {
    const doc = frame.contentDocument;
    if (!doc) return;
    const fit = () => fitMailFrame(frame, doc);
    fit();
    new ResizeObserver(fit).observe(doc.documentElement);
    // 邮件自带的 target（_self/_top 等）会让 iframe 原地跳转或被沙箱拦下，统一改成新窗口，
    // 由后端的新窗口处理交给系统浏览器。页内锚点（#xx）不能改成 _self：srcdoc 的基准地址是
    // 应用自己的页面，会把应用首页加载进正文框；保持 _blank 被后端拒绝，等于点了不动
    // （能触发点击监听的平台上由 keepMailFragment 负责滚动）
    for (const a of doc.querySelectorAll("a[href], area[href]")) a.target = "_blank";
    doc.addEventListener("click", (e) => {
      const a = e.target.closest && e.target.closest("a[href]");
      if (!a) return;
      // 邮件内的片段链接留在隔离 iframe 内滚动，外部链接才交给系统打开器。
      if (keepMailFragment(e, a)) return;
      e.preventDefault();
      invoke("open_link", { url: a.href }).catch(fail);
    });
    doc.addEventListener("dblclick", openMailImage);
    doc.addEventListener("keydown", (e) => onKey(e));
    watchReaderScrollInput(doc);
  };
}

// ---------------------------------------------------------------------------
// 操作
// ---------------------------------------------------------------------------
let openSeq = 0;

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------
const TH = () => window.MailBeaconThreads;
const isMine = (m) => S.accounts.some((a) => a.email.toLowerCase() === String(m.fromAddr || "").toLowerCase());
/** 会话模式下当前列表的分组；不是会话模式时为 null。 */
const listGroups = () => (S.threads ? TH().groupThreads(currentList(), isMine) : null);
/** 这一行是不是正在看的会话（看的可能是已发送里自己的回复，不在列表里）。按邮件位置比较：列表刷新后是新对象。 */
const inThread = (x) => !!S.thread?.items.some((y) => sameMsg(y, x.accountId, x.folder, x.uid));
const groupSelected = (g) => g.items.some(isSel) || (!!S.thread?.items.some(isSel) && g.items.some(inThread));
/** 向服务器查到的会话一分钟内有效；期间删除、移动、收到新邮件或发信会让它失效。 */
const THREAD_TTL = 60 * 1000;

function loadThreadPrefs() {
  try { S.threads = localStorage.getItem("mb.threads") === "1"; } catch {}
}

function toggleThreads() {
  S.threads = !S.threads;
  try { localStorage.setItem("mb.threads", S.threads ? "1" : "0"); } catch {}
  if (S.threads && S.sel) loadThread(S.sel);
  if (!S.threads) S.thread = null;
  render();
}

/** 选中的邮件所在的会话（只看已加载的列表）。 */
function listedConversation(sel) {
  const g = TH().groupThreads(currentList(), isMine).find((x) => x.items.some((m) => sameMsg(m, sel.accountId, sel.folder, sel.uid)));
  return g ? g.items : [];
}

const threadHas = (sel) => !!S.thread?.items.some((m) => sameMsg(m, sel.accountId, sel.folder, sel.uid));

/** 打开邮件时先用列表里已有的同一会话邮件，正文出来后再向服务器查完整的会话。 */
function showListedThread(sel) {
  // 已经在这个会话里（例如点开会话里自己的回复）：保留已有的邮件，只补上列表里新出现的，
  // 等 loadThread 再刷新；不能只用列表重建，否则不在列表里的那封就找不到了
  if (threadHas(sel)) {
    if (S.thread.stale) S.thread.items = TH().mergeConversation(S.thread.items, listedConversation(sel));
    return;
  }
  S.thread = { key: messageKey(sel), items: TH().mergeConversation([], listedConversation(sel)), at: 0, stale: true };
}

/** 向服务器要整个会话（含已发送里自己的回复）。同一会话刚查过就不再查。 */
async function loadThread(sel) {
  if (threadHas(sel) && !S.thread.stale && Date.now() - S.thread.at < THREAD_TTL) return;
  const seq = ++S.threadSeq;
  const m = findMsg(sel);
  const hint = m?.messageId ? { messageId: m.messageId, inReplyTo: m.inReplyTo || "", references: m.references || [] } : null;
  try {
    const found = await invoke("thread_messages", { accountId: sel.accountId, folder: sel.folder, uid: sel.uid, hint });
    if (seq !== S.threadSeq || !S.threads || !isSel(sel)) return;
    S.thread = { key: messageKey(sel), items: TH().mergeConversation(found, listedConversation(sel)), at: Date.now(), stale: false };
    renderReader();
  } catch (e) {
    console.warn("读取会话失败", e);
  }
}

/** 打开一个会话时，列表里这个会话其他未读的邮件也标为已读。 */
function markConversationRead(sel) {
  const unread = listedConversation(sel).filter((m) => !m.seen && !sameMsg(m, sel.accountId, sel.folder, sel.uid));
  const restore = (list) => {
    for (const m of list) patchMsg(m.accountId, m.folder, m.uid, { seen: false });
    if (list[0]?.folder === "INBOX" && S.live[list[0].accountId]) S.live[list[0].accountId].unread += list.length;
    updateBadge();
    render();
  };
  const byFolder = new Map();
  for (const m of unread) {
    const k = key(m.accountId, m.folder);
    if (!byFolder.has(k)) byFolder.set(k, []);
    byFolder.get(k).push(m);
  }
  for (const list of byFolder.values()) {
    const { accountId, folder } = list[0];
    for (const m of list) patchMsg(accountId, folder, m.uid, { seen: true });
    if (folder === "INBOX" && S.live[accountId]) S.live[accountId].unread = Math.max(0, S.live[accountId].unread - list.length);
    invoke("set_flag", { accountId, folder, uids: list.map((m) => m.uid), flag: "seen", value: true }).catch((e) => { restore(list); fail(e); });
  }
  if (unread.length) updateBadge();
}

/** 阅读区上方的会话列表；只有一封时不显示。 */
function threadStrip(m) {
  if (!S.threads || !S.thread) return "";
  const items = TH().mergeConversation(S.thread.items, listedConversation(m));
  if (items.length < 2) return "";
  return `<section class="thread" aria-label="会话">
    <div class="thread-head">此会话共 ${items.length} 封</div>
    <ol class="thread-list">${items.map((x) => {
      const cur = sameMsg(x, m.accountId, m.folder, m.uid);
      // 和列表不在同一个文件夹的标出来（例如收件箱里看到的「已发送」回复）
      const base = window.MailBeaconViews.isVirtual(S.folder)
        ? window.MailBeaconViews.sources(S.folder, S.folders[x.accountId] || []).map((src) => src.folder)
        : [S.folder];
      const where = base.includes(x.folder) ? "" : folderInfo(x.accountId, x.folder)?.name || x.folder;
      const who = isMine(x) ? "我" : x.fromName || x.fromAddr || "(未知发件人)";
      const prev = S.previews.get(key(key(x.accountId, x.folder), x.uid)) || "";
      return `<li><button class="thread-item${cur ? " cur" : ""}${x.seen ? "" : " unread"}" data-thread-open="${esc(x.accountId)}|${x.uid}|${esc(x.folder)}"${cur ? ' aria-current="true"' : ""}>
        <span class="avatar sm" aria-hidden="true" style="background:${colorOf(x.fromAddr)}">${esc(initial(who))}</span><span class="who">${esc(who)}${x.seen ? "" : '<span class="sr-only">（未读）</span>'}</span>${where ? `<span class="where">${esc(where)}</span>` : ""}
        <span class="prev">${esc(prev || x.subject || "")}</span><span class="date">${esc(fmtDate(x.date))}</span></button></li>`;
    }).join("")}</ol></section>`;
}
function fmtBytes(n) {
  return n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MB` : `${Math.round(n / 1024)} KB`;
}

/** 读信区下载提示：「正在下载 340 KB / 889 KB」 */
function progressText(received, total) {
  if (!received) return "正在下载…";
  // 收到的字节里还有命令往返和先取的正文，可能略多于整封大小
  if (total) received = Math.min(received, total);
  return `正在下载 ${fmtBytes(received)}${total ? ` / ${fmtBytes(total)}` : ""}`;
}

async function openMessage(sel) {
  clearBatch();
  cancelReaderScroll();
  hideReaderHint({ immediate: true });
  const seq = ++openSeq;
  S.sel = sel;
  readerEntry = { key: messageKey(sel), seq, startedAt: null };
  readerHintOpening = { key: messageKey(sel), seq };
  S.msg = null;
  S.msgProgress = 0;
  S.msgTotal = null;
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
  if (S.threads) {
    showListedThread(sel);
    markConversationRead(sel);
  } else S.thread = null;
  render();
  revealSelectedRow();
  const download = { accountId: sel.accountId, done: fetchMessage(sel, seq) };
  opening = download;
  try {
    await download.done;
  } finally {
    if (opening === download) opening = null;
    if (seq === openSeq) {
      S.msgProgress = 0;
      S.msgTotal = null;
    }
  }
}

async function fetchMessage(sel, seq) {
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const v = await invoke("get_message", { accountId: sel.accountId, folder: sel.folder, uid: sel.uid });
      if (seq !== openSeq) return;
      S.msg = v;
      S.msgError = "";
      // 正文先出来再查会话：会话搜索走命令连接，可能要几秒
      if (S.threads) loadThread(sel);
      prefetchNext();
      break;
    } catch (e) {
      if (seq !== openSeq) return;
      S.msgError = String(e);
    }
  }
  render();
}

/** 看信时把列表里的下一封先下到本地缓存，按 ↓ 时就不用等。 */
function prefetchNext() {
  const { items, index } = listEntries();
  const next = index >= 0 ? items[index + 1] : null;
  if (!next) return;
  invoke("prefetch_message", { accountId: next.accountId, folder: next.folder, uid: next.uid }).catch(() => {});
}

/** 上一封/下一封按列表里的行走：会话模式下一行是一个会话。 */
function listEntries() {
  const groups = listGroups();
  const items = groups ? groups.map((g) => g.latest) : currentList();
  const index = groups ? groups.findIndex(groupSelected) : items.findIndex(isSel);
  const entries = items.map((target, i) => ({ target, unread: groups ? groups[i].unread > 0 : !target.seen }));
  return { items, index, entries };
}

function spaceForward() {
  const reader = $("#reader");
  const outer = reader && $(".read-scroll", reader);
  if (!outer) return;
  const motion = getReaderScrollMotion();
  // 滚动完成后必须再按一次才允许换信，快速连按不排队、也不跳过未读正文。
  if (motion.active) return;
  const frame = $("#mail-frame", reader);
  const inner = frame?.contentDocument?.scrollingElement;
  const viewportRect = outer.getBoundingClientRect();
  const frameRect = frame?.getBoundingClientRect();
  const plan = window.MailBeaconReaderNav.spaceScrollPlan({
    outer, inner,
    frameTop: frameRect?.top, frameBottom: frameRect?.bottom,
    viewportTop: viewportRect.top, viewportBottom: viewportRect.bottom,
  });
  if (plan) {
    motion.scroll(plan.target === "inner" ? inner : outer, plan.top);
    return;
  }
  // 用同一份列表快照先确定目标身份，再让 openMessage 改选择和已读状态。
  const { entries, index } = listEntries();
  const next = window.MailBeaconReaderNav.nextMailTarget(entries, index, { preferUnread: true });
  if (next) return openMessage(next);
  if (index < 0) return toast("当前邮件已不在列表中");
  const k = key(S.acct, S.folder);
  const hasMore = S.acct !== "all" && !window.MailBeaconViews.isVirtual(S.folder) && !S.query &&
    !S.exhausted.has(k) && (S.morePages.get(k) === true || entries.length >= 100);
  toast(hasMore ? "当前已加载列表已到末尾" : "没有下一封了");
}

function step(delta) {
  const hadBatch = S.batch.size > 0;
  clearBatch();
  const { items, index } = listEntries();
  if (!items.length) { if (hadBatch) renderList(); return; }
  let i = index;
  i = i < 0 ? 0 : Math.min(items.length - 1, Math.max(0, i + delta));
  const m = items[i];
  if (!isSel(m)) openMessage({ accountId: m.accountId, folder: m.folder, uid: m.uid });
  else if (hadBatch) renderList();
}

const MS = () => window.MailBeaconSelection;
const MO = () => window.MailBeaconOperations;
let operationSequence = 0;
let pendingDeleteConfirm = null;
function clearBatch() { S.batch.clear(); S.batchAnchor = null; }
function selectionGroups() {
  return listGroups() || currentList().map(m => ({ latest:m, items:[m] }));
}
function selectMailRow(id, range = false) {
  const groups = selectionGroups();
  const index = groups.findIndex(g => MS().key(g.latest) === id);
  const anchor = range ? groups.findIndex(g => MS().key(g.latest) === S.batchAnchor) : null;
  S.batch = MS().toggle(S.batch, groups, index, anchor);
  if (!range) S.batchAnchor = id;
  renderList();
  // Re-rendering must not send keyboard focus back to the reader or document.
  [...(document.querySelectorAll?.('[data-select-mail]') || [])].find(el => el.dataset.selectMail === id)?.focus();
}
function bulkBar() {
  const count = MS().items(selectionGroups(),S.batch).length;
  if (!count) return "";
  const busy = S.moving.size > 0;
  return `<div class="bulk-bar" role="toolbar" aria-label="批量邮件操作"><div class="bulk-count">已选 ${count} 封 <span>仅已加载的邮件</span><button data-action="bulk-clear" aria-label="取消选择">×</button></div><div class="bulk-actions">
    ${[["seen","已读"],["unseen","未读"],["flagged","星标"],["archive","归档"],["move","移动"],["trash","删除"]].map(([action,label])=>`<button data-action="bulk-${action}" ${busy?'disabled':''}>${label}</button>`).join("")}</div></div>`;
}
function confirmPermanent(items) {
  return new Promise(resolve => {
    pendingDeleteConfirm = resolve;
    const names = [...new Set(items.map(m=>account(m.accountId)?.email || m.accountId))].join("、");
    $("#overlay").innerHTML = `<div class="scrim"><section class="sheet delete-confirm" role="dialog" aria-modal="true" aria-labelledby="delete-title"><h2 id="delete-title">彻底删除 ${items.length} 封邮件？</h2><p>这些邮件已在「已删除」中。删除后无法撤销，也不能从服务器恢复。</p><p class="hint">${esc(names)}</p><div class="confirm-actions"><button class="secondary" data-action="delete-cancel">取消</button><button class="danger" data-action="delete-confirm">彻底删除 ${items.length} 封</button></div></section></div>`;
    $('[data-action="delete-cancel"]')?.focus();
  });
}
function finishDeleteConfirmation(yes) {
  const resolve = pendingDeleteConfirm;
  pendingDeleteConfirm = null;
  closeOverlay();
  resolve?.(yes);
}
function refreshOperationSources(items, target) {
  const folders = new Map();
  for (const m of items) {
    folders.set(key(m.accountId,m.folder),[m.accountId,m.folder]);
    if (target) folders.set(key(m.accountId,target),[m.accountId,target]);
    if (window.MailBeaconViews.isVirtual(S.folder)) folders.set(key(m.accountId,S.folder),[m.accountId,S.folder]);
  }
  for (const [id,folder] of folders.values()) loadList(id,folder,{force:true});
}
function showOperationOutcome(result) {
  S.lastOperation = result;
  const summary = MO().summary(result);
  toast(summary.text,!summary.ok);
  const box = $("#toast");
  if (!box) return;
  box.insertAdjacentHTML("beforeend", '<button data-action="operation-details">查看记录</button>');
  if(summary.failed && S.lastRequest) box.insertAdjacentHTML("beforeend", '<button data-action="operation-retry">重试失败项</button>');
  if(summary.unknown || (summary.failed && !S.lastRequest)) box.insertAdjacentHTML("beforeend", '<button data-action="operation-refresh">刷新核对</button>');
  if (result.undoToken && result.expiresAt > Date.now()) {
    clearTimeout(toastTimer);
    box.insertAdjacentHTML("beforeend", '<button data-action="operation-undo">撤销</button>');
    toastTimer = setTimeout(()=> { $('[data-action="operation-undo"]',box)?.remove(); if(summary.ok) box.hidden=true; }, Math.min(10000,result.expiresAt-Date.now()));
  }
}
async function showOperationHistory() {
  try {
    const results = await invoke("list_mail_operations");
    $("#overlay").innerHTML = `<div class="scrim"><section class="sheet operation-history"><header><h2>操作记录</h2><button class="icon-btn" data-action="overlay-close" aria-label="关闭">${icon("x")}</button></header><p class="hint">只重试明确失败的邮件；结果待确认时请先刷新原文件夹和目标文件夹核对。</p><div class="operation-records">${results.length ? results.map(r=>`<article><strong>${esc(MO().summary(r).text)}</strong>${r.items.map(m=>`<p><span class="operation-state ${esc(m.status)}">${esc({succeeded:"成功",failed:"失败",unknown:"待确认"}[m.status]||"待确认")}</span>${esc(account(m.source.accountId)?.email||m.source.accountId)} · ${esc(folderInfo(m.source.accountId,m.source.folder)?.name||m.source.folder)} · #${m.source.uid}<small>${esc(m.message)}</small></p>`).join("")}</article>`).join("") : '<p class="hint">还没有操作记录</p>'}</div></section></div>`;
  } catch(e) { fail(e); }
}
async function undoLastOperation() {
  const result = S.lastOperation;
  if (!result?.undoToken || result.expiresAt <= Date.now()) return toast("撤销已过期，请到目标文件夹核对",true);
  const token = result.undoToken; result.undoToken = null;
  S.lastRequest = null;
  $('[data-action="operation-undo"]')?.remove();
  $('[data-action="operation-retry"]')?.remove();
  try {
    const undone = await invoke("undo_mail_operation",{token});
    refreshOperationSources(undone.items.map(m=>m.source));
    refreshOperationSources(result.items.map(m=>m.source));
    // 撤销是逐项反向移动，不能拿原来的归档/删除动作重试。
    S.lastRequest = null;
    showOperationOutcome(undone);
  } catch(e) { fail(e); }
}
async function runMailOperation(action, messages, {value=null,target=null,quiet=false}={}) {
  const items = [];
  for (const source of messages) {
    const current = findMsg(source);
    const recordedIdentity = source.receiveEpoch != null || source.uidValidity != null;
    if (recordedIdentity && ((account(source.accountId)?.receiveEpoch ?? 1) !== (source.receiveEpoch ?? 1)
        || (current && MS().key(current) !== MS().key(source)))) {
      toast("原邮件所属邮箱已发生变化，请刷新后重新选择；未操作其他邮件", true);
      return false;
    }
    // 仅最初的无身份 UI selection 可解析当前摘要，历史操作引用绝不重新绑定。
    items.push(recordedIdentity ? { ...current, ...source, localOnly: !!(current?.localOnly || source.localOnly) } : current || source);
  }
  if (!items.length || items.some(m=>S.moving.has(messageKey(m)))) return false;
  if (items.some(m=>m.localOnly || (isSel(m) && S.msg?.localOnly))) return toast("离线快照只供阅读，请连接邮箱后再整理",true);
  let confirmed = false;
  if (action === "trash") {
    const parts = MO().partitionTrash(items,(a,f)=>folderInfo(a,f)?.role);
    if (parts.permanent.length && parts.normal.length) return toast("请分开选择普通邮件和已删除邮件，避免误删",true);
    if (parts.permanent.length && !(confirmed = await confirmPermanent(parts.permanent))) return false;
  }
  const refs = items.map(m=>MS().source(m));
  const request = {operationId:globalThis.crypto?.randomUUID?.() || `mail-${Date.now()}-${++operationSequence}`,action,value,target,permanentConfirmed:confirmed,items:refs};
  for (const m of items) S.moving.add(messageKey(m));
  renderReader();
  const before = currentList();
  const selected = S.sel && {...S.sel};
  const selectedIndex = selected ? before.findIndex(m=>messageKey(m)===messageKey(selected)) : -1;
  try {
    const reply = await invoke("execute_mail_operation",{request});
    if (!Array.isArray(reply?.items)) throw new Error("未收到操作完成记录，请刷新核对，勿重复操作");
    const result = MO().completeResult(reply,refs);
    const successful = [];
    for (const item of result.items) {
      if (item.status !== "succeeded") continue;
      const source = refs.find(m=>MS().key(m)===MS().key(item.source));
      if (!source || (account(source.accountId)?.receiveEpoch ?? 1) !== source.receiveEpoch) continue;
      const current = findMsg(source);
      if (!current || MS().key(current)!==MS().key(source)) continue;
      successful.push(source);
      if (action === "seen" || action === "flagged") {
        const changed = current[action] !== value;
        patchMsg(source.accountId,source.folder,source.uid,{[action]:value});
        if (changed && action==="seen" && source.folder==="INBOX" && S.live[source.accountId]) S.live[source.accountId].unread=Math.max(0,S.live[source.accountId].unread+(value?-1:1));
      } else {
        S.removed.add(messageKey(source));
        dropMsg(source.accountId,source.folder,source.uid);
        const destination = target || (action === "inbox" ? "INBOX" : (folderByRole(source.accountId,action) || (action==="archive" && folderByRole(source.accountId,"all")))?.path);
        for (const folder of [destination,...window.MailBeaconViews.VIEWS.map(v=>v.key)].filter(Boolean)) {
          if (folder===source.folder || (folder===S.folder && window.MailBeaconViews.isVirtual(folder))) continue;
          const k=key(source.accountId,folder); S.lists.delete(k); S.loading.delete(k); S.exhausted.delete(k); S.listSeq.set(k,(S.listSeq.get(k)||0)+1);
        }
      }
      S.batch.delete(MS().key(source));
    }
    if (!["seen","flagged"].includes(action) && selected && isSel(selected) && successful.some(m=>messageKey(m)===messageKey(selected))) {
      const removed = new Set(successful.map(messageKey));
      const next = [...before.slice(selectedIndex+1),...before.slice(0,selectedIndex).reverse()].find(m=>!removed.has(messageKey(m)));
      if (next) openMessage(selectionOf(next)); else { ++openSeq; S.sel=null; S.msg=null; }
    }
    refreshOperationSources(refs,target);
    if (!quiet || !MO().summary(result).ok) { S.lastRequest=request; showOperationOutcome(result); }
    return MO().summary(result).ok;
  } catch(e) {
    fail(`操作结果未确认：${String(e)}。请刷新核对，勿重复操作`);
    refreshOperationSources(refs,target);
    return false;
  } finally {
    for (const m of items) S.moving.delete(messageKey(m));
    updateBadge(); render();
  }
}
async function moveCurrent(role,target,source=S.sel) {
  if (!source || !findMsg(source) || target===source.folder) return false;
  return runMailOperation(target ? "move" : role,[source],{target:target||null});
}
async function bulkAction(action,button) {
  const items = MS().items(selectionGroups(),S.batch);
  if (!items.length) return;
  if (action==="move") {
    const ids = [...new Set(items.map(m=>m.accountId))];
    const folders = (S.folders[ids[0]]||[]).filter(f=>ids.every(id=>(S.folders[id]||[]).some(other=>other.path===f.path)) && items.every(m=>m.folder!==f.path));
    return folders.length ? showMenu(button,folders.map(f=>({label:f.name,run:()=>runMailOperation("move",items,{target:f.path})}))) : toast("所选账户没有共同的目标文件夹，请分账户移动",true);
  }
  if (action==="trash") {
    const parts = MO().partitionTrash(items,(a,f)=>folderInfo(a,f)?.role);
    // Permanent deletion is always a separate, explicit batch.
    if (parts.normal.length && parts.permanent.length) return runMailOperation("trash",parts.normal);
  }
  return runMailOperation(action==="unseen"?"seen":action,items,{value:action==="unseen"?false:["seen","flagged"].includes(action)?true:null});
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
  if (!m || m.folder === "__outbox") return;
  return runMailOperation(flag,[m],{value});
}
async function markAllRead() {
  return runMailOperation("seen",currentList().filter(m=>!m.seen),{value:true});
}

function go(folder, acct = S.acct) {
  clearBatch();
  ++openSeq;
  S.acct = acct;
  S.folder = folder;
  S.sel = null;
  S.msg = null;
  S.filter = "all";
  const targets = acct === "all" ? S.accounts.map((a) => a.id) : [acct];
  for (const id of targets) {
    // 列表现在优先读本地索引（完整收件箱一次给全），进入文件夹时重新读一遍
    // 才能保证「所有收件箱」也拿到每个账号的完整列表
    loadList(id, folder);
    if (!S.folders[id]) loadFolders(id);
  }
  render();
}

/** 从系统通知打开：切视图时不走 go() 的 ++openSeq，避免取消紧随其后的 openMessage。 */
async function openFromNotification(sel) {
  const plan = window.MailBeaconNotificationOpen?.planOpenFromNotification?.(sel, {
    acct: S.acct,
    folder: S.folder,
  });
  if (!plan) return;
  if (plan.switchView) {
    S.acct = plan.sel.accountId;
    S.folder = plan.sel.folder;
    S.filter = "all";
    loadList(plan.sel.accountId, plan.sel.folder);
    if (!S.folders[plan.sel.accountId]) loadFolders(plan.sel.accountId);
    if (!plan.openUid) {
      S.sel = null;
      S.msg = null;
      render();
    }
  }
  if (plan.openUid) await openMessage(plan.sel);
  else if (!plan.switchView) render();
}

// ---------------------------------------------------------------------------
// 弹层：菜单、写信、设置
// ---------------------------------------------------------------------------
let menuReturnFocus = null;
let menuCleanup = null;
let attachmentMenuRequest = 0;
function openSendRecords() {
  hideReaderHint({ immediate: true });
  return window.MailBeaconSendRecords?.open({
    overlay: $("#overlay"), invoke, accountId: S.acct === "all" ? null : S.acct,
    onCheck: async (id) => {
      try {
        await loadFolders(id, { report: true });
        const sent = folderByRole(id, "sent");
        if (!sent) return fail("此账户没有已发送文件夹，可在邮箱网页版核查；发送记录查不到不代表没有发送");
        closeOverlay(); go(sent.path, id);
      } catch (e) { fail(e); }
    },
  });
}

function closeOverlay() {
  if (pendingDeleteConfirm) { const resolve=pendingDeleteConfirm; pendingDeleteConfirm=null; resolve(false); }
  window.MailBeaconSendRecords?.close();
  if ($('.sheet[data-busy="true"]')) return;
  ++attachmentMenuRequest;
  menuCleanup?.();
  window.MailBeaconDropdown?.close();
  const compose = $(".compose-sheet");
  compose?._quoteObserver?.disconnect();
  compose?._autosave?.dispose();
  clearTimeout(compose?._changeTimer);
  clearTimeout(compose?._suggestTimer);
  // 写信窗口无论怎么关闭（发送、保存、放弃或没改动直接关），都释放它登记的附件令牌。
  if (compose) releaseComposeAttachments(compose);
  $("#overlay").innerHTML = "";
  // 刷新邮箱会重绘侧栏，账户菜单仍要把焦点还给当前的入口。
  const focus = menuReturnFocus?.isConnected ? menuReturnFocus
    : menuReturnFocus?.dataset?.action === "account-menu" ? $('[data-action="account-menu"]') : null;
  menuReturnFocus = null;
  if (focus?.isConnected) focus.focus();
}

function showMenu(anchor, entries, point = null) {
  ++attachmentMenuRequest;
  menuCleanup?.();
  const overlay = $("#overlay");
  const r = anchor.getBoundingClientRect();
  menuReturnFocus = anchor;
  overlay.innerHTML = '<div class="scrim" style="background:none" data-close></div>';
  const panels = [];
  const closeChildren = (depth) => {
    while (panels.length > depth + 1) {
      const child = panels.pop();
      child.parent?.setAttribute("aria-expanded", "false");
      child.parent?.removeAttribute("aria-controls");
      child.menu.remove();
    }
  };
  const draw = (items, depth, parent = null, label = "邮件操作") => {
    const menu = document.createElement("div");
    menu.className = `menu${depth ? " menu-submenu" : ""}`;
    menu.setAttribute("role", "menu");
    menu.setAttribute("aria-label", label);
    menu.id = `action-menu-${depth}`;
    menu.innerHTML = items.map((e, i) => {
      if (e === "-") return '<div class="sep" role="separator"></div>';
      if (e.account !== undefined) return `<button role="menuitemradio" aria-checked="${!!e.check}" class="account-choice ${e.check ? "is-selected" : ""}" tabindex="-1" data-mi="${i}" title="${esc(e.label)}">${accountBadge(e.account, "sm")}<span class="account-choice-label">${esc(e.label)}</span><span class="check" aria-hidden="true">${e.check ? icon("check") : ""}</span></button>`;
      const children = !!e.children?.length;
      const image = safeAppIcon(e.image);
      const glyph = e.icon === "app" && !children
        ? `<span class="menu-app-icon" aria-hidden="true">${icon("app")}${image ? `<img src="${esc(image)}" alt="" decoding="async">` : ""}</span>`
        : e.check !== undefined ? `<span class="check">${e.check ? icon("check") : ""}</span>` : e.icon ? icon(e.icon) : "";
      return `<button role="menuitem" tabindex="-1" data-mi="${i}" title="${esc(e.label)}" ${e.disabled ? "disabled" : ""} ${children ? 'aria-haspopup="menu" aria-expanded="false"' : ""}>${glyph}<span class="menu-label">${esc(e.label)}</span>${e.sub ? `<span class="sub">${esc(e.sub)}</span>` : ""}${children ? `<span class="menu-chevron" aria-hidden="true">${icon("right")}</span>` : ""}</button>`;
    }).join("");
    overlay.appendChild(menu);
    const panel = { menu, parent };
    panels.push(panel);
    for (const img of menu.querySelectorAll(".menu-app-icon img")) {
      img.onload = () => img.parentElement.classList.add("has-image");
      img.onerror = () => { img.hidden = true; img.parentElement.classList.remove("has-image"); };
      if (img.complete && img.naturalWidth) img.onload();
    }
    const w = menu.offsetWidth, h = menu.offsetHeight;
    const origin = parent?.getBoundingClientRect();
    const owner = parent?.closest(".menu").getBoundingClientRect();
    const left = owner ? (owner.right + w + 4 <= innerWidth - 8 ? owner.right + 4 : owner.left - w - 4)
      : point ? point.x : r.left;
    const top = origin ? origin.top - 6 : point ? point.y
      : r.bottom + h + 6 > innerHeight ? r.top - h - 6 : r.bottom + 6;
    menu.style.left = Math.max(8, Math.min(left, innerWidth - w - 8)) + "px";
    menu.style.top = Math.max(8, Math.min(top, innerHeight - h - 8)) + "px";
    const buttons = [...menu.querySelectorAll('button:not(:disabled)')];
    const expand = (button, focus) => {
      const e = items[+button.dataset.mi];
      if (!e.children?.length) return;
      if (panels[depth + 1]?.parent !== button) {
        closeChildren(depth);
        draw(e.children, depth + 1, button, e.label);
        button.setAttribute("aria-expanded", "true");
        button.setAttribute("aria-controls", panels[depth + 1].menu.id);
      }
      if (focus) panels[depth + 1].menu.querySelector('button:not(:disabled)')?.focus();
    };
    menu.onkeydown = (ev) => {
      if (ev.isComposing || ev.metaKey || ev.ctrlKey || ev.altKey) return;
      if (ev.key === "Tab") { ev.preventDefault(); ev.stopPropagation(); closeOverlay(); return; }
      if ((ev.key === "ArrowLeft" || ev.key === "Escape") && depth) {
        ev.preventDefault(); ev.stopPropagation(); closeChildren(depth - 1); parent.focus(); return;
      }
      if (ev.key === "ArrowRight") {
        ev.preventDefault(); ev.stopPropagation();
        if (buttons.includes(document.activeElement)) expand(document.activeElement, true);
        return;
      }
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(ev.key) || !buttons.length) return;
      ev.preventDefault(); ev.stopPropagation(); closeChildren(depth);
      const i = buttons.indexOf(document.activeElement);
      const next = ev.key === "Home" ? 0 : ev.key === "End" ? buttons.length - 1
        : (i + (ev.key === "ArrowDown" ? 1 : -1) + buttons.length) % buttons.length;
      buttons[next].focus();
    };
    menu.onpointerover = (ev) => {
      const b = ev.target.closest("[data-mi]");
      if (!b || b.disabled || b.contains(ev.relatedTarget)) return;
      b.focus({ preventScroll: true });
      if (items[+b.dataset.mi].children?.length) expand(b, false);
      else closeChildren(depth);
    };
    menu.onscroll = () => closeChildren(depth);
    menu.onclick = (ev) => {
      const b = ev.target.closest("[data-mi]");
      if (!b || b.disabled) return;
      const entry = items[+b.dataset.mi];
      if (entry.children?.length) { expand(b, true); return; }
      closeOverlay();
      entry.run?.();
    };
    return menu;
  };
  const menu = draw(entries, 0);
  menu.querySelector('button:not(:disabled)')?.focus();
  // 别的入口可能直接换成写信/设置弹层，旧菜单的事件不能关掉新弹层。
  const onResize = () => {
    if (menu.isConnected && menu.parentElement === overlay) closeOverlay();
    else cleanup();
  };
  window.addEventListener("resize", onResize);
  const cleanup = () => {
    window.removeEventListener("resize", onResize);
    if (menuCleanup === cleanup) menuCleanup = null;
  };
  menuCleanup = cleanup;
}

/** 点收件人/抄送里的名字：菜单第一行显示完整地址，可以复制或直接写邮件。 */
function personMenuEntries(name, addr) {
  if (!addr) return null;
  const full = name ? `${name} <${addr}>` : addr;
  return [
    { label: full, disabled: true },
    "-",
    { label: "复制邮箱地址", run: () => copyAddress(addr) },
    { label: "写邮件", icon: "pen", run: () => openCompose({ to: full }) },
  ];
}

/** 未知类型（例如 .lic）没有系统推荐时，优先列出常见编辑器。 */
const ATTACHMENT_EDITORS = /^(TextEdit|Visual Studio Code|Code|Sublime Text|Typora|BBEdit|Xcode|TextMate|MacVim|文本编辑)$/i;

/** 系统推荐的应用；没有推荐时退回已安装应用，编辑器排前面。 */
function openWithApps(data) {
  const recommended = data?.recommended || [];
  const apps = recommended.length ? recommended : data?.installed || [];
  const unique = new Map();
  for (const app of apps) {
    if (!app?.path || !app.name) continue;
    const previous = unique.get(app.path);
    if (!previous || app.isDefault) unique.set(app.path, app);
  }
  const isEditor = (app) => ATTACHMENT_EDITORS.test(app.name) || ATTACHMENT_EDITORS.test(app.path.split("/").pop().replace(/\.app$/i, ""));
  const rank = (app) => app.isDefault ? 0 : !recommended.length && isEditor(app) ? 1 : 2;
  return [...unique.values()].sort((a, b) => rank(a) - rank(b));
}

/** 应用图标只接受后端生成的小尺寸 PNG，不加载本地路径或远程图片。 */
function safeAppIcon(value) {
  return typeof value === "string" && value.length <= 100000 && /^data:image\/png;base64,[A-Za-z0-9+/]+={0,2}$/.test(value) ? value : null;
}

function appMenuEntries(source, apps) {
  return apps.map((app) => ({
    label: app.name,
    icon: "app",
    image: safeAppIcon(app.icon),
    sub: app.isDefault ? "默认" : undefined,
    run: () => openAttachmentWithApp(source, app.path),
  }));
}

/** 右键附件：常用操作在一级菜单，应用放进可滚动的「打开方式」二级菜单。 */
async function showAttachmentMenu(att) {
  const request = ++attachmentMenuRequest;
  const previousOverlay = $("#overlay")?.firstChild;
  const source = JSON.parse(att.dataset.attachmentSource);
  const data = await invoke("attachment_open_with", source).catch((err) => {
    fail(err);
    return null;
  });
  if (!data || !att.isConnected || request !== attachmentMenuRequest || $("#overlay")?.firstChild !== previousOverlay) return;
  const apps = openWithApps(data);
  const entries = [{ label: "打开", icon: "file", run: () => openAttachment(source, att) }];
  if (apps.length) entries.push({ label: "打开方式", icon: "app", children: appMenuEntries(source, apps) });
  entries.push(
    "-",
    { label: "保存到「下载」", icon: "download", run: () => saveAttachmentFile(source) },
    { label: "在 Finder 中显示", icon: "folder", run: () => revealAttachment(source) },
  );
  showMenu(att, entries);
}

/** 双击/回车：先用默认程序打开；没有默认程序时退回「打开方式」列表。 */
async function openAttachment(source, anchor) {
  const request = ++attachmentMenuRequest;
  const previousOverlay = $("#overlay")?.firstChild;
  const stale = () => request !== attachmentMenuRequest || (anchor && !anchor.isConnected)
    || $("#overlay")?.firstChild !== previousOverlay;
  try {
    await invoke("open_attachment", source);
  } catch (err) {
    if (stale()) return;
    const data = await invoke("attachment_open_with", source).catch(() => null);
    if (stale()) return;
    const apps = openWithApps(data);
    if (!anchor || !apps.length) return fail(err);
    toast("没有默认打开方式，请选择应用");
    showMenu(anchor, appMenuEntries(source, apps));
  }
}

async function openAttachmentWithApp(source, appPath) {
  try {
    await invoke("open_attachment_with_app", { ...source, app: appPath });
  } catch (err) {
    fail(err);
  }
}

async function saveAttachmentFile(source) {
  try {
    const path = await invoke("save_attachment", source);
    toast(`已保存到 ${path.split("/").slice(-2).join("/")}`);
  } catch (err) {
    fail(err);
  }
}

async function revealAttachment(source) {
  try {
    await invoke("reveal_attachment", source);
  } catch (err) {
    fail(err);
  }
}

function showPersonMenu(btn) {
  const entries = personMenuEntries(btn.dataset.name || "", btn.dataset.addr || "");
  if (entries) showMenu(btn, entries);
}

/** 复制到剪贴板：优先 navigator.clipboard，旧 WebView 退回 execCommand。 */
async function copyText(text) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {}
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  } catch {
    return false;
  }
}

async function copyAddress(addr) {
  if (await copyText(addr)) toast(`已复制 ${addr}`);
  else fail("复制失败，请手动选择地址");
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
  if (!m || m.folder === "__outbox") return;
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
    ...(rulesAccounts().some((a) => a.id === m.accountId) && m.fromAddr
      ? ["-", { label: "按此发件人新建规则…", icon: "filter", run: () => openRules({ draft: window.MailBeaconRules.ruleFromMessage(m) }) }] : []),
  ], point);
}

function rowSelection(row) {
  const [accountId, uid, ...rest] = row.dataset.open.split("|");
  return { accountId, uid: +uid, folder: rest.join("|") };
}

function accountMenu(anchor) {
  const entries = [];
  if (S.accounts.length > 1) entries.push({ label: "所有账户", account: null, check: S.acct === "all", run: () => go("INBOX", "all") });
  for (const a of S.accounts) entries.push({ label: a.email, account: a, check: S.acct === a.id, run: () => go("INBOX", a.id) });
  entries.push("-", { label: "添加邮箱…", icon: "plus", run: () => openSettings(true) });
  showMenu(anchor, entries);
}

function openCreateFolder(source = null) {
  const moving = source ? selectionOf(source) : null;
  const selectedAccount = moving?.accountId || (S.acct !== "all" ? S.acct : S.accounts[0]?.id);
  if (!selectedAccount) return;
  $("#overlay").innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="folder-title"><div class="sheet folder-sheet">
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
  // 先出的文字版没有附件和内嵌图片，要等整封到了才能回复、转发
  if (!m || !v || v.partial) return null;
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
    const attachments = window.MailPulseDraft.messageAttachments(v, { ...selectionOf(m), uidValidity: v.uidValidity });
    return { ...original, accountId: m.accountId, to: "", cc: "", subject: "Fwd: " + strip(v.subject), body: "", attachments };
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
  const showCc = !!(d.cc || d.bcc);
  const editorHelper = window.MailPulseEditor;
  // 重新打开的草稿正文里已经有签名，不再插入，也不再随发件人自动替换（签名块为 null）。
  const signature = account(from)?.signature || "";
  const reopened = !!d.draftRef;
  const editorHtml = d.html != null ? d.html
    : reopened ? `<div>${editorHelper.textToHtml(d.body || "")}</div>`
    : editorHelper.initialHtml(d.body || "", signature);
  const signatureBlock = reopened || d.html != null ? null : editorHelper.signatureBlock(signature);
  const preservedQuote = d.quote ? window.MailPulseQuote.createQuote(d.quote) : null;
  const quoteMarkup = preservedQuote ? `<section class="compose-quote" aria-label="原邮件引用">
    <div class="compose-quote-head"><span>原邮件</span>${preservedQuote.rich && d.quoteHasRemote && !d.quoteRemoteImages ? '<button class="link" data-action="quote-images">显示引用中的图片</button>' : ""}</div>
    ${preservedQuote.notice ? `<p class="compose-quote-notice" role="status">${esc(preservedQuote.notice)}</p>` : ""}
    <div class="compose-quote-header">${esc(preservedQuote.header)}</div>
    ${preservedQuote.rich ? '<iframe id="compose-quote-frame" title="保留格式的原邮件" sandbox="allow-same-origin" referrerpolicy="no-referrer"></iframe>' : `<pre class="compose-quote-plain">${esc(preservedQuote.text)}</pre>`}
  </section>` : "";
  $("#overlay").innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="compose-title"><div class="sheet compose-sheet">
    <div class="sheet-head"><h3 id="compose-title">${esc(d.subject ? d.subject : "新邮件")}</h3><button class="icon-btn" data-action="compose-close" title="关闭" aria-label="关闭写信窗口">${icon("x")}</button></div>
    ${S.accounts.length > 1 ? `<div class="field"><label for="c-from">发件人</label><select id="c-from">${S.accounts.map((a) => `<option value="${esc(a.id)}" ${a.id === from ? "selected" : ""}>${esc(a.name)} &lt;${esc(a.email)}&gt;</option>`).join("")}</select></div>` : ""}
    <div class="field"><label>收件人</label><input id="c-to" value="${esc(d.to || "")}" spellcheck="false" autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="c-suggest" aria-expanded="false"><button class="link" data-action="pick-contacts" title="从通讯录选择">通讯录</button>${showCc ? "" : '<button class="link" data-action="show-cc">抄送 / 密送</button>'}</div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>抄送</label><input id="c-cc" value="${esc(d.cc || "")}" spellcheck="false" autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="c-suggest" aria-expanded="false"></div>
    <div class="field cc-row" ${showCc ? "" : "hidden"}><label>密送</label><input id="c-bcc" value="${esc(d.bcc || "")}" spellcheck="false" autocomplete="off" role="combobox" aria-autocomplete="list" aria-controls="c-suggest" aria-expanded="false"></div>
    <div class="recipient-suggest" id="c-suggest" role="listbox" aria-label="联系人建议" hidden></div>
    <div class="contact-picker" hidden></div>
    <div class="field"><label>主题</label><input id="c-subject" value="${esc(d.subject || "")}"></div>
    ${preservedQuote ? '<div class="compose-editor">' : ""}
    ${composeToolbar()}
    <div class="compose-popover" hidden></div>
    <div class="compose-body" id="c-body" contenteditable="true" role="textbox" aria-multiline="true" spellcheck="true" data-placeholder="${preservedQuote ? "在这里写回复…" : "写点什么…"}" aria-label="邮件正文"></div>
    ${quoteMarkup}${preservedQuote ? "</div>" : ""}
    <div class="compose-atts" aria-label="附件" hidden></div>
    <div class="compose-confirm" role="group" aria-label="邮件确认" hidden>
      <p class="compose-confirm-message" role="alert"></p>
      <div class="compose-confirm-actions"><button class="secondary" data-action="compose-continue">继续编辑</button>
        <button class="danger-link" data-action="compose-discard" hidden>不保存</button>
        <button class="primary" data-action="compose-save-draft" hidden>保存草稿</button>
        <button class="primary" data-action="compose-send-confirm" hidden>仍然发送</button></div>
    </div>
    <div class="sheet-foot"><button class="secondary" data-action="add-attachment" title="添加附件，也可以把文件拖进窗口">${icon("clip")}附件</button>
      <span class="hint">${esc(d.note || "多个收件人用逗号分隔 · ⌘↩ 发送")}</span>
      <label class="receipt-toggle" title="请收件人在读到邮件后回一封已读回执；对方可以拒绝"><input type="checkbox" id="c-receipt"${d.requestReceipt ? " checked" : ""}>已读回执</label>
      <button class="secondary" data-action="compose-close">取消</button>
      <button class="secondary" data-action="compose-schedule" title="定时发送">${icon("clock")}定时</button>
      <button class="primary" data-action="send">${icon("sent")}发送</button></div>
  </div></div>`;
  const sheet = $(".sheet");
  sheet.dataset.from = from;
  sheet.dataset.inReplyTo = d.inReplyTo || "";
  sheet.dataset.references = JSON.stringify(d.references || []);
  sheet._signatureBlock = signatureBlock;
  sheet._quote = preservedQuote;
  sheet._quoteSource = d.quoteSource ? { ...d.quoteSource } : null;
  sheet._quoteRemoteImages = !!d.quoteRemoteImages;
  if (preservedQuote?.rich) {
    sheet._quotePreview = window.MailPulseQuote.createQuote({ ...d.quote, html: d.quotePreviewHtml || d.quote.html });
    mountComposeQuote(sheet);
  }
  // 编辑器内容在解析外层标记之后单独赋值，读回来的 innerHTML 就是浏览器规范化后的形式。
  $("#c-body", sheet).innerHTML = editorHtml;
  sheet._attachments = d.attachments ? d.attachments.slice() : [];
  renderComposeAttachments(sheet);
  sheet._initialDraft = composeSnapshot(sheet);
  // 重新打开的草稿：原稿一直保留，自动保存写进单独的副本；
  // 用户「保存草稿」或发送后才删除原稿，「不保存」只删副本。
  sheet._originalDraft = d.draftRef || null;
  sheet._autosave = window.MailPulseDraft.createAutosaver({
    delay: DRAFT_AUTOSAVE_MS,
    save: (_snapshot, previous) => invoke("save_draft", { draft: buildDraft(sheet), previous }),
    onSaved: (ref) => { draftSaved(ref); setComposeHint(sheet, `草稿已保存 ${fmtDate(Date.now())}`); },
    onError: (e) => setComposeHint(sheet, `草稿保存失败：${e}`),
  });
  sheet._autosave.reset(sheet._initialDraft, null);
  const focus = d.to ? $("#c-body") : $("#c-to");
  focus.focus();
}

const FONTS = [
  ["", "字体"], ["PingFang SC, -apple-system, sans-serif", "苹方"], ["SimSun, Songti SC, serif", "宋体"],
  ["SimHei, Heiti SC, sans-serif", "黑体"], ["KaiTi, Kaiti SC, serif", "楷体"], ["Arial, Helvetica, sans-serif", "Arial"],
  ["Times New Roman, Times, serif", "Times"], ["Courier New, monospace", "等宽"],
];
const SIZES = [["", "字号"], ["small", "小"], ["normal", "正常"], ["large", "大"], ["huge", "特大"]];

function composeToolbar() {
  const tool = (cmd, label, content) => `<button class="tool" type="button" data-action="fmt" data-cmd="${cmd}" title="${label}" aria-label="${label}">${content}</button>`;
  return `<div class="compose-toolbar" role="toolbar" aria-label="格式">
    <select data-fmt="font" title="字体" aria-label="字体">${FONTS.map(([v, l]) => `<option value="${esc(v)}">${esc(l)}</option>`).join("")}</select>
    <select data-fmt="size" title="字号" aria-label="字号">${SIZES.map(([v, l]) => `<option value="${v}">${l}</option>`).join("")}</select>
    <label class="tool color" title="文字颜色"><span>A</span><input type="color" data-fmt="color" value="#c00000" aria-label="文字颜色"></label>
    <span class="tool-sep"></span>
    ${tool("bold", "加粗 ⌘B", "<b>B</b>")}${tool("italic", "斜体 ⌘I", "<i>I</i>")}${tool("underline", "下划线 ⌘U", "<u>U</u>")}${tool("strike", "删除线", "<s>S</s>")}
    <span class="tool-sep"></span>
    ${tool("ul", "项目符号", icon("list"))}${tool("ol", "编号", icon("listOrdered"))}
    ${tool("align-left", "左对齐", icon("alignLeft"))}${tool("align-center", "居中", icon("alignCenter"))}${tool("align-right", "右对齐", icon("alignRight"))}
    <span class="tool-sep"></span>
    ${tool("link", "插入链接", icon("link"))}${tool("image", "插入图片", icon("image"))}${tool("table", "插入表格", icon("table"))}
    ${tool("clear", "清除格式", icon("eraser"))}
    <span class="tool-sep"></span>
    <button class="tool" type="button" data-action="compose-templates" title="模板" aria-label="模板">${icon("template")}</button>
  </div>`;
}

// ---------------------------------------------------------------------------
// 收件人联想和通讯录选择
// ---------------------------------------------------------------------------
const RECIPIENT_FIELDS = new Set(["c-to", "c-cc", "c-bcc"]);

function scheduleSuggest(input) {
  const sheet = input.closest(".compose-sheet");
  if (!sheet) return;
  clearTimeout(sheet._suggestTimer);
  sheet._suggestTimer = setTimeout(() => loadSuggestions(sheet, input), 120);
}

async function loadSuggestions(sheet, input) {
  const R = window.MailBeaconRecipients;
  const token = R.currentToken(input.value, input.selectionStart ?? input.value.length);
  const seq = (sheet._suggestSeq || 0) + 1;
  sheet._suggestSeq = seq;
  if (!token.text) return hideSuggestions(sheet);
  let items = [];
  try {
    items = (await invoke("contacts_suggest", { query: token.text })) || [];
  } catch (e) {
    console.warn("联系人联想失败", e);
  }
  // 结果到达前用户可能已经按了 Esc、选了联想或换到别的输入框
  if (sheet._suggestSeq !== seq || !sheet.isConnected || document.activeElement !== input) return;
  if (!items.length) return hideSuggestions(sheet);
  sheet._suggest = { input, items, index: 0 };
  renderSuggestions(sheet);
}

function renderSuggestions(sheet) {
  const box = $(".recipient-suggest", sheet);
  const { input, items, index } = sheet._suggest;
  box.innerHTML = items.map((s, i) => {
    const main = s.kind === "group" ? `${esc(s.name)} <span class="s">${s.members.length} 人</span>` : esc(s.name || s.email);
    const sub = s.kind === "group" ? "分组" : s.name ? esc(s.email) : (s.kind === "recent" ? "最近联系人" : "");
    const forget = s.kind === "recent" ? `<button type="button" class="icon-btn suggest-forget" data-action="suggest-forget" data-index="${i}" title="不再推荐" aria-label="不再推荐 ${esc(s.email)}">${icon("x")}</button>` : "";
    return `<div class="suggest-row" role="none"><button type="button" id="c-suggest-${i}" class="suggest-item ${i === index ? "active" : ""}" data-action="suggest-pick" data-index="${i}" role="option" aria-selected="${i === index}">
      <span class="n">${main}</span><span class="e">${sub}</span></button>${forget}</div>`;
  }).join("");
  input.setAttribute("aria-expanded", "true");
  input.setAttribute("aria-activedescendant", `c-suggest-${index}`);
  const field = input.closest(".field");
  if (typeof field?.offsetTop === "number") box.style.top = `${field.offsetTop + field.offsetHeight}px`;
  box.hidden = false;
}

function hideSuggestions(sheet) {
  clearTimeout(sheet._suggestTimer);
  // 让还没返回的联想请求作废
  sheet._suggestSeq = (sheet._suggestSeq || 0) + 1;
  if (sheet._suggest) {
    sheet._suggest.input.setAttribute("aria-expanded", "false");
    sheet._suggest.input.removeAttribute("aria-activedescendant");
  }
  sheet._suggest = null;
  const box = $(".recipient-suggest", sheet);
  if (box && !box.hidden) {
    box.hidden = true;
    box.innerHTML = "";
  }
}

function pickSuggestion(sheet, index) {
  const current = sheet._suggest;
  const item = current?.items[index];
  if (!item) return;
  const R = window.MailBeaconRecipients;
  const input = current.input;
  const next = R.applySuggestion(input.value, input.selectionStart ?? input.value.length, R.suggestionText(item));
  // 分组可能包含已经填过的地址
  input.value = item.kind === "group" ? R.dedupe(next.value) : next.value;
  const caret = item.kind === "group" ? input.value.length : next.caret;
  input.focus();
  input.setSelectionRange?.(caret, caret);
  hideSuggestions(sheet);
  composeChanged(sheet);
}

async function forgetSuggestion(sheet, index) {
  const current = sheet._suggest;
  const item = current?.items[index];
  if (!item) return;
  try {
    await invoke("contacts_forget", { email: item.email });
  } catch (e) {
    return fail(e);
  }
  if (sheet._suggest !== current) return;
  current.items.splice(index, 1);
  if (!current.items.length) return hideSuggestions(sheet);
  current.index = Math.min(current.index, current.items.length - 1);
  renderSuggestions(sheet);
}

function showCcRows(sheet) {
  sheet.querySelectorAll(".cc-row").forEach((x) => (x.hidden = false));
  $('[data-action="show-cc"]', sheet)?.remove();
}

async function openContactPicker(sheet) {
  const box = $(".contact-picker", sheet);
  let contacts, groups;
  try {
    [contacts, groups] = await Promise.all([invoke("contacts_list", { query: null }), invoke("groups_list")]);
  } catch (e) {
    return fail(e);
  }
  const R = window.MailBeaconRecipients;
  const byId = new Map((contacts || []).map((c) => [c.id, c]));
  const contactRows = (contacts || []).filter((c) => c.emails?.length).map((c) => {
    const address = R.formatAddress(c.name, c.emails[0]);
    return `<label class="picker-row" data-picker-text="${esc(`${c.name} ${c.emails.join(" ")} ${c.org}`.toLowerCase())}">
      <input type="checkbox" data-pick-address="${esc(address)}"><span class="n">${esc(c.name || c.emails[0])}</span><span class="e">${esc(c.emails[0])}</span></label>`;
  }).join("");
  const groupRows = (groups || []).filter((g) => g.members.length).map((g) => {
    const members = g.members.map((id) => byId.get(id)).filter((c) => c?.emails?.length).map((c) => R.formatAddress(c.name, c.emails[0]));
    return `<label class="picker-row" data-picker-text="${esc(g.name.toLowerCase())}">
      <input type="checkbox" data-pick-group="${esc(JSON.stringify(members))}"><span class="n">${esc(g.name)}</span><span class="e">分组 · ${members.length} 人</span></label>`;
  }).join("");
  box.innerHTML = `<div class="picker-head"><input id="c-picker-q" placeholder="搜索联系人或分组" autocomplete="off">
      <button class="icon-btn" data-action="picker-close" title="关闭" aria-label="关闭通讯录">${icon("x")}</button></div>
    <div class="picker-list">${groupRows}${contactRows || '<div class="picker-empty">通讯录里还没有联系人，可以在侧栏「通讯录」中添加或导入。</div>'}</div>
    <div class="picker-actions"><button class="secondary" data-action="picker-add" data-target="to">添加到收件人</button>
      <button class="secondary" data-action="picker-add" data-target="cc">抄送</button>
      <button class="secondary" data-action="picker-add" data-target="bcc">密送</button></div>`;
  box.hidden = false;
  $("#c-picker-q", box)?.focus();
}

function closeContactPicker(sheet) {
  const box = $(".contact-picker", sheet);
  box.hidden = true;
  box.innerHTML = "";
  $('[data-action="pick-contacts"]', sheet)?.focus();
}

function addPickedRecipients(sheet, target) {
  const box = $(".contact-picker", sheet);
  const addresses = [];
  for (const el of box.querySelectorAll("[data-pick-group]")) if (el.checked) addresses.push(...JSON.parse(el.dataset.pickGroup));
  for (const el of box.querySelectorAll("[data-pick-address]")) if (el.checked) addresses.push(el.dataset.pickAddress);
  if (!addresses.length) return toast("请先勾选联系人");
  const field = $(`#c-${target}`, sheet);
  field.value = window.MailBeaconRecipients.addRecipients(field.value, addresses);
  if (target !== "to") showCcRows(sheet);
  closeContactPicker(sheet);
  composeChanged(sheet);
}

function filterContactPicker(input) {
  const q = input.value.trim().toLowerCase();
  for (const row of input.closest(".contact-picker").querySelectorAll(".picker-row")) {
    row.hidden = !!q && !row.dataset.pickerText.includes(q);
  }
}

// ---------------------------------------------------------------------------
// 通讯录管理
// ---------------------------------------------------------------------------
async function openContacts() {
  S.contactsUi = { query: "", detail: null };
  $("#overlay").innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="contacts-title"><div class="sheet contacts-sheet">
    <div class="sheet-head"><h3 id="contacts-title">通讯录</h3><button class="icon-btn" data-action="overlay-close" aria-label="关闭">${icon("x")}</button></div>
    <div class="contacts-toolbar"><input id="contacts-q" placeholder="搜索姓名、邮箱、单位" autocomplete="off">
      <button class="secondary" data-action="contact-new">新建联系人</button>
      <button class="secondary" data-action="group-new">新建分组</button>
      <button class="secondary" data-action="contacts-import" title="vCard（.vcf）或 CSV（Outlook、Gmail、网易导出）">导入</button>
      <button class="secondary" data-action="contacts-export" title="导出为 vCard">导出</button></div>
    <div class="contacts-body"><div class="contacts-list" aria-label="联系人"></div><div class="contacts-detail"></div></div>
  </div></div>`;
  await refreshContacts();
}

/**
 * 重新读取通讯录。列表按搜索词过滤；右侧编辑区和分组成员始终用完整列表，
 * 搜索不会让正在编辑的联系人「消失」，也不会漏掉被过滤掉的分组成员。
 * `detail` 为 false 时不重画编辑区，保留未保存的输入。
 */
async function refreshContacts({ detail = true } = {}) {
  const ui = S.contactsUi;
  if (!ui || !$(".contacts-sheet")) return;
  const seq = (ui.seq || 0) + 1;
  ui.seq = seq;
  try {
    const [all, groups, filtered] = await Promise.all([
      invoke("contacts_list", { query: null }),
      invoke("groups_list"),
      ui.query ? invoke("contacts_list", { query: ui.query }) : null,
    ]);
    if (ui.seq !== seq || S.contactsUi !== ui) return;
    ui.all = all || [];
    ui.contacts = filtered || ui.all;
    ui.groups = groups || [];
  } catch (e) {
    return fail(e);
  }
  if (ui.detail?.contact != null && !ui.all.some((c) => c.id === ui.detail.contact)) ui.detail = null;
  if (ui.detail?.group != null && !ui.groups.some((g) => g.id === ui.detail.group)) ui.detail = null;
  renderContactsList();
  if (detail) renderContactDetail();
}

function renderContactsList() {
  const ui = S.contactsUi;
  const box = $(".contacts-list");
  if (!box) return;
  const groups = ui.groups.map((g) => `<button class="contact-row ${ui.detail?.group === g.id ? "sel" : ""}" data-group-id="${g.id}">
      ${icon("users")}<span class="n">${esc(g.name)}</span><span class="e">${g.members.length} 人</span></button>`).join("");
  const contacts = ui.contacts.map((c) => `<button class="contact-row ${ui.detail?.contact === c.id ? "sel" : ""}" data-contact-id="${c.id}">
      ${avatar(c.name || c.emails[0], c.emails[0], "sm")}<span class="n">${esc(c.name || c.emails[0])}</span><span class="e">${esc(c.emails[0])}${c.emails.length > 1 ? ` 等 ${c.emails.length} 个` : ""}</span></button>`).join("");
  box.innerHTML = (groups ? `<div class="contacts-section">分组</div>${groups}` : "") +
    `<div class="contacts-section">联系人 · ${ui.contacts.length}</div>` +
    (contacts || `<div class="picker-empty">${ui.query ? "没有找到匹配的联系人" : "还没有联系人。可以新建，或导入 vCard / CSV。"}</div>`);
}

function renderContactDetail() {
  const ui = S.contactsUi;
  const box = $(".contacts-detail");
  if (!box) return;
  const detail = ui.detail;
  if (!detail) {
    box.innerHTML = `<div class="picker-empty">选择左侧的联系人或分组，或者新建一个。</div>`;
    return;
  }
  if (detail.group !== undefined) {
    const group = ui.groups.find((g) => g.id === detail.group) || { id: null, name: "", members: [] };
    const members = group.id === null ? "" : `<div class="form-title">成员</div><div class="member-list">${ui.all.map((c) =>
      `<label class="picker-row"><input type="checkbox" data-member-id="${c.id}" ${group.members.includes(c.id) ? "checked" : ""}>
        <span class="n">${esc(c.name || c.emails[0])}</span><span class="e">${esc(c.emails[0])}</span></label>`).join("") || '<div class="picker-empty">先添加联系人</div>'}</div>`;
    box.innerHTML = `<div class="form-grid contact-form"><label for="group-name">分组名称</label><input id="group-name" value="${esc(group.name)}">
      <div class="form-actions">${group.id === null ? "" : '<button class="danger-link" data-action="group-delete">删除分组</button>'}<button class="primary" data-action="group-save">保存</button></div></div>${members}`;
    return;
  }
  const c = ui.all.find((x) => x.id === detail.contact) || { id: null, name: "", emails: [], org: "", note: "" };
  box.innerHTML = `<div class="form-grid contact-form">
      <label for="contact-name">姓名</label><input id="contact-name" value="${esc(c.name)}">
      <label for="contact-emails">邮箱</label><textarea id="contact-emails" rows="3" placeholder="每行一个" spellcheck="false">${esc(c.emails.join("\n"))}</textarea>
      <label for="contact-org">单位</label><input id="contact-org" value="${esc(c.org)}">
      <label for="contact-note">备注</label><textarea id="contact-note" rows="2">${esc(c.note)}</textarea>
      <div class="form-actions">${c.id === null ? "" : `<button class="danger-link" data-action="contact-delete">删除</button>
        <button class="danger-link" data-action="contact-delete-confirm" hidden>确认删除</button>`}
        <button class="primary" data-action="contact-save">保存</button></div></div>`;
  // textarea 的初始内容在 HTML 解析时会丢掉开头的换行，单独赋值
  $("#contact-emails", box).value = c.emails.join("\n");
  $("#contact-note", box).value = c.note;
}

async function saveContact() {
  const ui = S.contactsUi;
  const contact = {
    id: ui.detail?.contact ?? null,
    name: $("#contact-name").value.trim(),
    emails: $("#contact-emails").value.split(/[\n,，;；]+/).map((x) => x.trim()).filter(Boolean),
    org: $("#contact-org").value.trim(),
    note: $("#contact-note").value.trim(),
  };
  try {
    const id = await invoke("contact_save", { contact });
    ui.detail = { contact: id };
    toast("已保存");
    await refreshContacts();
  } catch (e) {
    fail(e);
  }
}

async function deleteContact() {
  const ui = S.contactsUi;
  try {
    await invoke("contact_delete", { id: ui.detail.contact });
    ui.detail = null;
    toast("已删除");
    await refreshContacts();
  } catch (e) {
    fail(e);
  }
}

async function saveGroup() {
  const ui = S.contactsUi;
  try {
    const id = await invoke("group_save", { id: ui.detail?.group ?? null, name: $("#group-name").value });
    ui.detail = { group: id };
    await refreshContacts();
  } catch (e) {
    fail(e);
  }
}

async function deleteGroup() {
  const ui = S.contactsUi;
  try {
    await invoke("group_delete", { id: ui.detail.group });
    ui.detail = null;
    await refreshContacts();
  } catch (e) {
    fail(e);
  }
}

/** 只增减刚勾选的那个成员，其余成员保持不变。 */
async function updateGroupMembers(checkbox) {
  const ui = S.contactsUi;
  const group = ui.groups.find((g) => g.id === ui.detail?.group);
  if (!group) return;
  const id = Number(checkbox.dataset.memberId);
  const previous = group.members;
  const members = checkbox.checked
    ? (previous.includes(id) ? previous : previous.concat(id))
    : previous.filter((m) => m !== id);
  // 先更新本地，连续快速勾选时下一次基于这一次的结果；失败再撤回
  group.members = members;
  renderContactsList();
  try {
    await invoke("group_members_set", { id: group.id, members });
  } catch (e) {
    if (group.members === members) {
      group.members = previous;
      checkbox.checked = !checkbox.checked;
      renderContactsList();
    }
    fail(e);
  }
}

async function importContacts() {
  try {
    const result = await invoke("contacts_import");
    if (!result) return;
    toast(`导入完成：新增 ${result.added}，合并 ${result.updated}，跳过 ${result.skipped}（没有有效邮箱）`);
    await refreshContacts();
  } catch (e) {
    fail(e);
  }
}

async function exportContacts() {
  try {
    const path = await invoke("contacts_export");
    if (path) toast(`已导出到 ${path.split("/").slice(-2).join("/")}`);
  } catch (e) {
    fail(e);
  }
}

/** 编辑器里最近一次的选区：点工具栏或输入框会让编辑器失去焦点，执行命令前要还原。 */
function rememberSelection() {
  const sel = typeof window.getSelection === "function" ? window.getSelection() : null;
  if (!sel || !sel.rangeCount) return;
  const editor = $("#c-body");
  const range = sel.getRangeAt(0);
  if (editor && editor.contains(range.commonAncestorContainer)) editor.closest(".sheet")._range = range.cloneRange();
}

/** 选区当前就在编辑器里时以它为准，否则还原记下的选区。 */
function restoreSelection(sheet) {
  const editor = $("#c-body", sheet);
  const sel = typeof window.getSelection === "function" ? window.getSelection() : null;
  const current = sel?.rangeCount ? sel.getRangeAt(0) : null;
  if (current && editor.contains(current.commonAncestorContainer)) {
    sheet._range = current.cloneRange();
    return editor.focus();
  }
  editor.focus();
  if (sel && sheet._range && editor.contains(sheet._range.commonAncestorContainer)) {
    sel.removeAllRanges();
    sel.addRange(sheet._range);
  }
}

let applyingFormat = false;
function editorCommand(sheet, command, value = null, { keepFocus = null } = {}) {
  if (sheet.dataset.busy === "true") return;
  // 还原选区、执行命令时浏览器会把焦点移到编辑器；这期间的失焦不算用户离开下拉框
  applyingFormat = true;
  try {
    if (keepFocus) {
      // 下拉框要留着焦点（方向键继续选），只还原编辑器里的选区
      const sel = typeof window.getSelection === "function" ? window.getSelection() : null;
      const editor = $("#c-body", sheet);
      if (sel && sheet._range && editor.contains(sheet._range.commonAncestorContainer)) {
        sel.removeAllRanges();
        sel.addRange(sheet._range);
      } else if (sel) {
        // 编辑器还没获得过焦点：把光标放进编辑器并留在那里，样式作用于接下来的输入
        restoreSelection(sheet);
        keepFocus.value = "";
        keepFocus = null;
      }
    } else {
      restoreSelection(sheet);
    }
    // 格式命令用内联样式（span style），收件方客户端兼容最好；插入 HTML/文字时关掉，
    // 否则浏览器会给插入的内容附上一堆 background-color: initial 之类的样式。
    document.execCommand("styleWithCSS", false, !["insertHTML", "insertText"].includes(command));
    document.execCommand(command, false, value);
    if (command === "insertHTML") stripStyleNoise($("#c-body", sheet));
    rememberSelection();
    keepFocus?.focus();
  } finally {
    applyingFormat = false;
  }
  composeChanged(sheet);
}

/**
 * Chrome/WebKit 插入 HTML 时会补上 background-color: initial 之类的样式，
 * 去掉这些没有意义的声明，以及因此变成空壳的 <span>。
 */
function stripStyleNoise(editor) {
  for (const el of editor.querySelectorAll("[style]")) {
    if (!el.style?.removeProperty) return;
    for (const prop of [...el.style]) {
      if (/^(initial|inherit|unset)$/.test(el.style.getPropertyValue(prop).trim())) el.style.removeProperty(prop);
    }
    if (!el.getAttribute("style")) el.removeAttribute("style");
  }
  for (const span of editor.querySelectorAll("span")) {
    if (!span.attributes.length) span.replaceWith(...span.childNodes);
  }
}

function runFormat(sheet, action, value, options) {
  if (action === "link") return showComposePopover(sheet, "link");
  if (action === "table") return showComposePopover(sheet, "table");
  if (action === "image") return insertInlineImage(sheet);
  const command = window.MailPulseEditor.commandFor(action, value);
  if (command) editorCommand(sheet, command[0], command[1], options);
}

function showComposePopover(sheet, kind) {
  rememberSelection();
  const box = $(".compose-popover", sheet);
  box.dataset.kind = kind;
  if (kind === "templates") return renderTemplatesPopover(sheet);
  if (kind === "schedule") return renderSchedulePopover(sheet);
  box.innerHTML = kind === "link"
    ? `<label>链接地址</label><input id="c-link" placeholder="https://example.com 或邮箱地址" spellcheck="false">
       <button class="secondary" data-action="popover-cancel">取消</button><button class="primary" data-action="popover-ok">插入</button>`
    : `<label>表格</label><input id="c-rows" value="3" inputmode="numeric" aria-label="行数"> 行 × <input id="c-cols" value="3" inputmode="numeric" aria-label="列数"> 列
       <button class="secondary" data-action="popover-cancel">取消</button><button class="primary" data-action="popover-ok">插入</button>`;
  box.hidden = false;
  $(kind === "link" ? "#c-link" : "#c-rows", box).focus();
}

// ---------------------------------------------------------------------------
// 模板
// ---------------------------------------------------------------------------
async function renderTemplatesPopover(sheet) {
  const box = $(".compose-popover", sheet);
  let loadError = "";
  try { S.templates = await invoke("templates_list"); } catch (e) { loadError = String(e); }
  const list = loadError ? `<span class="hint err">模板读取失败：${esc(loadError)}</span>` : S.templates.map((t) => `<span class="template-chip"><button class="secondary" type="button" data-template-id="${esc(t.id)}" title="${esc(t.subject || "")}">${esc(t.name)}</button><button class="icon-btn" type="button" data-template-delete="${esc(t.id)}" aria-label="删除模板「${esc(t.name)}」">${icon("x")}</button></span>`).join("");
  box.innerHTML = `<div class="templates-list" role="group" aria-label="插入模板">${loadError ? list : list || '<span class="hint">还没有模板，可以把当前邮件存成模板</span>'}</div>
    <input id="c-template-name" placeholder="模板名称" aria-label="模板名称" autocomplete="off">
    <button class="secondary" type="button" data-action="template-save">存为模板</button>
    <button class="secondary" data-action="popover-cancel">关闭</button>`;
  box.hidden = false;
  (box.querySelector("[data-template-id]") || $("#c-template-name", box)).focus();
}

/** 编辑器里用户写的内容（不含签名），存模板用。 */
function editorContent(sheet) {
  const copy = $("#c-body", sheet).cloneNode(true);
  copy.querySelectorAll("[data-mp-signature], .mp-signature").forEach((n) => n.remove());
  return copy.innerHTML.trim();
}

async function saveTemplate(sheet) {
  const name = $("#c-template-name", sheet).value.trim();
  if (!name) return toast("请填写模板名称", true);
  const html = editorContent(sheet), subject = $("#c-subject", sheet).value;
  const existing = S.templates.find((t) => t.name === name);
  if (existing && !confirm(`替换已有的模板「${name}」？`)) return;
  const next = existing ? S.templates.map((t) => (t === existing ? { ...t, subject, html } : t)) : [...S.templates, { id: "", name, subject, html }];
  try {
    S.templates = await invoke("templates_save", { templates: next });
    toast(`已存为模板「${name}」`);
    await renderTemplatesPopover(sheet);
    $(".compose-popover", sheet).querySelector(`[data-template-id="${CSS.escape(S.templates.find((t) => t.name === name)?.id || "")}"]`)?.focus();
  } catch (e) {
    fail(e);
  }
}

async function deleteTemplate(sheet, id) {
  const t = S.templates.find((x) => x.id === id);
  if (!t || !confirm(`删除模板「${t.name}」？`)) return;
  try {
    S.templates = await invoke("templates_save", { templates: S.templates.filter((x) => x !== t) });
    renderTemplatesPopover(sheet);
  } catch (e) {
    fail(e);
  }
}

/** 插入模板：主题为空时填上；编辑器里只有签名时放在签名前，否则插到光标处。 */
async function insertTemplate(sheet, id) {
  const t = S.templates.find((x) => x.id === id);
  if (!t) return;
  hideComposePopover(sheet);
  const subject = $("#c-subject", sheet);
  if (!subject.value.trim() && t.subject) subject.value = t.subject;
  let html;
  try {
    html = window.MailPulseEditor.unwrapOuterDiv(await invoke("clean_html", { html: t.html, blockRemote: false }));
  } catch (e) {
    // 没清理过的 HTML 不能放进主窗口的编辑器
    return fail(`模板没能插入：${e}`);
  }
  const body = $("#c-body", sheet);
  const signature = body.querySelector("[data-mp-signature], .mp-signature");
  if (!editorContent(sheet).replace(/<div><br><\/div>|<br>/g, "").trim()) {
    for (const n of [...body.childNodes]) if (n !== signature) n.remove();
    if (signature) signature.insertAdjacentHTML("beforebegin", html + "<div><br></div>");
    else body.innerHTML = html;
  } else {
    editorCommand(sheet, "insertHTML", html);
  }
  composeChanged(sheet);
}

// ---------------------------------------------------------------------------
// 定时发送
// ---------------------------------------------------------------------------
function renderSchedulePopover(sheet) {
  const box = $(".compose-popover", sheet);
  const now = Date.now();
  box.innerHTML = `<label for="c-send-at">定时发送</label>
    ${X().quickSendTimes(now).map((q) => `<button class="secondary" type="button" data-send-at="${q.at}">${q.label}</button>`).join("")}
    <input type="datetime-local" id="c-send-at" value="${X().toLocalInput(X().defaultSendAt(now))}" min="${X().toLocalInput(now)}">
    <button class="secondary" data-action="popover-cancel">取消</button><button class="primary" type="button" data-action="schedule-ok">定时发送</button>
    <span class="hint">到时间时 MailBeacon 需要在运行（可以在后台）</span>`;
  box.hidden = false;
  $("#c-send-at", box).focus();
}

/** 定时发送和发送走同一条路：同样的检查（含无主题确认）、同样在提交期间锁住编辑。 */
function scheduleCompose(sheet, sendAt) {
  if (Number.isNaN(sendAt)) return toast("请选择发送时间", true);
  return sendCompose({ sendAt });
}

function hideComposePopover(sheet) {
  const box = $(".compose-popover", sheet);
  box.hidden = true;
  box.innerHTML = "";
}

function confirmComposePopover(sheet, target = null) {
  const box = $(".compose-popover", sheet);
  const helper = window.MailPulseEditor;
  if (box.dataset.kind === "templates") {
    // 模板名称框里回车＝存为模板；焦点在模板按钮上时由按钮自己处理
    if (target?.id === "c-template-name") return saveTemplate(sheet);
    return;
  }
  if (box.dataset.kind === "schedule") return scheduleCompose(sheet, X().fromLocalInput($("#c-send-at", box).value));
  if (box.dataset.kind === "link") {
    const url = helper.normalizeLink($("#c-link", box).value);
    if (!url) return toast("链接地址无效，只支持网址和邮箱", true);
    hideComposePopover(sheet);
    return editorCommand(sheet, "createLink", url);
  }
  const rows = parseInt($("#c-rows", box).value, 10) || 1;
  const cols = parseInt($("#c-cols", box).value, 10) || 1;
  hideComposePopover(sheet);
  editorCommand(sheet, "insertHTML", helper.tableHtml(rows, cols));
}

async function insertInlineImage(sheet) {
  rememberSelection();
  try {
    const url = await invoke("pick_inline_image");
    if (url) editorCommand(sheet, "insertHTML", `<img src="${esc(url)}">`);
  } catch (e) {
    fail(e);
  }
}

const MAX_PASTED_IMAGE = 10 * 1024 * 1024;

/**
 * 粘贴或拖入编辑器：HTML 交给后端清理（复制的表格常同时带一张预览图，以 HTML 为准），
 * 只有图片时转成 data URL，其余按纯文本插入。
 */
async function insertTransfer(e, data) {
  const sheet = e.target.closest?.(".compose-sheet");
  if (!sheet || sheet.dataset.busy === "true" || !data) return;
  e.preventDefault();
  const html = data.getData("text/html");
  const onlyImage = /^\s*(<meta[^>]*>\s*)?(<img\b[^>]*>\s*)?$/i.test(html || "");
  const image = [...(data.files || [])].find((f) => /^image\/(png|jpe?g|gif|webp|bmp)$/i.test(f.type));
  if (image && onlyImage) {
    if (image.size > MAX_PASTED_IMAGE) return toast("图片超过 10 MB，请作为附件发送", true);
    const url = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(image);
    }).catch(() => null);
    if (url) editorCommand(sheet, "insertHTML", `<img src="${esc(url)}">`);
    return;
  }
  if (html && !onlyImage) {
    try {
      const clean = await invoke("clean_html", { html });
      if (clean) return editorCommand(sheet, "insertHTML", clean);
    } catch (err) {
      console.warn("清理粘贴内容失败，改为纯文本", err);
    }
  }
  const text = data.getData("text/plain");
  if (text) editorCommand(sheet, "insertText", text);
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
      // 引用里的目录锚点只影响引用 iframe，不应被当作外部 URL 打开。
      if (keepMailFragment(e, link)) return;
      e.preventDefault();
      invoke("open_link", { url: link.href }).catch(fail);
    });
    doc.addEventListener("dblclick", openMailImage);
    doc.addEventListener("keydown", onKey);
  };
  frame.srcdoc = window.MailPulseQuote.previewDocument(sheet._quotePreview, { allowRemoteImages: sheet._quoteRemoteImages });
}

function composeValues(sheet, { full = true } = {}) {
  return {
    accountId: $("#c-from", sheet)?.value || sheet.dataset.from,
    to: $("#c-to", sheet).value,
    cc: $("#c-cc", sheet).value,
    bcc: $("#c-bcc", sheet).value,
    subject: $("#c-subject", sheet).value,
    ...(() => {
      const raw = $("#c-body", sheet).innerHTML;
      const html = full ? raw : window.MailPulseEditor.fingerprintHtml(raw);
      return { body: window.MailPulseEditor.htmlToText(html), html };
    })(),
    attachments: window.MailPulseDraft.toRefs(sheet._attachments || []),
  };
}

const DRAFT_AUTOSAVE_MS = 30000;
const MAX_ATTACHMENT_BYTES = 50 * 1024 * 1024;

function setComposeHint(sheet, text) {
  const hint = $(".sheet-foot .hint", sheet);
  if (hint && sheet.dataset.busy !== "true") hint.textContent = text;
}

/**
 * 用于比较是否改过的快照：正文里的图片数据只留指纹，不把几 MB 的 base64 拿来比较。
 */
function composeSnapshot(sheet) {
  const { html, ...values } = composeValues(sheet, { full: false });
  return { ...values, html };
}

/** 内容变化后稍等片刻再生成快照，连续输入时不反复序列化整篇正文。 */
function composeChanged(sheet) {
  clearTimeout(sheet._changeTimer);
  sheet._changeTimer = setTimeout(() => {
    if (sheet.isConnected) sheet._autosave?.change(composeSnapshot(sheet));
  }, COMPOSE_CHANGE_DEBOUNCE_MS);
}
const COMPOSE_CHANGE_DEBOUNCE_MS = 350;

function renderComposeAttachments(sheet) {
  const box = $(".compose-atts", sheet);
  if (!box) return;
  const list = sheet._attachments || [];
  box.hidden = !list.length;
  box.innerHTML = list.map((a) => `<span class="att-chip">${icon("file")}<span class="n">${esc(a.name)}</span><span class="s">${fmtSize(a.size || 0)}</span><button class="icon-btn" data-action="remove-attachment" data-key="${esc(window.MailPulseDraft.attachmentKey(a))}" title="移除附件" aria-label="移除 ${esc(a.name)}">${icon("x")}</button></span>`).join("");
}

function addComposeAttachments(sheet, picked) {
  if (!picked?.length) return;
  sheet._attachments = window.MailPulseDraft.addAttachments(sheet._attachments || [], picked);
  renderComposeAttachments(sheet);
  const limit = window.MailPulseDraft.limitMessage(sheet._attachments, MAX_ATTACHMENT_BYTES);
  if (limit) toast(limit, true);
  composeChanged(sheet);
}

/** 文件被拖进窗口：写信窗口开着就加为附件。 */
function onAttachmentsDropped(picked) {
  const sheet = $("#c-body")?.closest(".sheet");
  if (!sheet || sheet.dataset.busy === "true") {
    if (picked?.length) toast("先打开写信窗口，再把文件拖进来作为附件");
    return;
  }
  addComposeAttachments(sheet, picked);
}

async function pickComposeAttachments(sheet) {
  try {
    addComposeAttachments(sheet, await invoke("pick_attachments"));
  } catch (e) {
    fail(e);
  }
}

/** 草稿保存后刷新草稿箱列表（已加载时），本地草稿会让侧栏出现「本地草稿」。 */
function draftSaved(ref) {
  if (!ref) return;
  const a = ref.accountId;
  if (S.lists.has(key(a, ref.folder))) loadList(a, ref.folder, { force: true });
  const reloadDrafts = () => {
    if (S.lists.has(key(a, "__drafts")) || S.folder === "__drafts") loadList(a, "__drafts", { force: true });
  };
  // 本地草稿可能让「本地草稿」文件夹第一次出现，先刷新文件夹列表
  if (ref.folder === "__local_drafts") loadFolders(a).then(reloadDrafts);
  else reloadDrafts();
}

/** 发送和保存草稿共用：编辑区文字加上引用、附件和回复信息。 */
function buildDraft(sheet) {
  const { html, ...values } = composeValues(sheet);
  const draft = {
    ...values,
    ...window.MailPulseQuote.composeBody(values.body, sheet._quote || null, html),
    inReplyTo: sheet.dataset.inReplyTo || null,
    references: JSON.parse(sheet.dataset.references || "[]"),
    requestReceipt: !!$("#c-receipt", sheet)?.checked,
  };
  if (sheet._quote) Object.assign(draft, { quoteSource: sheet._quoteSource, quoteCids: sheet._quote.cids || [] });
  return draft;
}

const sameDraft = (a, b) => !!a && !!b && a.accountId === b.accountId && a.folder === b.folder && a.uid === b.uid && a.messageId === b.messageId;

/** 释放写信窗口登记的本地附件令牌（后端会丢掉暂存的内容）。 */
function releaseComposeAttachments(sheet) {
  const tokens = (sheet._attachments || []).filter((a) => a.kind === "file").map((a) => a.token);
  if (tokens.length) invoke("release_attachments", { tokens }).catch(() => {});
}

/** 删除草稿并从已加载的列表里移除。 */
async function deleteDraft(ref) {
  try {
    await invoke("delete_draft", { draft: ref });
    if (ref.uid) dropMsg(ref.accountId, ref.folder, ref.uid);
    draftSaved(ref);
  } catch (e) {
    console.warn("删除草稿失败", e);
  }
}

async function saveDraftAndClose(sheet) {
  if (sheet.dataset.busy === "true") return;
  hideComposeConfirmation(sheet);
  sheet.dataset.busy = "true";
  try {
    // 以当前内容为准，不依赖最后一次输入事件是否已经触发。
    sheet._autosave.change(composeSnapshot(sheet));
    const ref = await sheet._autosave.flush();
    const original = sheet._originalDraft;
    if (original && ref && !sameDraft(original, ref)) await deleteDraft(original);
    sheet.dataset.busy = "false";
    toast(ref?.folder === "__local_drafts" ? "已保存到本地草稿" : "已保存到草稿箱");
    if (sheet.isConnected) closeOverlay();
  } catch (e) {
    fail(e);
  } finally {
    sheet.dataset.busy = "false";
  }
}

async function discardCompose(sheet) {
  // 只删除这次自动保存出来的副本；重新打开的原稿保持原样。
  const ref = await sheet._autosave?.settle();
  closeOverlay();
  if (ref && !sameDraft(ref, sheet._originalDraft)) await deleteDraft(ref);
}

function showComposeConfirmation(sheet, kind) {
  const box = $(".compose-confirm", sheet);
  if (box.hidden) {
    const active = document.activeElement;
    const input = active?.closest(".sheet") === sheet && active.matches("input,textarea,select,[contenteditable]") ? active : $("#c-body", sheet);
    sheet._confirmFocus = { input, start: input.selectionStart, end: input.selectionEnd };
  }
  box.dataset.kind = kind;
  $(".compose-confirm-message", box).textContent = kind === "discard" ? "关闭前保存为草稿？" : "这封邮件没有主题，仍然发送？";
  $('[data-action="compose-discard"]', box).hidden = kind !== "discard";
  $('[data-action="compose-save-draft"]', box).hidden = kind !== "discard";
  $('[data-action="compose-send-confirm"]', box).hidden = kind !== "send-without-subject";
  $('[data-action="compose-send-confirm"]', box).textContent = "仍然发送";
  box.hidden = false;
  $('[data-action="send"]', sheet).disabled = true;
  $('[data-action="compose-continue"]', box).focus();
}

function hideComposeConfirmation(sheet) {
  $(".compose-confirm", sheet).hidden = true;
  sheet._pendingSendAt = null;
  $('[data-action="send"]', sheet).disabled = false;
  const focus = sheet._confirmFocus;
  delete sheet._confirmFocus;
  if (focus?.input.isConnected) {
    focus.input.focus();
    if (focus.input.matches("input,textarea") && typeof focus.start === "number") focus.input.setSelectionRange(focus.start, focus.end);
    else if (focus.input.id === "c-body") restoreSelection(sheet);
  }
}

function requestComposeClose() {
  const sheet = $("#c-body")?.closest(".sheet");
  if (!sheet || sheet.dataset.busy === "true") return;
  const current = composeSnapshot(sheet);
  if (JSON.stringify(current) !== JSON.stringify(sheet._initialDraft)) {
    showComposeConfirmation(sheet, "discard");
  } else closeOverlay();
}

/** 发送；给了 `sendAt` 时改为定时发送。 */
async function sendCompose({ allowEmptySubject = false, sendAt = null } = {}) {
  const sheet = $(".sheet");
  const btn = $('[data-action="send"]');
  if (!sheet || !btn || sheet.dataset.busy === "true") return;
  const confirmation = $(".compose-confirm", sheet);
  if (!confirmation.hidden) {
    // 确认框开着时只有「仍然发送」能继续；它沿用弹出确认时选的发送时间（可能是定时）
    if (!allowEmptySubject || confirmation.dataset.kind !== "send-without-subject") return;
    sendAt = sheet._pendingSendAt ?? null;
    hideComposeConfirmation(sheet);
  }
  const draft = buildDraft(sheet);
  if (!draft.to.trim() && !draft.cc.trim() && !draft.bcc.trim()) return fail("请填写收件人");
  const limit = window.MailPulseDraft.limitMessage(sheet._attachments || [], MAX_ATTACHMENT_BYTES);
  if (limit) return fail(limit);
  if (!draft.subject.trim() && !allowEmptySubject) {
    showComposeConfirmation(sheet, "send-without-subject");
    sheet._pendingSendAt = sendAt;
    if (sendAt) {
      $(".compose-confirm-message", sheet).textContent = `这封邮件没有主题，仍然定时发送（${X().fmtSendAt(sendAt)}）？`;
      $('[data-action="compose-send-confirm"]', sheet).textContent = "仍然定时发送";
    }
    return;
  }
  const source = sheet._quoteSource;
  const controls = [...sheet.querySelectorAll("input,textarea,select,button")].map((control) => ({ control, disabled: control.disabled }));
  const editor = $("#c-body", sheet);
  sheet.dataset.busy = "true";
  for (const { control } of controls) control.disabled = true;
  editor.setAttribute("contenteditable", "false");
  const hint = $(".sheet-foot .hint", sheet);
  const previousHint = hint.textContent;
  hint.textContent = sendAt ? "正在保存定时邮件…" : "正在发送，请稍候…";
  btn.innerHTML = sendAt ? `${icon("clock")}保存中…` : `${icon("sent")}发送中…`;
  try {
    // 先等正在进行的自动保存结束，发送成功后由后端删除这份草稿。
    const autosaved = await sheet._autosave.settle();
    const draftRefs = [autosaved, sheet._originalDraft].filter((ref, i, all) => ref && all.findIndex((x) => sameDraft(x, ref)) === i);
    if (sendAt) await invoke("schedule_mail", { draft, draftRefs, sendAt });
    else {
      const createId = () => globalThis.crypto?.randomUUID?.() || `send-${Date.now()}-${Math.random().toString(36).slice(2)}`;
      if (window.MailBeaconSendRecords) window.MailBeaconSendRecords.submissionId(sheet, draft, createId);
      else sheet._sendId ||= createId();
      const result = await invoke("send_mail", { draft, draftRefs, sendId: sheet._sendId });
      if (result && result.status !== "accepted") {
        sheet.dataset.busy = "false";
        closeOverlay();
        await openSendRecords();
        return;
      }
    }
    // 回复会出现在会话里：下次看这个会话时重新查
    if (S.thread) S.thread.stale = true;
    for (const ref of draftRefs) if (ref.uid) dropMsg(ref.accountId, ref.folder, ref.uid);
    sheet.dataset.busy = "false";
    if (sheet.isConnected) closeOverlay();
    if (sendAt) {
      toast(`将于 ${X().fmtSendAt(sendAt)} 发送`);
      await refreshOutbox();
      render();
      return;
    }
    toast("已发送");
    if (draft.inReplyTo) {
      const m = findMsg(source);
      if (m) patchMsg(m.accountId, m.folder, m.uid, { answered: true });
    }
  } catch (e) {
    fail(e);
    sheet._submitError = `发送结果未确认：${String(e)}。可在侧栏“发送记录”中检查；不要另开邮件盲目重发。`;
    sheet._autosave.resume();
  } finally {
    sheet.dataset.busy = "false";
    for (const { control, disabled } of controls) control.disabled = disabled;
    editor.setAttribute("contenteditable", "true");
    btn.innerHTML = `${icon("sent")}发送`;
    hint.textContent = sheet._submitError || previousHint;
  }
}

let formKind = "qq";
let formReceiveProtocol = "imap";
function accountForm(err = "") {
  const k = KINDS.find((x) => x.id === formKind);
  const custom = formKind === "custom";
  const pop3 = formReceiveProtocol === "pop3";
  return `<div class="kinds">${KINDS.map((x) => `<button class="kind ${x.id === formKind ? "on" : ""}" data-kind="${x.id}">${esc(x.label)}</button>`).join("")}</div>
    <form class="form-grid" id="acct-form" autocomplete="off">
      <label>邮箱地址</label><input name="email" type="email" placeholder="${formKind === "gmail" ? "you@gmail.com" : formKind === "exmail" ? "you@company.com" : "you@" + (formKind === "custom" ? "example.com" : formKind + ".com")}" required spellcheck="false">
      <label>授权码</label><input name="password" type="password" placeholder="不是网页登录密码" required>
      <div class="form-hint">${esc(k.hint)}</div>
      <label>显示名称</label><input name="name" placeholder="可选，发信时对方看到的名字">
      <label for="receive-protocol">收信协议</label><select name="receiveProtocol" id="receive-protocol"><option value="imap" data-description="文件夹、已读与实时推送" ${!pop3 ? "selected" : ""}>IMAP</option><option value="pop3" data-description="仅收取收件箱邮件" ${pop3 ? "selected" : ""}>POP3</option></select>
      ${custom && !pop3 ? `<label>IMAP</label><div class="pair"><input name="imapHost" placeholder="imap.example.com" spellcheck="false"><input name="imapPort" value="993" inputmode="numeric"></div>` : ""}
      ${pop3 ? `<label>POP3</label><div class="pair"><input name="pop3Host" placeholder="pop.example.com" spellcheck="false"><input name="pop3Port" value="995" inputmode="numeric"></div><label class="check"><input name="pop3Tls" type="checkbox" checked>使用 SSL/TLS</label>` : ""}
      ${custom ? `
      <label>SMTP</label><div class="pair"><input name="smtpHost" placeholder="smtp.example.com" spellcheck="false"><input name="smtpPort" value="465" inputmode="numeric"></div>` : ""}
      ${err ? `<div class="form-err">${esc(err)}</div>` : ""}
      <div class="form-actions"><button class="primary" type="submit">登录并添加</button></div>
    </form>`;
}

async function submitAccount(form) {
  if (form.dataset.busy === "true") return;
  form.dataset.busy = "true";
  const f = new FormData(form);
  const input = { kind: formKind, email: f.get("email"), password: f.get("password"), name: f.get("name") || "", receiveProtocol: f.get("receiveProtocol") || "imap" };
  if (formKind === "custom") {
    Object.assign(input, {
      imapHost: f.get("imapHost") || "", imapPort: +f.get("imapPort") || 993,
      smtpHost: f.get("smtpHost"), smtpPort: +f.get("smtpPort") || 465,
    });
  }
  if (input.receiveProtocol === "pop3") {
    input.pop3Host = f.get("pop3Host") || "";
    input.pop3Port = +f.get("pop3Port") || 995;
    input.pop3Tls = !!f.get("pop3Tls");
  }
  const keep = window.MailBeaconAccountRepair.capture(form);
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
    if (!form.isConnected) return;
    $("#acct-form-wrap", box).innerHTML = accountForm(String(e));
    const nf = $("#acct-form", box);
    window.MailBeaconAccountRepair.restore(nf, keep);
    nf.password.focus();
  }
}

function accountConnectionForm(a) {
  const pop = a.receiveProtocol === 'pop3';
  const prefix = pop ? 'pop3' : 'imap';
  const currentHost = a[prefix + 'Host'] || '';
  const currentPort = a[prefix + 'Port'] || (pop ? 995 : 993);
  return `<details class="account-connection"><summary>连接设置与授权码</summary>
    <form class="form-grid account-repair-form" data-account-repair="${esc(a.id)}" autocomplete="off">
      <label>显示名称</label><input name="name" value="${esc(a.name)}">
      <label>新授权码</label><div class="connection-password"><input name="password" type="password" placeholder="留空保持原授权码" autocomplete="new-password"><button class="link" type="button" data-action="repair-password">显示</button></div>
      <label>${pop ? 'POP3' : 'IMAP'} 收信服务器</label><div class="pair"><input name="${prefix}Host" value="${esc(currentHost)}" required spellcheck="false" aria-label="收信服务器"><input name="${prefix}Port" type="number" min="1" max="65535" value="${currentPort}" required aria-label="收信端口"></div>
      ${pop ? `<label class="check"><input name="pop3Tls" type="checkbox" ${a.pop3Tls !== false ? 'checked' : ''}>收信使用 SSL/TLS</label>` : ''}
      <label>SMTP 发信服务器</label><div class="pair"><input name="smtpHost" value="${esc(a.smtpHost || '')}" required spellcheck="false" aria-label="发信服务器"><input name="smtpPort" type="number" min="1" max="65535" value="${a.smtpPort || 465}" required aria-label="发信端口"></div>
      <p class="form-hint">邮箱地址和接收协议保持不变。检测只验证连接与认证，不发送邮件。更换收信服务器会暂停下载、隔离旧缓存；原导出文件保留。</p>
      ${a.previousArchiveDirs?.length ? `<p class="form-hint">历史导出目录（仅保留文件，不复用到新服务器）：${a.previousArchiveDirs.map(esc).join('、')}</p>` : ''}
      <div class="connection-actions"><button type="button" class="secondary" data-action="repair-test">检测收发信</button><button type="button" class="secondary" data-action="repair-reconnect">重试已保存的连接</button><button type="submit" class="primary">保存并重连</button></div>
      <p class="connection-status" role="status" aria-live="polite" hidden></p>
    </form></details>`;
}

function adoptRepairedAccount(next) {
  const old = account(next.id);
  S.accounts = S.accounts.map(a => a.id === next.id ? next : a);
  if ((next.receiveEpoch || 1) !== (old?.receiveEpoch || 1)) {
    for (const map of [S.lists, S.results, S.indexing]) for (const k of [...map.keys()]) if (k.startsWith(next.id + '|')) map.delete(k);
    delete S.folders[next.id];
    if (S.sel?.accountId === next.id) { ++openSeq; S.sel = null; S.msg = null; S.thread = null; }
  }
  const events = S.accountRepairEvents.get(next.id) || [];
  S.accountRepairEvents.delete(next.id);
  for (const event of events) onMailEvent(event);
  render();
}

async function runAccountRepair(form, action) {
  if (!form || form.dataset.busy === 'true') return;
  const a = account(form.dataset.accountRepair);
  if (!a) return;
  const status = $('.connection-status', form);
  const controls = [...form.querySelectorAll('input,button')];
  const fields = Object.fromEntries(new FormData(form));
  if (a.receiveProtocol === 'pop3') fields.pop3Tls = !!form.elements.pop3Tls.checked;
  const input = window.MailBeaconAccountRepair.input(a, fields);
  form.dataset.busy = 'true';
  form.dataset.awaitConnection = action === 'test' ? 'false' : 'true';
  for (const c of controls) c.disabled = true;
  status.hidden = false;
  status.classList.remove('err');
  status.textContent = action === 'test' ? '正在分别检测收信和发信…' : '正在保存并重建此账户连接…';
  try {
    if (action === 'test') {
      const result = await invoke('test_account_connection', { accountId: a.id, input });
      if (form.isConnected) status.textContent = window.MailBeaconAccountRepair.testSummary(result);
    } else {
      const next = await invoke(action === 'reconnect' ? 'reconnect_account' : 'update_account_connection', action === 'reconnect' ? { accountId: a.id } : { accountId: a.id, input });
      adoptRepairedAccount(next);
      if (form.isConnected) {
        form.elements.password.value = '';
        form.elements.password.type = 'password';
        form.querySelector('[data-action="repair-password"]').textContent = '显示';
        status.textContent = S.live[next.id]?.state === 'online' ? '收信连接已恢复；可检测发信连接。' : '设置已保存，正在重新连接…';
        if (next.receiveEpoch !== a.receiveEpoch) status.textContent += '收信身份已更新，旧导出文件保留；下载需重新选择目录。';
      }
    }
  } catch (error) {
    if (form.isConnected) { status.textContent = String(error); status.classList.add('err'); form.dataset.awaitConnection = 'false'; }
    // 恢复提交可能已完成配置落盘，刷新非敏感快照以便原地重试。
    try { const current = await invoke('snapshot'); for (const next of current.accounts) adoptRepairedAccount(next); } catch {}
  } finally {
    form.dataset.busy = 'false';
    for (const c of controls) c.disabled = false;
  }
}

function openAccountRepair(id) {
  S.settingsExpanded.set(id, true);
  openSettings();
  const form = [...document.querySelectorAll('.account-repair-form')].find(form => form.dataset.accountRepair === id);
  if (form) { form.closest('details').open = true; form.scrollIntoView({ block: 'nearest' }); form.elements.password.focus(); }
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
        ${accountBadge(a)}
        <span class="meta"><span class="n">${esc(a.email)}</span><span class="s ${l.state === "error" ? "err" : ""}">${esc(a.name)} · ${esc(st)}</span><span class="account-bulk-summary" hidden></span></span>
        ${icon("right", "account-disclosure")}
      </button>
      <button class="danger-link" data-remove="${esc(a.id)}">移除</button></div>
      <div class="settings-account-details" id="settings-account-details-${esc(a.id)}" ${expanded ? "" : "hidden"}>
      ${accountConnectionForm(a)}
      ${accountNotifySection(a)}
      <form class="signature-form" data-signature-account="${esc(a.id)}">
        <label for="signature-${esc(a.id)}">邮件签名</label>
        <textarea id="signature-${esc(a.id)}" name="signature" rows="3" placeholder="姓名、职务或联系方式">${esc(a.signature || "")}</textarea>
        <div class="signature-actions"><span class="hint">新邮件、回复和转发时自动加入；留空保存可关闭。</span><button class="secondary" type="submit">保存签名</button></div>
        <p class="signature-status" role="status" hidden></p>
      </form>
      ${bodyCacheForm(a)}
      ${autoDownloadForm(a)}
      ${bulkDownloadSection(a)}</div></section>`;
  }).join("");
  $("#overlay").innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="settings-title"><div class="sheet">
    <div class="sheet-head"><h3 id="settings-title">${addOnly ? "添加邮箱" : "设置"}</h3><button class="icon-btn" data-action="overlay-close">${icon("x")}</button></div>
    <div class="settings-body">
      ${addOnly ? "" : `${prefsForm()}<div class="form-title">邮箱账户</div>${rows}<div class="form-title">添加邮箱</div>`}
      <div id="acct-form-wrap">${accountForm()}</div>
    </div></div></div>`;
  renderAutoDownloadStatus();
  renderBulkDownloadStatus();
  renderSettingsAccounts();
  if (!addOnly) loadBodyCacheStatus();
  if (!addOnly) for (const a of S.accounts) if (!Array.isArray(S.folders[a.id])) loadNotifyFolders(a.id);
}

// ---------------------------------------------------------------------------
// 通知、托盘、开机启动
// ---------------------------------------------------------------------------
// 画界面用：还在保存中的改动优先，免得重画时把刚勾的选项抹掉
const currentPrefs = () => S.prefsWanted || S.prefs || { autostart: false, tray: true, notify: true, notifyContent: "full", dnd: false, dndStart: "22:00", dndEnd: "08:00", mutedAccounts: [], notifyFolders: {} };
const checked = (on) => (on ? " checked" : "");

function prefsForm() {
  const p = currentPrefs();
  return `<div class="form-title" style="margin-top:8px">通知与常规</div>
    <form class="prefs-form" aria-label="通知与常规设置">
      <label class="prefs-toggle"><input type="checkbox" name="notify"${checked(p.notify)}>新邮件弹出系统通知</label>
      <div class="prefs-row"><label for="prefs-content">通知内容</label>
        <select class="auto-download-filter" id="prefs-content" name="notifyContent"${p.notify ? "" : " disabled"}>
          ${window.MailBeaconPrefs.CONTENT.map((c) => `<option value="${c.key}"${p.notifyContent === c.key ? " selected" : ""}>${c.label}</option>`).join("")}
        </select></div>
      <label class="prefs-toggle"><input type="checkbox" name="dnd"${checked(p.dnd)}${p.notify ? "" : " disabled"}>免打扰时段</label>
      <div class="prefs-row prefs-times"><input type="time" name="dndStart" value="${esc(p.dndStart)}"${p.dnd && p.notify ? "" : " disabled"} aria-label="免打扰开始时间"><span>至</span><input type="time" name="dndEnd" value="${esc(p.dndEnd)}"${p.dnd && p.notify ? "" : " disabled"} aria-label="免打扰结束时间"></div>
      <p class="hint">免打扰期间照常收信，只是不弹通知；结束时间早于开始时间表示跨过午夜。窗口在前台时不弹通知。</p>
      <label class="prefs-toggle"><input type="checkbox" name="tray"${checked(p.tray)}>显示托盘图标（关闭窗口后继续在后台收信）</label>
      <label class="prefs-toggle"><input type="checkbox" name="autostart"${checked(p.autostart)}>开机自动启动（在后台运行）</label>
      <p class="prefs-status" role="status" hidden></p>
    </form>`;
}

function accountNotifySection(a) {
  const p = currentPrefs();
  const muted = (p.mutedAccounts || []).includes(a.id);
  const chosen = new Set(p.notifyFolders?.[a.id] || []);
  const loaded = Array.isArray(S.folders[a.id]);
  const failed = !loaded && S.folderErrors.get(a.id);
  const folders = window.MailBeaconPrefs.notifiableFolders(S.folders[a.id]);
  const hintId = `notify-hint-${esc(a.id)}`;
  const list = failed ? `<p class="hint err">读取文件夹失败：${esc(failed)} <button class="link" type="button" data-action="retry-notify-folders">重试</button></p>`
    : !loaded ? `<p class="hint">正在读取文件夹…</p>`
    : folders.length ? `<fieldset class="notify-folders"><legend class="hint">收件箱之外，这些文件夹有新邮件也提醒（例如被规则移走的邮件）：</legend>${folders.map((f) => `<label><input type="checkbox" name="notifyFolder" value="${esc(f.path)}"${checked(chosen.has(f.path))}${muted || !p.notify ? " disabled" : ""}>${esc(f.name || f.path)}</label>`).join("")}</fieldset>`
    : `<p class="hint">没有可以单独提醒的文件夹。</p>`;
  return `<section class="account-notify" data-notify-account="${esc(a.id)}">
    <label class="prefs-toggle"><input type="checkbox" name="accountNotify" aria-describedby="${hintId}"${checked(!muted)}${p.notify ? "" : " disabled"}>此账号新邮件提醒</label>
    <p class="hint account-notify-hint" id="${hintId}">${esc(window.MailBeaconPrefs.accountNotifyHint(p, a.id))}</p>
    ${list}
  </section>`;
}

/** 设置开着时，只重画这个账号的提醒设置（文件夹列表读到了或读取失败）。 */
function redrawNotifySection(accountId) {
  const a = account(accountId);
  const el = [...($("#overlay")?.querySelectorAll("[data-notify-account]") || [])].find((x) => x.dataset.notifyAccount === accountId);
  if (!a || !el) return;
  // 重画会换掉节点，焦点在里面时放回同名的控件上
  const focused = el.contains?.(document.activeElement) ? document.activeElement : null;
  const name = focused?.name, value = focused?.value;
  el.outerHTML = accountNotifySection(a);
  if (focused) {
    const fresh = [...$("#overlay").querySelectorAll("[data-notify-account]")].find((x) => x.dataset.notifyAccount === accountId);
    [...(fresh?.querySelectorAll(`[name="${name}"]`) || [])].find((x) => x.value === value)?.focus();
  }
}

/** 设置里要列出每个账号的文件夹；还没读到的现在读。读到后 loadFolders 会重画那一块。 */
function loadNotifyFolders(accountId) {
  S.folderErrors.delete(accountId);
  return Promise.resolve(loadFolders(accountId, { report: true })).then(() => redrawNotifySection(accountId), (e) => {
    // 同时有别的读取成功了就不报错
    if (Array.isArray(S.folders[accountId])) return;
    S.folderErrors.set(accountId, String(e));
    redrawNotifySection(accountId);
  });
}

function forgetAccountPrefs(id) {
  if (S.prefs) S.prefs = window.MailBeaconPrefs.forgetAccount(S.prefs, id);
  if (S.prefsWanted) S.prefsWanted = window.MailBeaconPrefs.forgetAccount(S.prefsWanted, id);
}

/** 读出发生变化的那块设置（常规表单或某个账号）。 */
function prefsControls(target) {
  const within = (sel) => { const el = target.closest?.(sel); return el?.matches?.(sel) ? el : null; };
  const form = within(".prefs-form");
  if (form) {
    const f = (name) => form.querySelector(`[name="${name}"]`);
    return { notify: f("notify").checked, notifyContent: f("notifyContent").value, dnd: f("dnd").checked, dndStart: f("dndStart").value, dndEnd: f("dndEnd").value, tray: f("tray").checked, autostart: f("autostart").checked };
  }
  const section = within("[data-notify-account]");
  if (!section) return null;
  const list = section.querySelector(".notify-folders");
  return { accounts: { [section.dataset.notifyAccount]: {
    notify: section.querySelector('[name="accountNotify"]').checked,
    folders: list ? [...list.querySelectorAll('[name="notifyFolder"]')].filter((c) => c.checked).map((c) => c.value) : null,
  } } };
}

function setPrefsStatus(text, isErr = false) {
  const el = $("#overlay .prefs-status");
  if (!el) return isErr && toast(text, true);
  el.hidden = !text;
  el.textContent = text;
  el.classList.toggle("err", isErr);
}

/** 把界面上的勾选状态、可用状态和提示改成 S.prefs（保存失败时回滚）。 */
function syncPrefsForm() {
  const overlay = $("#overlay");
  const form = overlay?.querySelector?.(".prefs-form");
  const p = currentPrefs();
  if (form) {
    const f = (name) => form.querySelector(`[name="${name}"]`);
    for (const k of ["notify", "dnd", "tray", "autostart"]) f(k).checked = !!p[k];
    f("notifyContent").value = p.notifyContent;
    f("dndStart").value = p.dndStart;
    f("dndEnd").value = p.dndEnd;
    f("notifyContent").disabled = f("dnd").disabled = !p.notify;
    f("dndStart").disabled = f("dndEnd").disabled = !(p.notify && p.dnd);
    window.MailBeaconDropdown?.refresh(f("notifyContent"));
  }
  for (const section of overlay?.querySelectorAll("[data-notify-account]") || []) {
    const id = section.dataset.notifyAccount, muted = (p.mutedAccounts || []).includes(id);
    const chosen = new Set(p.notifyFolders?.[id] || []);
    const toggle = section.querySelector('[name="accountNotify"]');
    toggle.checked = !muted;
    toggle.disabled = !p.notify;
    for (const c of section.querySelectorAll('[name="notifyFolder"]')) {
      c.checked = chosen.has(c.value);
      c.disabled = muted || !p.notify;
    }
    section.querySelector(".account-notify-hint").textContent = window.MailBeaconPrefs.accountNotifyHint(p, id);
  }
}

/**
 * 保存一处改动。保存按顺序一个接一个发出，每次都带上之前还在保存中的改动，
 * 所以快速连点不同的选项不会互相覆盖；最后一次保存的结果决定界面显示。
 */
async function savePrefs(changes) {
  const seq = ++S.prefsSeq;
  setPrefsStatus("正在保存…");
  if (!S.prefs) {
    // 设置还没读到：先读，免得用默认值覆盖已保存的静音和文件夹选择
    try {
      S.prefs = await invoke("get_prefs");
    } catch (e) {
      if (seq === S.prefsSeq) setPrefsStatus(`设置还没读取成功：${e}`, true);
      return;
    }
  }
  const next = window.MailBeaconPrefs.fromControls(S.prefsWanted || S.prefs, changes);
  S.prefsWanted = next;
  let error = null;
  const run = S.prefsQueue.then(async () => {
    try {
      S.prefs = await invoke("save_prefs", { prefs: next });
    } catch (e) {
      error = e;
    }
  });
  S.prefsQueue = run;
  await run;
  if (seq !== S.prefsSeq) return;
  S.prefsWanted = null;
  setPrefsStatus(error === null ? "已保存" : String(error), error !== null);
  syncPrefsForm();
}

// ---------------------------------------------------------------------------
// 收信规则
// ---------------------------------------------------------------------------
const R = () => window.MailBeaconRules;
/** POP3 只有收件箱，规则移不了也标不了服务器上的状态。 */
const rulesAccounts = () => S.accounts.filter((a) => a.receiveProtocol !== "pop3");
const accountOptions = (selected) => rulesAccounts().map((a) => `<option value="${esc(a.id)}"${a.id === selected ? " selected" : ""}>${esc(a.email)}</option>`).join("");

async function openRules({ draft = null } = {}) {
  S.rulesUi = { editing: draft ? { rule: draft, index: null } : null, saving: false };
  $("#overlay").innerHTML = `<div class="scrim" role="dialog" aria-modal="true" aria-labelledby="rules-title"><div class="sheet rules-sheet">
    <div class="sheet-head"><h3 id="rules-title">收信规则</h3><button class="icon-btn" data-action="overlay-close" aria-label="关闭">${icon("x")}</button></div>
    <div class="rules-body"></div></div></div>`;
  renderRules();
  try {
    S.rules = await invoke("get_rules");
  } catch (e) {
    fail(e);
  }
  if (!S.rulesUi) return;
  renderRules();
  if (draft) {
    loadRuleFolders(draft.accountId);
    $("#overlay #rule-name")?.focus();
  } else {
    $("#overlay .rules-body")?.querySelector("button:not(:disabled)")?.focus();
  }
}

/** 编辑器要列出账号的文件夹；还没读到的现在读，读到后补上默认的目标文件夹。 */
function loadRuleFolders(accountId) {
  if (Array.isArray(S.folders[accountId])) return;
  if (S.rulesUi) S.rulesUi.folderError = null;
  const rule = () => S.rulesUi?.editing?.rule;
  Promise.resolve(loadFolders(accountId, { report: true })).then(() => {
    const r = rule();
    if (!r || r.accountId !== accountId) return;
    if (r.actions.moveTo === "") r.actions.moveTo = R().defaultTarget(S.folders[accountId]);
    renderRules();
  }, (e) => {
    if (rule()?.accountId !== accountId || Array.isArray(S.folders[accountId])) return;
    S.rulesUi.folderError = String(e);
    renderRules();
  });
}

function renderRules() {
  const body = $("#overlay")?.querySelector(".rules-body");
  if (!body || !S.rulesUi) return;
  body.innerHTML = S.rulesUi.editing ? ruleEditor(S.rulesUi.editing.rule) : rulesList();
  // 列表重画后把焦点放回刚操作的那条规则（排序后它换了位置）
  const want = S.rulesUi.focus;
  if (want && !S.rulesUi.editing && !S.rulesUi.saving) {
    S.rulesUi.focus = null;
    const row = [...body.querySelectorAll("[data-rule-id]")].find((r) => r.dataset.ruleId === want.id);
    const pick = (sel) => { const el = row?.querySelector(sel); return el && !el.disabled ? el : null; };
    const other = { "rule-up": "rule-down", "rule-down": "rule-up" }[want.action];
    (pick(`[data-action="${want.action}"]`) || (other && pick(`[data-action="${other}"]`)) || pick("[data-rule-toggle]")
      || body.querySelector('[data-action="rule-new"]'))?.focus();
  }
}

function rulesList() {
  const busy = S.rulesUi.saving ? " disabled" : "";
  const accounts = rulesAccounts();
  const rows = S.rules.map((r, i) => `<li class="rule-row${r.enabled ? "" : " off"}" data-rule-index="${i}" data-rule-id="${esc(r.id)}">
      <input type="checkbox" data-rule-toggle${r.enabled ? " checked" : ""}${busy} aria-label="启用「${esc(r.name)}」">
      <div class="meta"><div class="n">${esc(r.name)}</div><div class="s">${esc(account(r.accountId)?.email || "")} · ${esc(R().describe(r, S.folders[r.accountId]))}</div></div>
      <button class="icon-btn" data-action="rule-up" aria-label="上移「${esc(r.name)}」"${i === 0 ? " disabled" : busy}>${icon("up")}</button>
      <button class="icon-btn" data-action="rule-down" aria-label="下移「${esc(r.name)}」"${i === S.rules.length - 1 ? " disabled" : busy}>${icon("down")}</button>
      <button class="secondary" data-action="rule-edit"${busy}>编辑</button>
      <button class="danger-link" data-action="rule-delete"${busy}>删除</button>
    </li>`).join("");
  return `<p class="hint">收件箱来了新邮件时，按从上到下的顺序检查每条规则，命中的都会执行（移动以第一条为准）。MailBeacon 运行时生效，关闭期间到达的邮件可以用「对收件箱最近 500 封执行」补上；从别的文件夹移回收件箱的邮件不会再被移走。POP3 账号不支持。被规则移走的邮件不会被「自动下载新邮件」保存。</p>
    <div class="rules-toolbar"><button class="primary" data-action="rule-new"${accounts.length ? busy : " disabled"}>新建规则</button>
      ${accounts.length ? `<span class="rules-run"><select id="rules-run-account" aria-label="要整理的邮箱">${accountOptions(S.acct)}</select><button class="secondary" data-action="rules-run"${busy}>对收件箱最近 500 封执行</button></span>` : `<span class="hint">需要先添加 IMAP 邮箱。</span>`}</div>
    ${S.rules.length ? `<ul class="rules-list">${rows}</ul>` : `<p class="rules-empty">还没有规则。可以按发件人、收件人、主题或附件，把邮件自动移到文件夹、标为已读或加星标。</p>`}`;
}

function ruleEditor(rule) {
  const r = R();
  const targets = r.moveTargets(S.folders[rule.accountId]);
  // 规则里的文件夹被删了也要显示出来，免得一保存就悄悄丢掉
  const loaded = Array.isArray(S.folders[rule.accountId]);
  const missing = loaded && rule.actions.moveTo && !targets.some((f) => f.path === rule.actions.moveTo) ? [{ path: rule.actions.moveTo, name: `${rule.actions.moveTo}（找不到）` }] : [];
  const moving = rule.actions.moveTo !== null && rule.actions.moveTo !== undefined;
  const opt = (list, value) => list.map((o) => `<option value="${o.key}"${o.key === value ? " selected" : ""}>${o.label}</option>`).join("");
  const conds = rule.conditions.map((c, i) => `<div class="rule-cond" data-cond-index="${i}">
      <select name="field" aria-label="条件 ${i + 1} 的字段">${opt(r.FIELDS, c.field)}</select>
      <select name="op" aria-label="条件 ${i + 1} 的方式">${opt(c.field === "attachment" ? r.ATTACHMENT_OPS : r.OPS, c.op)}</select>
      ${c.field === "attachment" ? "" : `<input name="value" value="${esc(c.value)}" placeholder="${c.field === "subject" ? "例如：发票" : "例如：@bank.com"}" aria-label="条件 ${i + 1} 的内容" spellcheck="false" autocomplete="off">`}
      <button class="icon-btn" type="button" data-action="rule-cond-remove" aria-label="删除条件 ${i + 1}"${rule.conditions.length === 1 ? " disabled" : ""}>${icon("x")}</button>
    </div>`).join("");
  const a = rule.actions;
  return `<form class="rule-form" aria-label="${S.rulesUi.editing.index === null ? "新建规则" : "编辑规则"}">
    <div class="rule-grid">
      <label for="rule-name">名称</label><input id="rule-name" name="name" value="${esc(rule.name)}" placeholder="例如：银行账单" autocomplete="off">
      <label for="rule-account">邮箱账号</label><select id="rule-account" name="accountId">${accountOptions(rule.accountId)}</select>
      <label for="rule-match">满足</label><select id="rule-match" name="matchAll"><option value="all"${rule.matchAll ? " selected" : ""}>以下全部条件</option><option value="any"${rule.matchAll ? "" : " selected"}>以下任意一个条件</option></select>
    </div>
    <div class="rule-conds">${conds}</div>
    <button class="link" type="button" data-action="rule-cond-add"${rule.conditions.length >= 20 ? " disabled" : ""}>${icon("plus")}添加条件</button>
    <fieldset class="rule-actions"><legend>然后</legend>
      <label class="prefs-toggle"><input type="checkbox" name="move"${moving ? " checked" : ""}>移到文件夹</label>
      <select name="moveTo" aria-label="目标文件夹"${moving ? "" : " disabled"}>${!loaded ? `<option value="${esc(a.moveTo || "")}">${S.rulesUi.folderError ? "文件夹列表读取失败" : "正在读取文件夹…"}</option>` : ""}${loaded && !a.moveTo ? `<option value="" selected>请选择文件夹</option>` : ""}${loaded ? [...targets, ...missing].map((f) => `<option value="${esc(f.path)}"${f.path === a.moveTo ? " selected" : ""}>${esc(f.name || f.path)}</option>`).join("") : ""}</select>
      ${!loaded && S.rulesUi.folderError ? `<p class="hint err rule-folder-error">读取文件夹失败：${esc(S.rulesUi.folderError)} <button class="link" type="button" data-action="rule-folders-retry">重试</button></p>` : ""}
      <label class="prefs-toggle"><input type="checkbox" name="markRead"${a.markRead ? " checked" : ""}>标为已读</label>
      <label class="prefs-toggle"><input type="checkbox" name="flag"${a.flag ? " checked" : ""}>加星标</label>
    </fieldset>
    <label class="prefs-toggle"><input type="checkbox" name="enabled"${rule.enabled ? " checked" : ""}>启用这条规则</label>
    <p class="rule-problem" role="alert"></p>
    <div class="rule-buttons"><button class="secondary" type="button" data-action="rule-cancel">取消</button><button class="primary" type="submit"${S.rulesUi.saving ? " disabled" : ""}>保存规则</button></div>
  </form>`;
}

/** 从编辑表单读出规则（保留原 id）。 */
function readRuleForm(form) {
  const prev = S.rulesUi.editing.rule;
  const f = (name) => form.querySelector(`[name="${name}"]`);
  const moveOn = f("move").checked;
  return {
    ...prev,
    name: f("name").value,
    accountId: f("accountId").value,
    matchAll: f("matchAll").value !== "any",
    enabled: f("enabled").checked,
    conditions: [...form.querySelectorAll(".rule-cond")].map((row) => {
      const field = row.querySelector('[name="field"]').value;
      return { field, op: row.querySelector('[name="op"]').value, value: field === "attachment" ? "" : row.querySelector('[name="value"]')?.value || "" };
    }),
    // 勾了「移到文件夹」但还没选到（例如文件夹还在读取）时是 ""，保存前会提示
    actions: { moveTo: moveOn ? f("moveTo").value : null, markRead: f("markRead").checked, flag: f("flag").checked },
  };
}

/** 表单里换了字段、账号或「移到文件夹」后重画编辑器，让可选项跟着变。 */
function ruleFormChanged(form, target) {
  const rule = readRuleForm(form);
  const conditionIndex = target.name === "field" ? +target.closest(".rule-cond").dataset.condIndex : null;
  if (target.name === "field") {
    const c = rule.conditions[conditionIndex];
    if (c.field === "attachment" || !R().OPS.some((o) => o.key === c.op)) c.op = "contains";
  }
  // 换了账号：原来的目标文件夹属于旧账号，换成新账号的默认文件夹（还在读取时先留空）
  if (target.name === "accountId" && rule.actions.moveTo !== null) {
    rule.actions.moveTo = R().defaultTarget(S.folders[rule.accountId]);
  }
  // 刚勾上「移到文件夹」：下拉框之前是禁用的，它的值不算用户的选择
  if (target.name === "move" && target.checked && S.rulesUi.editing.rule.actions.moveTo === null) {
    rule.actions.moveTo = R().defaultTarget(S.folders[rule.accountId]);
  }
  S.rulesUi.editing.rule = rule;
  if (target.name === "accountId") loadRuleFolders(rule.accountId);
  if (["field", "accountId", "move"].includes(target.name)) {
    renderRules();
    const row = conditionIndex === null ? "" : `.rule-cond[data-cond-index="${conditionIndex}"] `;
    $(`#overlay .rule-form ${row}[name="${target.name}"]`)?.focus();
  }
}

function editRuleConditions(form, change) {
  const rule = readRuleForm(form);
  change(rule.conditions);
  S.rulesUi.editing.rule = rule;
  renderRules();
}

/** 保存整套规则；成功时用服务器规范化后的结果。 */
async function saveRules(list) {
  if (S.rulesUi) S.rulesUi.saving = true;
  renderRules();
  try {
    S.rules = await invoke("save_rules", { rules: list });
    return true;
  } catch (e) {
    fail(e);
    return false;
  } finally {
    if (S.rulesUi) S.rulesUi.saving = false;
    renderRules();
  }
}

async function submitRule(form) {
  const rule = readRuleForm(form);
  const problem = R().problem(rule) || (rule.actions.moveTo === "" ? "请选择要移到的文件夹" : "");
  if (problem) {
    const el = $(".rule-problem", form);
    el.textContent = problem;
    return;
  }
  S.rulesUi.editing.rule = rule;
  const { index } = S.rulesUi.editing;
  const list = index === null ? [...S.rules, rule] : S.rules.map((r, i) => (i === index ? rule : r));
  if (await saveRules(list)) {
    S.rulesUi.editing = null;
    renderRules();
  }
}

async function runRulesNow(accountId) {
  if (S.rulesUi) S.rulesUi.saving = true;
  renderRules();
  try {
    const report = await invoke("run_rules", { accountId });
    const summary = report.matched ? `已处理 ${report.matched} 封邮件，其中 ${report.moved} 封移到了其他文件夹` : "收件箱最近的邮件都没有命中规则";
    toast(report.errors?.length ? `${summary}；${report.errors[0]}` : summary, !!report.errors?.length);
    if (report.matched) {
      // 收件箱和正在看的视图都刷新；正在看的邮件被移走了就关掉
      const viewing = S.acct === accountId || S.acct === "all";
      for (const folder of new Set(["INBOX", ...(viewing ? [S.folder] : [])])) await loadList(accountId, folder, { force: true });
      if (S.sel?.accountId === accountId && !findMsg(S.sel)) { S.sel = null; S.msg = null; render(); }
    }
  } catch (e) {
    fail(e);
  } finally {
    if (S.rulesUi) S.rulesUi.saving = false;
    renderRules();
  }
}

function onRulesClick(btn) {
  const row = btn.closest("[data-rule-index]");
  const index = row ? +row.dataset.ruleIndex : null;
  const form = btn.closest(".rule-form");
  switch (btn.dataset.action) {
    case "rule-new": {
      const accountId = rulesAccounts().some((a) => a.id === S.acct) ? S.acct : rulesAccounts()[0]?.id;
      S.rulesUi.editing = { rule: R().blankRule(accountId), index: null };
      loadRuleFolders(accountId);
      renderRules();
      return $("#overlay #rule-name")?.focus();
    }
    case "rule-edit":
      S.rulesUi.editing = { rule: structuredClone(S.rules[index]), index };
      loadRuleFolders(S.rules[index].accountId);
      renderRules();
      return $("#overlay #rule-name")?.focus();
    case "rule-cancel":
      S.rulesUi.editing = null;
      return renderRules();
    case "rule-delete":
      if (!confirm(`删除规则「${S.rules[index].name}」？`)) return;
      S.rulesUi.focus = { id: "", action: "rule-new" };
      return saveRules(S.rules.filter((_, i) => i !== index));
    case "rule-up":
    case "rule-down":
      S.rulesUi.focus = { id: S.rules[index].id, action: btn.dataset.action };
      return saveRules(R().move(S.rules, index, btn.dataset.action === "rule-up" ? -1 : 1));
    case "rule-cond-add":
      return editRuleConditions(form, (c) => c.push({ field: "subject", op: "contains", value: "" }));
    case "rule-cond-remove":
      return editRuleConditions(form, (c) => c.splice(+btn.closest(".rule-cond").dataset.condIndex, 1));
    case "rules-run":
      return runRulesNow($("#rules-run-account").value);
    case "rule-folders-retry":
      return loadRuleFolders(S.rulesUi.editing.rule.accountId);
  }
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
    <p class="hint">中断后已保存的文件会保留；再次选择同一目录可继续核验并保存，不重复保存已核验的邮件。关闭 MailBeacon 会中断本次下载，重新打开后需手动开始。</p>
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
    $(".bulk-download-saved-rule", section).textContent = modeLabel[saved.filterMode]
      ? `该账号已保存${modeLabel[saved.filterMode]}；${autoDownloadRuleSummary(saved)}`
      : "该账号已保存的设置未启用黑白名单，继承后仍不做过滤。";
    const taskRule = $(".bulk-download-task-rule", section);
    taskRule.hidden = !runtime || runtime.state === "idle";
    taskRule.textContent = runtime?.inheritFilters
      ? `本次任务：继承${modeLabel[runtime.filterMode] || "规则（未启用黑白名单，不做过滤）"}${modeLabel[runtime.filterMode] ? `；${autoDownloadScopeLabel(runtime.keywordScope)}` : ""}，使用任务开始时的已保存规则。`
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

// ---------------------------------------------------------------------------
// 邮件缓存：总量、预先下载的单封上限、后台预热进度
// ---------------------------------------------------------------------------

const BC = () => window.MailBeaconBodyCache;
// 每个账号收到的 bodyCache 事件数：命令的回复比事件旧时不覆盖
const bodyCacheSeq = new Map();

/** 用命令的回复更新状态；请求期间来过更新的事件就以事件为准 */
async function fetchBodyCache(accountId, cmd) {
  const seq = bodyCacheSeq.get(accountId) || 0;
  const p = await invoke(cmd, { accountId });
  if ((bodyCacheSeq.get(accountId) || 0) === seq) S.bodyCache[accountId] = p;
}

function bodyCacheForm(a) {
  if (a.receiveProtocol === "pop3") return "";
  const c = BC().settingsOf(a), id = esc(a.id);
  return `<form class="body-cache-form" data-body-cache-account="${id}" aria-label="${esc(a.email)} 的邮件缓存设置">
    <div class="form-subtitle">邮件缓存</div>
    <label class="auto-download-toggle"><input type="checkbox" name="enabled" ${c.enabled ? "checked" : ""}>后台预先缓存收件箱邮件</label>
    <div class="body-cache-sizes">
      <label for="cache-limit-${id}">缓存总量</label><span class="unit-input"><input type="number" id="cache-limit-${id}" name="limitMb" min="64" max="102400" step="1" value="${c.limitMb}" inputmode="numeric">MB</span>
      <label for="cache-max-${id}">单封上限</label><span class="unit-input"><input type="number" id="cache-max-${id}" name="maxMailMb" min="1" max="32" step="1" value="${c.maxMailMb}" inputmode="numeric">MB</span>
    </div>
    <p class="hint">打开过的邮件和预先下载的邮件都存在本机，重启后再打开不用重新下载；超过总量时删掉最久没看的。预先下载只缓存收件箱里不超过单封上限的邮件，从最新的开始，读信时自动暂停。</p>
    <progress class="body-cache-bar" hidden></progress>
    <div class="auto-download-actions"><p class="body-cache-status" role="status"></p>
      <button class="secondary" type="button" data-action="refresh-body-cache">立即更新缓存</button>
      <button class="secondary" type="submit">保存设置</button></div>
    <div class="archive-dirs">
      <div class="form-subtitle">已下载的邮件</div>
      <p class="hint">自动下载、全量下载存下来的邮件直接当缓存读，不用再从服务器下载，也不占缓存总量。只读使用，不会修改或删除这些文件。</p>
      <ul class="archive-list"></ul>
      <button class="secondary" type="button" data-action="add-archive-dir">添加已下载目录…</button>
    </div>
  </form>`;
}

function renderArchiveDirs(accountId) {
  const overlay = $("#overlay");
  if (!overlay) return;
  for (const form of overlay.querySelectorAll(".body-cache-form")) {
    const id = form.dataset.bodyCacheAccount;
    if (accountId && accountId !== id) continue;
    const rows = BC().archiveRows(S.archives[id]);
    $(".archive-list", form).innerHTML = S.archiveErrors[id] ? `<li class="archive-empty err">读取失败：${esc(S.archiveErrors[id])} <button type="button" class="link" data-action="retry-archive-dirs" data-account-id="${esc(id)}">重试</button>${rows.length ? "（上次的目录信息已保留）" : ""}</li>` : rows.length
      ? rows.map((r) => `<li><span class="archive-kind">${esc(r.label)}</span><span class="archive-path" title="${esc(r.path)}">${esc(r.path)}</span>${r.note ? `<span class="archive-note">${esc(r.note)}</span>` : ""}${r.removable ? `<button class="link" type="button" data-action="remove-archive-dir" data-path="${esc(r.path)}">移除</button>` : ""}</li>`).join("")
      : `<li class="archive-empty">${S.archives[id] ? "还没有。做过全量下载后会自动加进来。" : "正在读取…"}</li>`;
  }
}

// 每个账号最近一次目录列表请求的序号：慢的旧回复不覆盖新结果
const archiveSeq = new Map();

/** 发一个返回目录列表的命令，只有它还是最新的一次请求时才采用 */
async function updateArchives(accountId, cmd, input) {
  const seq = (archiveSeq.get(accountId) || 0) + 1;
  archiveSeq.set(accountId, seq);
  const list = await invoke(cmd, { accountId, ...input });
  if (archiveSeq.get(accountId) !== seq) return false;
  S.archives[accountId] = list;
  delete S.archiveErrors[accountId];
  renderArchiveDirs(accountId);
  return true;
}

async function loadArchives(accountId) {
  try {
    await updateArchives(accountId, "archive_status");
  } catch (e) {
    console.warn("读取下载目录失败", e);
    S.archiveErrors[accountId] = String(e);
    renderArchiveDirs(accountId);
  }
}

async function addArchiveDir(accountId) {
  try {
    const directory = await invoke("choose_download_directory", { directory: null });
    if (directory == null) return;
    await updateArchives(accountId, "add_archive_dir", { directory });
  } catch (e) {
    toast(String(e), true);
  }
}

async function removeArchiveDir(accountId, path) {
  try {
    await updateArchives(accountId, "remove_archive_dir", { path });
    toast("已不再当缓存使用，文件没有删除");
  } catch (e) {
    toast(String(e), true);
  }
}

function renderBodyCacheStatus(accountId) {
  const overlay = $("#overlay");
  if (!overlay) return;
  for (const form of overlay.querySelectorAll(".body-cache-form")) {
    const id = form.dataset.bodyCacheAccount;
    if (accountId && accountId !== id) continue;
    const p = S.bodyCache[id];
    const status = $(".body-cache-status", form);
    status.textContent = BC().statusText(p, BC().settingsOf(account(id)));
    status.classList.toggle("err", p?.state === "error");
    const b = BC().bar(p), bar = $(".body-cache-bar", form);
    bar.hidden = !b.visible;
    bar.max = b.max;
    if (b.value == null) bar.removeAttribute("value");
    else bar.value = b.value;
    $('[data-action="refresh-body-cache"]', form).disabled = !BC().canRefresh(p);
  }
}

/** 设置页上显示了缓存设置的账号，读一次当前占用和进度（之后靠 bodyCache 事件更新） */
async function loadBodyCacheStatus() {
  const overlay = $("#overlay");
  const ids = [...(overlay?.querySelectorAll(".body-cache-form") || [])].map((f) => f.dataset.bodyCacheAccount);
  for (const accountId of ids) {
    try {
      await fetchBodyCache(accountId, "body_cache_status");
    } catch (e) {
      console.warn("读取缓存状态失败", e);
    }
    renderBodyCacheStatus(accountId);
    // 不等它：下载目录卡住（网盘、睡着的移动硬盘）时别挡住后面账号的缓存状态
    renderArchiveDirs(accountId);
    loadArchives(accountId);
  }
}

async function saveBodyCache(form) {
  const button = $('button[type="submit"]', form);
  if (button.disabled) return;
  const status = $(".body-cache-status", form);
  const f = form.elements;
  const { settings, error } = BC().fromForm({ enabled: f.enabled.checked, limitMb: f.limitMb.value, maxMailMb: f.maxMailMb.value });
  if (error) {
    status.textContent = error;
    status.classList.toggle("err", true);
    return;
  }
  button.disabled = true;
  try {
    const updated = await invoke("set_body_cache", { accountId: form.dataset.bodyCacheAccount, settings });
    const old = account(updated.id);
    if (old) old.bodyCache = updated.bodyCache;
    toast("缓存设置已保存");
  } catch (e) {
    status.textContent = String(e);
    status.classList.toggle("err", true);
  } finally {
    button.disabled = false;
  }
}

async function refreshBodyCache(accountId) {
  try {
    await fetchBodyCache(accountId, "refresh_body_cache");
  } catch (e) {
    toast(String(e), true);
  }
  renderBodyCacheStatus(accountId);
}

const AUTO_DOWNLOAD_KEYWORD_FIELDS = [
  ["subject", "邮件主题"], ["sender", "发送人"], ["recipient", "收件人"], ["body", "正文"],
];

function autoDownloadScopeLabel(scope) {
  return scope === "fields" ? "按字段设置（任意一个字段命中即可）" : "任意命中（主题、发件人、正文）";
}

function autoDownloadFilterHint(scope) {
  const matching = scope === "fields"
    ? "各字段关键词只匹配对应内容，任意一个字段命中即视为命中，空字段忽略。发送人匹配名称或地址；收件人匹配邮件头中收件人、抄送和密送的名称或地址。四个字段合计最多 100 个关键词。"
    : "任一关键词包含于主题、发件人名称或地址、正文即视为命中。最多 100 个关键词。";
  return matching + "每行一个，每个最多 200 个字符；不区分大小写，不匹配附件。切换匹配方式或选择不过滤时，已填写的关键词会保留，只有当前启用的规则生效。";
}

function autoDownloadRuleSummary(config) {
  const preview = words => words.slice(0, 3).map(word => {
    const chars = [...word];
    return chars.length > 60 ? chars.slice(0, 60).join("") + "…" : word;
  }).join("、") + (words.length > 3 ? ` 等 ${words.length} 个` : "");
  const keywords = config.keywordScope === "fields"
    ? AUTO_DOWNLOAD_KEYWORD_FIELDS.filter(([field]) => config.fieldKeywords[field].length)
      .map(([field, label]) => `${label}：${preview(config.fieldKeywords[field])}`).join("；")
    : `关键词：${preview(config.keywords)}`;
  return `${autoDownloadScopeLabel(config.keywordScope)}；${keywords || "尚未填写关键词"}`;
}

function autoDownloadSettings(a) {
  const config = a?.autoDownload;
  return {
    enabled: config?.enabled === true,
    directory: config?.directory || "",
    filterMode: ["whitelist", "blacklist"].includes(config?.filterMode) ? config.filterMode : "none",
    keywords: Array.isArray(config?.keywords) ? config.keywords.filter(word => typeof word === "string") : [],
    keywordScope: config?.keywordScope === "fields" ? "fields" : "any",
    fieldKeywords: Object.fromEntries(AUTO_DOWNLOAD_KEYWORD_FIELDS.map(([field]) => [field,
      Array.isArray(config?.fieldKeywords?.[field]) ? config.fieldKeywords[field].filter(word => typeof word === "string") : [],
    ])),
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
    <label class="auto-download-path-label" for="download-scope-${esc(a.id)}">匹配方式</label>
    <select class="auto-download-filter" id="download-scope-${esc(a.id)}" name="keywordScope" aria-describedby="download-filter-hint-${esc(a.id)}" ${config.filterMode === "none" ? "disabled" : ""}>
      <option value="any" ${config.keywordScope === "any" ? "selected" : ""}>任意命中（原模式）</option>
      <option value="fields" ${config.keywordScope === "fields" ? "selected" : ""}>按字段设置</option>
    </select>
    <div class="auto-download-any-keywords" ${config.keywordScope !== "any" ? "hidden" : ""}>
      <label class="auto-download-path-label" for="download-keywords-${esc(a.id)}">关键词（每行一个）</label>
      <textarea class="auto-download-keywords" id="download-keywords-${esc(a.id)}" name="keywords" rows="3" placeholder="例如：GitHub&#10;发票" spellcheck="false" aria-describedby="download-filter-hint-${esc(a.id)}" ${config.filterMode === "none" || config.keywordScope !== "any" ? "disabled" : ""}>${esc(config.keywords.join("\n"))}</textarea>
    </div>
    <div class="auto-download-field-keywords" ${config.keywordScope !== "fields" ? "hidden" : ""}>
      ${AUTO_DOWNLOAD_KEYWORD_FIELDS.map(([field, label]) => `<div class="auto-download-keyword-field">
        <label class="auto-download-path-label" for="download-${field}-keywords-${esc(a.id)}">${label}关键词</label>
        <textarea class="auto-download-keywords" id="download-${field}-keywords-${esc(a.id)}" name="${field}Keywords" rows="3" placeholder="每行一个，留空则不匹配此字段" spellcheck="false" aria-describedby="download-filter-hint-${esc(a.id)}" ${config.filterMode === "none" || config.keywordScope !== "fields" ? "disabled" : ""}>${esc(config.fieldKeywords[field].join("\n"))}</textarea>
      </div>`).join("")}
    </div>
    <p class="auto-download-filter-hint hint" id="download-filter-hint-${esc(a.id)}">${autoDownloadFilterHint(config.keywordScope)}</p>
    <p class="hint">仅将启用后新到收件箱的邮件保存为完整 .eml（含附件），不下载历史邮件。MailBeacon 运行时自动保存，重新打开后补下关闭期间的新邮件。更改路径或有效过滤规则，仅影响保存后的新邮件，不追溯旧邮件。</p>
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

function updateAutoDownloadKeywordControls(form) {
  const scope = $('[name="keywordScope"]', form).value;
  const inactive = form._autoDownloadBusy === true || $('[name="filterMode"]', form).value === "none";
  $('[name="keywordScope"]', form).disabled = inactive;
  $('[name="keywords"]', form).disabled = inactive || scope !== "any";
  for (const [field] of AUTO_DOWNLOAD_KEYWORD_FIELDS) $('[name="' + field + 'Keywords"]', form).disabled = inactive || scope !== "fields";
  $(".auto-download-any-keywords", form).hidden = scope !== "any";
  $(".auto-download-field-keywords", form).hidden = scope !== "fields";
  $(".auto-download-filter-hint", form).textContent = autoDownloadFilterHint(scope);
}

function autoDownloadChanged(form) {
  updateAutoDownloadKeywordControls(form);
  const status = $(".auto-download-status", form);
  status.textContent = "更改尚未保存";
  status.hidden = false;
  status.classList.remove("err");
}

function autoDownloadBusy(form, busy) {
  form._autoDownloadBusy = busy;
  for (const input of form.querySelectorAll("input, select, textarea, button")) input.disabled = busy;
  updateAutoDownloadKeywordControls(form);
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
    const keywordScope = $('[name="keywordScope"]', form).value;
    if (!["any", "fields"].includes(keywordScope)) throw new Error("请选择有效的关键词匹配方式");
    // 两种模式的草稿都保存，但仅要求当前启用的模式填写关键词。
    const keywords = autoDownloadKeywords($('[name="keywords"]', form).value, keywordScope === "any" ? filterMode : "none");
    const fieldKeywords = Object.fromEntries(AUTO_DOWNLOAD_KEYWORD_FIELDS.map(([field]) => [field,
      autoDownloadKeywords($('[name="' + field + 'Keywords"]', form).value, "none"),
    ]));
    const fieldCount = Object.values(fieldKeywords).reduce((count, words) => count + words.length, 0);
    if (fieldCount > 100) throw new Error("四个字段合计最多可设置 100 个关键词");
    if (keywordScope === "fields" && filterMode !== "none" && !fieldCount) throw new Error("请至少在一个字段填写关键词，或选择不过滤");
    if (!["none", "whitelist", "blacklist"].includes(filterMode)) throw new Error("请选择有效的关键词过滤模式");
    const updated = await invoke("save_auto_download", { accountId, enabled, directory, filterMode, keywords, keywordScope, fieldKeywords });
    const old = account(accountId);
    // 其他设置可能同时保存，只合并本次更改的字段。
    if (old) old.autoDownload = updated.autoDownload;
    const saved = autoDownloadSettings(updated);
    $('[name="enabled"]', form).checked = saved.enabled;
    $('[name="directory"]', form).value = saved.directory;
    $('[name="filterMode"]', form).value = saved.filterMode;
    $('[name="keywords"]', form).value = saved.keywords.join("\n");
    $('[name="keywordScope"]', form).value = saved.keywordScope;
    for (const name of ["filterMode", "keywordScope"]) window.MailBeaconDropdown?.refresh($(`[name="${name}"]`, form));
    for (const [field] of AUTO_DOWNLOAD_KEYWORD_FIELDS) $('[name="' + field + 'Keywords"]', form).value = saved.fieldKeywords[field].join("\n");
    status.textContent = saved.enabled ? "自动下载已开启，设置已保存" : "自动下载已关闭，设置已保存";
    // 自动下载目录也是缓存来源
    if (S.archives[accountId]) loadArchives(accountId);
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
      <h2>欢迎使用 MailBeacon</h2>
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
  const att = e.target.closest?.("[data-att]");
  if (att) {
    e.preventDefault();
    showAttachmentMenu(att);
    return;
  }
  const row = e.target.closest?.(".row[data-open]");
  if (!row) return;
  e.preventDefault();
  // 不打开邮件，避免右键操作把未读邮件变成已读。
  showMessageMenu(row, rowSelection(row), { x: e.clientX, y: e.clientY });
});

// 双击附件直接用默认程序打开（右键才弹菜单）
document.addEventListener("dblclick", (e) => {
  const att = e.target.closest?.("[data-att]");
  if (!att) return;
  e.preventDefault();
  openAttachment(JSON.parse(att.dataset.attachmentSource), att);
});

document.addEventListener("click", async (e) => {
  const t = e.target;
  if (t.closest("[data-close]")) return closeOverlay();
  if (t.classList.contains("scrim") && !t.querySelector(".sheet .compose-body")) return closeOverlay();

  // 写信正文里的链接也走系统打开器，避免在 contenteditable 中导航离开窗口。
  const composeBody = t.closest?.("#c-body");
  const composeLink = t.closest?.("a[href]");
  if (composeBody && composeLink) {
    e.preventDefault();
    return invoke("open_link", { url: composeLink.href }).catch(fail);
  }

  const kind = t.closest(".kind[data-kind]");
  if (kind) {
    formKind = kind.dataset.kind;
    const wrap = kind.closest("#acct-form-wrap");
    const email = $("#acct-form", wrap)?.email.value;
    const name = $("#acct-form", wrap)?.name.value;
    wrap.innerHTML = accountForm();
    if (email) $("#acct-form", wrap).email.value = email;
    if (name) $("#acct-form", wrap).name.value = name;
    return;
  }
  const rm = t.closest("[data-remove]");
  if (rm) {
    const a = account(rm.dataset.remove);
    const pending = S.outbox.filter((i) => i.accountId === a.id).length;
    if (!confirm(`移除 ${a.email}？\n只会从 MailBeacon 中移除，不影响服务器上的邮件。${pending ? `\n还有 ${pending} 封定时邮件没发出，会一起删除。` : ""}`)) return;
    try {
      await invoke("remove_account", { accountId: a.id });
      S.accounts = S.accounts.filter((x) => x.id !== a.id);
      for (const map of [S.lists, S.results]) for (const k of [...map.keys()]) if (k.startsWith(a.id + "|")) map.delete(k);
      delete S.live[a.id];
      delete S.autoDownloads[a.id];
      delete S.bulkDownloads[a.id];
      S.bulkPending.delete(a.id);
      S.bulkEventSeq.delete(a.id);
      S.bulkErrors.delete(a.id);
      S.bulkDirectories.delete(a.id);
      S.bulkInheritFilters.delete(a.id);
      S.settingsExpanded.delete(a.id);
      forgetAccountPrefs(a.id);
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
  const tplBtn = t.closest("[data-template-id]");
  if (tplBtn) return insertTemplate(tplBtn.closest(".compose-sheet"), tplBtn.dataset.templateId);
  const tplDel = t.closest("[data-template-delete]");
  if (tplDel) return deleteTemplate(tplDel.closest(".compose-sheet"), tplDel.dataset.templateDelete);
  const quick = t.closest("[data-send-at]");
  if (quick) return scheduleCompose(quick.closest(".compose-sheet"), +quick.dataset.sendAt);
  const selectRow = t.closest("[data-select-mail]");
  if (selectRow) { e.preventDefault(); return selectMailRow(selectRow.dataset.selectMail,e.shiftKey); }
  const openRow = t.closest("[data-open]");
  if (openRow) {
    if (e.metaKey || e.ctrlKey || e.shiftKey) { e.preventDefault(); const m=findMsg(rowSelection(openRow)); return m && selectMailRow(MS().key(m),e.shiftKey); }
    return openMessage(rowSelection(openRow));
  }
  const threadItem = t.closest("[data-thread-open]");
  if (threadItem) {
    const [accountId, uid, ...folder] = threadItem.dataset.threadOpen.split("|");
    return openMessage({ accountId, uid: +uid, folder: folder.join("|") });
  }
  const goBtn = t.closest("[data-go]");
  if (goBtn) return go(goBtn.dataset.go);
  const sw = t.closest("[data-switch]");
  if (sw) return go("INBOX", sw.dataset.switch);
  const att = t.closest("[data-att]");
  if (att) {
    // 鼠标单击只聚焦；双击见 dblclick，右键见 contextmenu。
    // 键盘回车/空格激活按钮时 e.detail 为 0，按“打开”处理。
    if (e.detail === 0) openAttachment(JSON.parse(att.dataset.attachmentSource), att);
    return;
  }
  const link = t.closest("[data-link]");
  if (link) {
    e.preventDefault();
    return invoke("open_link", { url: link.dataset.link }).catch(fail);
  }

  const contactRow = t.closest("[data-contact-id]");
  if (contactRow && S.contactsUi) {
    S.contactsUi.detail = { contact: Number(contactRow.dataset.contactId) };
    renderContactsList();
    return renderContactDetail();
  }
  const groupRow = t.closest("[data-group-id]");
  if (groupRow && S.contactsUi) {
    S.contactsUi.detail = { group: Number(groupRow.dataset.groupId) };
    renderContactsList();
    return renderContactDetail();
  }
  const btn = t.closest("[data-action]");
  if (!btn || btn.disabled) return;
  const m = findMsg(S.sel);
  if (btn.dataset.action.startsWith("bulk-") && btn.dataset.action!=="bulk-clear" && btn.dataset.action!=="bulk-all") return bulkAction(btn.dataset.action.slice(5),btn);
  switch (btn.dataset.action) {
    case "retry-archive-dirs": return loadArchives(btn.dataset.accountId);
    case "dismiss-toast": $("#toast").hidden=true; return;
    case "delete-cancel": return finishDeleteConfirmation(false);
    case "delete-confirm": return finishDeleteConfirmation(true);
    case "bulk-clear": clearBatch(); renderList(); $(".rows")?.focus(); return;
    case "bulk-all": S.batch=MS().all(selectionGroups()); return renderList();
    case "operation-details": return showOperationHistory();
    case "operation-undo": return undoLastOperation();
    case "operation-refresh": return refreshOperationSources(S.lastOperation?.items.map(m=>m.source)||[],S.lastRequest?.target);
    case "operation-retry": { const req=S.lastRequest, items=MO().retryItems(S.lastOperation); return req && runMailOperation(req.action,items,{value:req.value,target:req.target}); }
    case "toggle-settings-account": return toggleSettingsAccount(btn.closest(".settings-account"));
    case "retry-notify-folders": return loadNotifyFolders(btn.closest("[data-notify-account]").dataset.notifyAccount);
    case "refresh-body-cache": return refreshBodyCache(btn.closest(".body-cache-form").dataset.bodyCacheAccount);
    case "add-archive-dir": return addArchiveDir(btn.closest(".body-cache-form").dataset.bodyCacheAccount);
    case "remove-archive-dir": return removeArchiveDir(btn.closest(".body-cache-form").dataset.bodyCacheAccount, btn.dataset.path);
    case "choose-download-directory": return chooseAutoDownloadDirectory(btn.closest(".auto-download-form"));
    case "start-bulk-download": return startBulkDownload(btn.closest(".bulk-download-section"));
    case "cancel-bulk-download": return cancelBulkDownload(btn.closest(".bulk-download-section"));
    case "account-menu": return accountMenu(btn);
    case "compose": return openCompose();
    case "compose-close": return requestComposeClose();
    case "compose-continue": return hideComposeConfirmation(btn.closest(".sheet"));
    case "compose-discard": {
      const box = btn.closest(".compose-confirm");
      if (!box.hidden && box.dataset.kind === "discard") return discardCompose(btn.closest(".sheet"));
      return;
    }
    case "compose-save-draft": return saveDraftAndClose(btn.closest(".sheet"));
    case "suggest-pick": return pickSuggestion(btn.closest(".compose-sheet"), Number(btn.dataset.index));
    case "suggest-forget": return forgetSuggestion(btn.closest(".compose-sheet"), Number(btn.dataset.index));
    case "pick-contacts": return openContactPicker(btn.closest(".compose-sheet"));
    case "picker-close": return closeContactPicker(btn.closest(".compose-sheet"));
    case "picker-add": return addPickedRecipients(btn.closest(".compose-sheet"), btn.dataset.target);
    case "contacts": return openContacts();
    case "rules": return openRules();
    case "rule-new": case "rule-edit": case "rule-cancel": case "rule-delete": case "rule-up": case "rule-down":
    case "rule-cond-add": case "rule-cond-remove": case "rules-run": case "rule-folders-retry": return onRulesClick(btn);
    case "contact-new": S.contactsUi.detail = { contact: null }; renderContactsList(); return renderContactDetail();
    case "contact-save": return saveContact();
    case "contact-delete": btn.hidden = true; $('[data-action="contact-delete-confirm"]').hidden = false; return;
    case "contact-delete-confirm": return deleteContact();
    case "group-new": S.contactsUi.detail = { group: null }; renderContactsList(); return renderContactDetail();
    case "group-save": return saveGroup();
    case "group-delete": return deleteGroup();
    case "contacts-import": return importContacts();
    case "contacts-export": return exportContacts();
    case "fmt": return runFormat(btn.closest(".compose-sheet"), btn.dataset.cmd);
    case "popover-ok": return confirmComposePopover(btn.closest(".compose-sheet"));
    case "popover-cancel": {
      const sheet = btn.closest(".compose-sheet");
      hideComposePopover(sheet);
      return restoreSelection(sheet);
    }
    case "add-attachment": return pickComposeAttachments(btn.closest(".sheet"));
    case "remove-attachment": {
      const sheet = btn.closest(".sheet");
      const removed = (sheet._attachments || []).find((a) => window.MailPulseDraft.attachmentKey(a) === btn.dataset.key);
      if (removed?.kind === "file") invoke("release_attachments", { tokens: [removed.token] }).catch(() => {});
      sheet._attachments = window.MailPulseDraft.removeAttachment(sheet._attachments || [], btn.dataset.key);
      renderComposeAttachments(sheet);
      return composeChanged(sheet);
    }
    case "delete-local-draft": {
      if (!m) return;
      const ref = { accountId: m.accountId, folder: m.folder, uid: m.uid, uidValidity: null, messageId: "" };
      try {
        await invoke("delete_draft", { draft: ref });
        dropMsg(m.accountId, m.folder, m.uid);
        S.sel = null;
        S.msg = null;
        loadFolders(m.accountId);
        render();
      } catch (e) {
        fail(e);
      }
      return;
    }
    case "edit-draft": {
      if (!m || !S.msg) return;
      const view = S.msg, where = { ...selectionOf(m), uidValidity: view.uidValidity ?? null };
      try {
        // 只取视图列出的附件；回复引用里的内嵌图片不是附件
        const indices = (view.attachments || []).map((a) => a.index);
        const picked = indices.length ? await invoke("draft_attachments", { ...where, indices }) : [];
        const draft = window.MailPulseDraft.draftFromView(view, where, picked);
        // 自己的草稿里有回执请求头，说明当时勾了「已读回执」
        draft.requestReceipt = !!view.receiptTo;
        // 草稿的 HTML 要放进主窗口的编辑器，先按白名单清理
        if (view.html) {
          const clean = await invoke("clean_html", { html: view.html, blockRemote: true });
          draft.html = window.MailPulseEditor.unwrapOuterDiv(clean);
        }
        return openCompose(draft);
      } catch (e) {
        return fail(e);
      }
    }
    case "compose-send-confirm": return sendCompose({ allowEmptySubject: true });
    case "compose-templates": return showComposePopover(btn.closest(".compose-sheet"), "templates");
    case "template-save": return saveTemplate(btn.closest(".compose-sheet"));
    case "compose-schedule": return showComposePopover(btn.closest(".compose-sheet"), "schedule");
    case "schedule-ok": return scheduleCompose(btn.closest(".compose-sheet"), X().fromLocalInput($("#c-send-at").value));
    case "receipt-send": return answerReceipt(true);
    case "receipt-ignore": return answerReceipt(false);
    case "outbox-send-now": return outboxAction("send-now");
    case "outbox-cancel": return outboxAction("cancel");
    case "outbox-reschedule": S.outboxEditing = `${m?.accountId}|${m?.uid}`; renderReader(); return $("#outbox-send-at")?.focus();
    case "outbox-reschedule-ok": return outboxAction("reschedule-ok");
    case "outbox-reschedule-cancel": S.outboxEditing = null; return renderReader();
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
    case "send-records": return openSendRecords();
    case "create-folder": return openCreateFolder();
    case "search":
      S.searching = true;
      renderList();
      return $("#q").focus();
    case "search-close":
      S.searching = false;
      S.query = "";
      requestServerSearch();
      return render();
    case "filter":
      S.filter = btn.dataset.filter || "all";
      return renderList();
    case "list-menu":
      return showMenu(btn, [
        { label: "按会话显示", check: S.threads, run: toggleThreads },
        "-",
        { label: "选择已加载的邮件", icon: "checkAll", run: () => { S.batch=MS().all(selectionGroups()); renderList(); $(".rows")?.focus(); } },
        { label: "已加载邮件标为已读", icon: "checkAll", run: markAllRead },
        { label: "操作记录", icon: "clock", run: showOperationHistory },
        { label: "刷新", icon: "refresh", run: () => {
          const targets = S.acct === "all" ? S.accounts.map((a) => a.id) : [S.acct];
          for (const id of targets) { loadFolders(id); loadList(id, S.folder, { force: true }); }
        } },
      ]);
    case "repair-test": return runAccountRepair(btn.closest('.account-repair-form'), 'test');
    case "repair-reconnect": return runAccountRepair(btn.closest('.account-repair-form'), 'reconnect');
    case "repair-password": {
      const input = btn.closest('.connection-password').querySelector('input');
      input.type = input.type === 'password' ? 'text' : 'password';
      btn.textContent = input.type === 'password' ? '显示' : '隐藏';
      return;
    }
    case "repair-account": return openAccountRepair(btn.dataset.accountId || S.accounts.find(a => ['error','offline'].includes(S.live[a.id]?.state))?.id || S.accounts[0]?.id);
    case "check-mail": return checkMail();
    case "toggle-sidebar": return toggleSidebar();
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
    case "show-person": return showPersonMenu(btn);
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

document.addEventListener("change", (e) => {
  if (e.target.id !== "receive-protocol") return;
  formReceiveProtocol = e.target.value === "pop3" ? "pop3" : "imap";
  const wrap = e.target.closest("#acct-form-wrap");
  const form = e.target.form;
  // 只恢复新表单仍有的字段；协议专属服务器字段使用各自的默认值。
  const keep = [...form.elements].filter((field) => field.name).map((field) => ({
    name: field.name, value: field.value, checked: field.checked,
  }));
  wrap.innerHTML = accountForm();
  const nextForm = $("#acct-form", wrap);
  for (const saved of keep) {
    const field = nextForm.elements.namedItem(saved.name);
    if (!field) continue;
    if (field.type === "checkbox") field.checked = saved.checked;
    else field.value = saved.value;
  }
  $("#receive-protocol", wrap)?.focus();
});

document.addEventListener("submit", (e) => {
  if (e.target.matches?.(".account-repair-form")) {
    e.preventDefault();
    return runAccountRepair(e.target, "save");
  }
  if (e.target.matches?.(".rule-form")) {
    e.preventDefault();
    submitRule(e.target);
  } else if (e.target.id === "acct-form") {
    e.preventDefault();
    submitAccount(e.target);
  } else if (e.target.id === "folder-form") {
    e.preventDefault();
    submitFolder(e.target);
  } else if (e.target.matches(".signature-form")) {
    e.preventDefault();
    saveSignature(e.target);
  } else if (e.target.matches(".body-cache-form")) {
    e.preventDefault();
    saveBodyCache(e.target);
  } else if (e.target.matches(".auto-download-form")) {
    e.preventDefault();
    saveAutoDownload(e.target);
  }
});


// 输入法会在 composition 期间先发出一串带拼音的 input 事件。此时如果
// 重建搜索框，原生输入法会失去 composition 状态，最终把未提交的拼音
// 直接写进搜索框（例如“工贸bong'z...”）。等候 compositionend 后再筛选。
document.addEventListener("compositionstart", (e) => {
  if (e.target.id === "q") S.searchComposing = true;
});

document.addEventListener("compositionend", (e) => {
  if (e.target.id !== "q") return;
  S.searchComposing = false;
  S.query = e.target.value;
  renderList();
  requestServerSearch();
});

document.addEventListener("input", (e) => {
  const repair = e.target.closest('.account-repair-form');
  if (repair) {
    repair.dataset.awaitConnection = 'false';
    const status = $('.connection-status', repair);
    status.hidden = false;
    status.textContent = '设置尚未保存；修改后请重新检测。';
    return;
  }
  const downloadForm = e.target.closest(".auto-download-form");
  if (downloadForm) return autoDownloadChanged(downloadForm);
  if (e.target.id === "q") {
    if (e.isComposing || S.searchComposing) return;
    S.query = e.target.value;
    renderList();
    requestServerSearch();
  }
  if (RECIPIENT_FIELDS.has(e.target.id)) scheduleSuggest(e.target);
  if (e.target.id === "c-picker-q") return filterContactPicker(e.target);
  if (e.target.id === "contacts-q" && S.contactsUi) {
    S.contactsUi.query = e.target.value.trim();
    clearTimeout(S.contactsUi.timer);
    S.contactsUi.timer = setTimeout(() => refreshContacts({ detail: false }), 150);
    return;
  }
  const compose = e.target.closest?.(".compose-sheet");
  if (compose && !e.target.closest(".contact-picker")) composeChanged(compose);
});

document.addEventListener("change", (e) => {
  const ruleForm = e.target.closest?.(".rule-form");
  if (ruleForm?.matches?.(".rule-form") && S.rulesUi?.editing) return ruleFormChanged(ruleForm, e.target);
  if (e.target.matches?.("[data-rule-toggle]")) {
    const index = +e.target.closest("[data-rule-index]").dataset.ruleIndex;
    S.rulesUi.focus = { id: S.rules[index].id, action: "toggle" };
    return saveRules(S.rules.map((r, i) => (i === index ? { ...r, enabled: e.target.checked } : r)));
  }
  const prefs = prefsControls(e.target);
  if (prefs) return savePrefs(prefs);
  if (e.target.classList?.contains("bulk-download-inherit")) {
    return changeBulkDownloadInheritance(e.target.closest(".bulk-download-section"), e.target.checked);
  }
  const downloadForm = e.target.closest(".auto-download-form");
  if (downloadForm) return autoDownloadChanged(downloadForm);
  if (e.target.dataset?.memberId && S.contactsUi) return updateGroupMembers(e.target);
  const fmt = e.target.dataset?.fmt;
  if (fmt) {
    const sheet = e.target.closest(".compose-sheet");
    const select = e.target.tagName === "SELECT";
    if (sheet && e.target.value) runFormat(sheet, fmt, e.target.value, select ? { keepFocus: e.target } : undefined);
    return;
  }
  if (e.target.id !== "c-from") return;
  const sheet = e.target.closest(".sheet");
  const body = $("#c-body", sheet);
  const before = body.innerHTML;
  const next = window.MailPulseEditor.switchSignature(before, sheet._signatureBlock, account(e.target.value)?.signature || "");
  if (next.html !== before) {
    // 只替换签名节点，保留其余内容的光标位置和撤销记录；不支持时退回整体替换
    const node = body.querySelector("[data-mp-signature]");
    if (node && typeof node.insertAdjacentHTML === "function" && node.outerHTML === sheet._signatureBlock) {
      if (next.block) node.insertAdjacentHTML("afterend", next.block);
      node.remove();
    } else if (!node && sheet._signatureBlock === "" && typeof body.insertAdjacentHTML === "function") {
      body.insertAdjacentHTML("beforeend", next.block);
    } else {
      body.innerHTML = next.html;
    }
  }
  sheet._signatureBlock = next.block;
  composeChanged(sheet);
});

document.addEventListener("paste", (e) => {
  if (e.target.closest?.("#c-body")) return insertTransfer(e, e.clipboardData);
});

// 拖进编辑器的网页内容同样要清理（文件拖拽由 Tauri 接管，登记为附件）
let draggingInsideEditor = false;
document.addEventListener("dragstart", (e) => { draggingInsideEditor = !!e.target.closest?.("#c-body"); });
document.addEventListener("dragend", () => { draggingInsideEditor = false; });
document.addEventListener("drop", (e) => {
  if (!e.target.closest?.("#c-body")) return;
  // 编辑器内部拖动（移动选中的文字）交给浏览器原生处理
  if (draggingInsideEditor) { draggingInsideEditor = false; return; }
  // 插到放下的位置，而不是原来的光标处
  const range = document.caretRangeFromPoint?.(e.clientX, e.clientY);
  if (range && typeof window.getSelection === "function") {
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  }
  return insertTransfer(e, e.dataTransfer);
});

// 字体、字号下拉框离开时回到提示项：下次再选同一项也会生效，也不会显示与光标处不符的值
document.addEventListener("focusout", (e) => {
  if (!applyingFormat && e.target.matches?.("select[data-fmt]")) e.target.value = "";
  if (RECIPIENT_FIELDS.has(e.target.id)) {
    const sheet = e.target.closest(".compose-sheet");
    if (sheet?._suggest?.input === e.target) hideSuggestions(sheet);
  }
});

// 点工具栏按钮不让编辑器失去焦点和选区
document.addEventListener("mousedown", (e) => {
  if (e.target.closest?.(".compose-toolbar button, .recipient-suggest button")) e.preventDefault();
});

document.addEventListener("selectionchange", rememberSelection);

function onKey(e) {
  if ((e.key === 'Enter' || e.key === ' ') && !e.isComposing && !e.metaKey && !e.ctrlKey && !e.altKey && e.target.closest?.('[data-action="repair-account"]')) {
    e.preventDefault();
    return e.target.closest('[data-action="repair-account"]').click();
  }
  if (e.key !== " ") cancelReaderScroll();
  if (e.target.id === "outbox-send-at" && e.key === "Enter") {
    e.preventDefault();
    return outboxAction("reschedule-ok");
  }
  // 链接/表格输入框：Esc 只关输入框，回车确认
  const popover = e.target.closest?.(".compose-popover");
  if (popover && (e.key === "Escape" || e.key === "Enter") && !e.isComposing) {
    // 焦点在按钮上时回车就是点这个按钮
    if (e.key === "Enter" && e.target.tagName === "BUTTON") return;
    e.preventDefault();
    const sheet = popover.closest(".compose-sheet");
    if (e.key === "Enter") return confirmComposePopover(sheet, e.target);
    hideComposePopover(sheet);
    return restoreSelection(sheet);
  }
  // 收件人联想：方向键选择，回车或 Tab 确认，Esc 只关列表
  const suggestSheet = RECIPIENT_FIELDS.has(e.target.id) ? e.target.closest(".compose-sheet") : null;
  // keyCode 229：WebKit 里输入法确认的回车 isComposing 可能是 false
  if (suggestSheet?._suggest?.input === e.target && !e.isComposing && e.keyCode !== 229 && !e.metaKey && !e.ctrlKey) {
    const current = suggestSheet._suggest;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      current.index = (current.index + (e.key === "ArrowDown" ? 1 : -1) + current.items.length) % current.items.length;
      return renderSuggestions(suggestSheet);
    }
    if (e.key === "Enter" || (e.key === "Tab" && !e.shiftKey)) {
      e.preventDefault();
      return pickSuggestion(suggestSheet, current.index);
    }
    if (e.key === "Tab") hideSuggestions(suggestSheet);
    // Shift+Delete：键盘删除高亮的最近联系人
    if (e.key === "Delete" && e.shiftKey && current.items[current.index]?.kind === "recent") {
      e.preventDefault();
      return forgetSuggestion(suggestSheet, current.index);
    }
    if (e.key === "Escape") {
      e.preventDefault();
      return hideSuggestions(suggestSheet);
    }
  }
  // 通讯录选择器里按 Esc 只关选择器
  const picker = e.target.closest?.(".contact-picker");
  if (picker && e.key === "Escape") {
    e.preventDefault();
    return closeContactPicker(picker.closest(".compose-sheet"));
  }
  const typing = e.target.closest?.("input, textarea, select, [contenteditable]");
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
    hideReaderHint({ immediate: true });
    if (overlay) {
      const sheet = $("#c-body")?.closest(".sheet");
      if (!sheet) return closeOverlay();
      e.preventDefault();
      if (sheet.dataset.busy === "true") return;
      return !$(".compose-confirm", sheet).hidden ? hideComposeConfirmation(sheet) : requestComposeClose();
    }
    if (S.batch.size) { e.preventDefault(); clearBatch(); renderList(); $(".rows")?.focus(); return; }
    if (S.searching) { S.searching = false; S.query = ""; requestServerSearch(); return render(); }
    if (S.sel) { ++openSeq; S.sel = null; S.msg = null; return render(); }
  }
  if (e.metaKey && e.shiftKey && e.key.toLowerCase() === "n") { e.preventDefault(); return checkMail(); }
  if (e.metaKey && e.key.toLowerCase() === "n") { e.preventDefault(); return openCompose(); }
  if (e.metaKey && e.key.toLowerCase() === "f") { e.preventDefault(); S.searching = true; renderList(); return $("#q").focus(); }
  const listFocus = e.target.closest?.("#list");
  if (!typing && !overlay && !e.isComposing && e.keyCode!==229 && !e.altKey && listFocus && (e.metaKey || e.ctrlKey) && e.key.toLowerCase()==="a") {
    e.preventDefault(); S.batch=MS().all(selectionGroups()); renderList(); $(".rows")?.focus(); return;
  }
  if (typing || overlay || e.metaKey || e.ctrlKey || e.altKey || e.isComposing || e.keyCode===229 || !S.accounts.length) return;
  if (S.batch.size && ["e","s","u","Delete","Backspace","#"].includes(e.key)) {
    if (listFocus) {
      e.preventDefault();
      return bulkAction({e:"archive",s:"flagged",u:"seen",Delete:"trash",Backspace:"trash","#":"trash"}[e.key],e.target);
    }
    // 重绘、移除行等可能让焦点落在 body；不能悄悄转成对旧阅读邮件的操作。
    const readerFocus = e.target.closest?.("#reader") || (e.target.ownerDocument && e.target.ownerDocument === $("#mail-frame")?.contentDocument);
    if (!readerFocus) { e.preventDefault(); return; }
  }
  const m = findMsg(S.sel);
  const click = (a) => $(`[data-action="${a}"]`)?.click();
  switch (e.key) {
    case " ": {
      if (e.shiftKey || e.isComposing || e.keyCode === 229 || !S.sel || !m) return;
      // 列表行也是 button；其他控件的空格仍用于原生点击、勾选等操作。
      if (!row && e.target.closest?.('button, a[href], summary, [role="button"], [role="checkbox"], [role="switch"]')) return;
      if (!$(".read-scroll", $("#reader") || document)) return;
      e.preventDefault();
      if (e.repeat) return;
      hideReaderHint();
      return spaceForward();
    }
    case "ArrowDown": case "j": e.preventDefault(); hideReaderHint(); return step(1);
    case "ArrowUp": case "k": e.preventDefault(); hideReaderHint(); return step(-1);
    case "/": e.preventDefault(); return click("search");
    case "r": case "a": case "f":
      if (m && S.msg?.partial) return toast("邮件还在加载");
      // 点击会同步聚焦写信输入框，先取消默认输入，避免把快捷键字符写进正文或收件人。
      if (m) e.preventDefault();
      if (e.key === "r" && m && S.msg && $('#reader [data-action="reply"]') && !$('#reader [data-action="reply"]').disabled) hideReaderHint();
      return m && click({ r: "reply", a: "reply-all", f: "forward" }[e.key.toLowerCase()]);
    case "e":
      if (m && $('#reader [data-action="archive"]') && !$('#reader [data-action="archive"]').disabled) hideReaderHint();
      return m && click("archive");
    case "s":
      if (m && m.folder !== "__outbox") hideReaderHint();
      return m && setFlag(m, "flagged", !m.flagged);
    case "u": return m && setFlag(m, "seen", !m.seen);
    case "Backspace": case "Delete": case "#": return m && click("delete");
  }
}
document.addEventListener("keydown", onKey);
watchReaderScrollInput(document);

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
  S.ui = loadUiPrefs();
  const railExpand = $("#rail-expand");
  if (railExpand) railExpand.innerHTML = icon("sidebar");
  applyLayout();
  const splitter = $("#list-splitter");
  splitter?.addEventListener("pointerdown", startListResize);
  splitter?.addEventListener("dblclick", resetListWidth);
  $("#side-hotzone")?.addEventListener("mouseenter", () => setRailPeek(true));
  $("#sidebar")?.addEventListener("mouseleave", () => setRailPeek(false));
  if (typeof window.addEventListener === "function") {
    window.addEventListener("resize", () => {
      S.ui.listWidth = clampListWidth(S.ui.listWidth);
      applyLayout();
    });
  }
  loadThreadPrefs();
  loadReceiptPrefs();
  await listen("mail", (e) => onMailEvent(e.payload));
  await listen("attachments-dropped", (e) => onAttachmentsDropped(e.payload));
  await listen("tray-compose", () => openCompose());
  await listen("outbox", (e) => onOutboxEvent(e.payload));
  await listen("send-record-changed", () => window.MailBeaconSendRecords?.changed());
  await listen("open-mail", (e) => {
    const sel = window.MailBeaconNotificationOpen?.selectionFromNotification?.(e.payload);
    if (!sel) return;
    openFromNotification(sel).catch(fail);
  });
  refreshOutbox();
  // 设置在 savePrefs 里也会补读；这里提前读好，打开设置时显示的就是已保存的值
  invoke("get_prefs").then((p) => { if (!S.prefs) { S.prefs = p; syncPrefsForm(); } }).catch((e) => console.warn("读取设置失败", e));
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
  // 首屏先用后台缓存的第一页快速显示；本地索引里已经有整个文件夹时，
  // 这里接着把完整列表读出来（读本地库，不碰网络）
  for (const id of S.acct === "all" ? S.accounts.map((a) => a.id) : [S.acct]) loadList(id, S.folder);
  updateBadge();
  render();
})();

// 第一次添加账号后从欢迎页切到三栏
const _render = render;
render = function () {
  if (S.accounts.length && $("#sidebar").hidden) leaveWelcome();
  _render();
};
