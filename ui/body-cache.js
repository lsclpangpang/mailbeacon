/* 邮件缓存设置：总量、历史预热上限、保留期限和后台进度。 */
(function (root) {
  "use strict";

  const DEFAULTS = { enabled: true, limitMb: 2048, maxMailMb: 5, autoCleanupDays: 0 };
  const CLEANUP_DAYS = [7, 30, 90, 180, 365];

  /** 账号里保存的设置；老账号没有这一项时用默认值（默认开启） */
  function settingsOf(account) {
    return { ...DEFAULTS, ...(account?.bodyCache || {}) };
  }

  /** 表单里的值 → { settings } 或 { error }，规则和后端一致 */
  function fromForm({ enabled, limitMb, maxMailMb, autoCleanupDays = 0 }) {
    const whole = (v) => (String(v).trim() === "" ? NaN : Number(v));
    const limit = whole(limitMb), max = whole(maxMailMb);
    if ([limit, max].some((n) => Number.isFinite(n) && !Number.isInteger(n))) return { error: "请填写整数（MB）" };
    if (!Number.isInteger(limit) || limit < 64 || limit > 102400) return { error: "缓存总量请填 64～102400 MB" };
    if (!Number.isInteger(max) || max < 1 || max > 32) return { error: "单封大小上限请填 1～32 MB" };
    if (max > limit) return { error: "单封大小上限不能超过缓存总量" };
    const days = whole(autoCleanupDays);
    if (!Number.isInteger(days) || days < 0 || days > 36500) return { error: "自动清理期限请填 1～36500 的整数天数，0 表示关闭" };
    return { settings: { enabled: !!enabled, limitMb: limit, maxMailMb: max, autoCleanupDays: days } };
  }

  function cleanupDays(period, customDays, automatic = false) {
    if (period === 'all' && !automatic) return { days: null };
    if (period === '0' && automatic) return { days: 0 };
    const days = Number(period === 'custom' ? customDays : period);
    if (!Number.isInteger(days) || days < 1 || days > 36500
        || (period !== 'custom' && !CLEANUP_DAYS.includes(days))) return { error: "清理期限请填写 1～36500 的整数天数" };
    return { days };
  }

  function cleanupChoice(days) {
    return days === 0 || CLEANUP_DAYS.includes(days) ? String(days) : 'custom';
  }

  function cleanupSize(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
    return size(bytes);
  }

  function size(bytes) {
    const mb = bytes / 1024 / 1024;
    if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
    return `${Math.round(mb)} MB`;
  }

  /** 设置页上的一行状态：在做什么 + 用了多少 */
  function statusText(p, settings) {
    if (!p) return "正在读取缓存状态…";
    const fromDownloads = p.archived > 0 ? `其中 ${p.archived} 封直接用已下载的邮件` : "";
    const used = `已用 ${size(p.cachedBytes || 0)} / ${size(p.limitBytes || settings.limitMb * 1024 * 1024)}（${p.cachedCount || 0} 封）`;
    const doing = {
      running: `正在缓存收件箱 ${p.done || 0} / ${p.total || 0} 封${fromDownloads ? `（${fromDownloads}）` : ""}`,
      waiting: "等待空闲后开始缓存",
      done: `符合设置的收件箱缓存已更新（历史预热单封上限 ${settings.maxMailMb} MB${fromDownloads ? `，${fromDownloads}` : ""}）`,
      full: "已到缓存总量上限，不再预先下载",
      error: `缓存中断：${p.detail || "未知错误"}`,
      off: "未开启预先缓存，打开过的邮件照样缓存",
    }[p.state];
    return doing ? `${doing} · ${used}` : used;
  }

  /** 进度条：只在缓存进行中显示；还不知道总数时显示为不确定进度 */
  function bar(p) {
    if (p?.state !== "running") return { visible: false, value: null, max: 1 };
    if (!p.total) return { visible: true, value: null, max: 1 };
    return { visible: true, value: p.done || 0, max: p.total };
  }

  /** 「立即更新缓存」只在真正下载中时不可点；等待空闲时点一下就马上开始 */
  function canRefresh(p) {
    return p?.state !== "running";
  }

  /** 当缓存用的下载目录 → 设置页上的行 */
  function archiveRows(list) {
    return (list || []).map((d) => ({
      path: d.path,
      label: d.kind === "auto" ? "自动下载（收件箱）" : "全量下载",
      note: d.online ? "" : "不在线",
      removable: !!d.removable,
    }));
  }

  const api = { DEFAULTS, CLEANUP_DAYS, settingsOf, fromForm, cleanupDays, cleanupChoice, cleanupSize, statusText, bar, canRefresh, archiveRows };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconBodyCache = api;
})(typeof window !== "undefined" ? window : globalThis);
