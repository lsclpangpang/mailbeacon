/* 邮件缓存设置：每个账号的缓存总量、预先下载的单封上限和后台预热进度。 */
(function (root) {
  "use strict";

  const DEFAULTS = { enabled: true, limitMb: 2048, maxMailMb: 5 };

  /** 账号里保存的设置；老账号没有这一项时用默认值（默认开启） */
  function settingsOf(account) {
    return { ...DEFAULTS, ...(account?.bodyCache || {}) };
  }

  /** 表单里的值 → { settings } 或 { error }，规则和后端一致 */
  function fromForm({ enabled, limitMb, maxMailMb }) {
    const whole = (v) => (String(v).trim() === "" ? NaN : Number(v));
    const limit = whole(limitMb), max = whole(maxMailMb);
    if ([limit, max].some((n) => Number.isFinite(n) && !Number.isInteger(n))) return { error: "请填写整数（MB）" };
    if (!Number.isInteger(limit) || limit < 64 || limit > 102400) return { error: "缓存总量请填 64～102400 MB" };
    if (!Number.isInteger(max) || max < 1 || max > 32) return { error: "单封大小上限请填 1～32 MB" };
    if (max > limit) return { error: "单封大小上限不能超过缓存总量" };
    return { settings: { enabled: !!enabled, limitMb: limit, maxMailMb: max } };
  }

  function size(bytes) {
    const mb = bytes / 1024 / 1024;
    if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
    return `${Math.round(mb)} MB`;
  }

  /** 设置页上的一行状态：在做什么 + 用了多少 */
  function statusText(p, settings) {
    if (!p) return "正在读取缓存状态…";
    const used = `已用 ${size(p.cachedBytes || 0)} / ${size(p.limitBytes || settings.limitMb * 1024 * 1024)}（${p.cachedCount || 0} 封）`;
    const doing = {
      running: `正在缓存收件箱 ${p.done || 0} / ${p.total || 0} 封`,
      waiting: "等待空闲后开始缓存",
      done: `收件箱已全部缓存（不含超过 ${settings.maxMailMb} MB 的邮件）`,
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

  const api = { DEFAULTS, settingsOf, fromForm, statusText, bar, canRefresh };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconBodyCache = api;
})(typeof window !== "undefined" ? window : globalThis);
