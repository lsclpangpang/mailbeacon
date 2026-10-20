/* Incoming-mail rules: conditions, actions and how they read in the UI. */
(function (root) {
  "use strict";

  const FIELDS = [
    { key: "from", label: "发件人" },
    { key: "to", label: "收件人" },
    { key: "subject", label: "主题" },
    { key: "attachment", label: "附件" },
  ];
  const OPS = [
    { key: "contains", label: "包含" },
    { key: "notContains", label: "不包含" },
    { key: "equals", label: "等于" },
  ];
  /** The attachment field only asks "has / has not". */
  const ATTACHMENT_OPS = [
    { key: "contains", label: "有附件" },
    { key: "notContains", label: "没有附件" },
  ];

  const blankRule = (accountId) => ({
    id: "", name: "", enabled: true, accountId, matchAll: true,
    conditions: [{ field: "from", op: "contains", value: "" }],
    actions: { moveTo: null, markRead: false, flag: false },
  });

  function ruleFromMessage(m) {
    const rule = blankRule(m.accountId);
    rule.name = `来自 ${m.fromName || m.fromAddr}`;
    rule.conditions = [{ field: "from", op: "equals", value: m.fromAddr }];
    return rule;
  }

  /** Moving new mail into the inbox, drafts or sent makes no sense. */
  const NOT_TARGETS = new Set(["inbox", "drafts", "sent", "local-drafts"]);
  const moveTargets = (folders) => (folders || []).filter((f) => !NOT_TARGETS.has(f.role) && !String(f.path).startsWith("__"));
  /** Folder suggested when "move" is first ticked: the user's own folders before system ones. */
  const defaultTarget = (folders) => {
    const list = moveTargets(folders);
    // 没有自己的文件夹时留空让用户选：默认移到「垃圾邮件」「所有邮件」都容易出事
    return (list.find((f) => f.role === "other") || list.find((f) => f.role === "archive"))?.path || "";
  };

  const labelOf = (list, key) => list.find((x) => x.key === key)?.label || key;

  function describe(rule, folders) {
    const conds = (rule.conditions || []).map((c) => c.field === "attachment"
      ? labelOf(ATTACHMENT_OPS, c.op)
      : `${labelOf(FIELDS, c.field)}${labelOf(OPS, c.op)}「${c.value}」`);
    const a = rule.actions || {};
    const acts = [];
    if (a.moveTo) acts.push(`移到「${(folders || []).find((f) => f.path === a.moveTo)?.name || a.moveTo}」`);
    if (a.markRead) acts.push("标为已读");
    if (a.flag) acts.push("加星标");
    return `${conds.join(rule.matchAll ? " 且 " : " 或 ")} → ${acts.join("、")}`;
  }

  /** What the user still has to fill in; empty when the rule can be saved. */
  function problem(rule) {
    if (!rule.accountId) return "请选择邮箱账号";
    if (!rule.conditions?.length) return "请至少添加一个条件";
    if (rule.conditions.some((c) => c.field !== "attachment" && !String(c.value || "").trim())) return "请填写条件内容";
    const a = rule.actions || {};
    if (!a.moveTo && !a.markRead && !a.flag) return "请至少选择一个动作";
    return "";
  }

  function move(list, index, delta) {
    const to = index + delta;
    const next = [...list];
    if (to < 0 || to >= list.length) return next;
    [next[index], next[to]] = [next[to], next[index]];
    return next;
  }

  const api = { FIELDS, OPS, ATTACHMENT_OPS, blankRule, ruleFromMessage, moveTargets, defaultTarget, describe, problem, move };
  if (typeof module === "object" && module.exports) module.exports = api;
  else root.MailBeaconRules = api;
})(typeof window !== "undefined" ? window : globalThis);
