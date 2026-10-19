// 只在普通浏览器里预览界面时使用：模拟 Tauri 后端，返回假数据。App 里不会加载。
(function () {
  const now = Date.now();
  const H = 3600e3;
  const accounts = [
    { id: "a1", kind: "gmail", email: "jarvis@gmail.com", name: "Jarvis Li", signature: "", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465 },
    { id: "a2", kind: "exmail", email: "li@company.com", name: "李嘉文", signature: "", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465 },
  ];
  const fieldKeys = ["subject", "sender", "recipient", "body"];
  const emptyFieldKeywords = () => Object.fromEntries(fieldKeys.map(key => [key, []]));
  const normalizeKeywords = values => {
    if (!Array.isArray(values)) throw "关键词必须是列表";
    const seen = new Set(), normalized = [];
    for (const value of values) {
      if (typeof value !== "string") throw "关键词必须是文本";
      const keyword = value.trim();
      if (!keyword) continue;
      if ([...keyword].length > 200) throw "每个关键词最多 200 个字符";
      const key = keyword.toLowerCase();
      if (!seen.has(key)) { seen.add(key); normalized.push(keyword); }
    }
    if (normalized.length > 100) throw "最多可设置 100 个关键词";
    return normalized;
  };
  for (const account of accounts) account.autoDownload = { enabled: false, directory: "", filterMode: "none", keywords: [], keywordScope: "any", fieldKeywords: emptyFieldKeywords() };
  const autoDownloads = Object.fromEntries(accounts.map(a => [a.id, { state: "disabled", detail: "未开启自动下载", savedCount: 0 }]));
  const bulkIdle = () => ({ state: "idle", detail: "尚未开始全量下载", directory: "", currentFolder: null, inheritFilters: false, filterMode: "none", keywordScope: "any", savedCount: 0, existingCount: 0, skippedCount: 0, failedCount: 0, vanishedCount: 0, totalCount: null, processedCount: 0, foldersScanned: 0, foldersDone: 0, folderCount: 0 });
  const bulkDownloads = Object.fromEntries(accounts.map(a => [a.id, bulkIdle()]));
  const bulkJobs = new Map(), bulkArchives = new Map();
  const manualDownloadPaths = new Set();
  const mailListeners = new Set();
  const raw = [
    ["a1", "Anne-Laure Le Cunff", "annelaure@nesslabs.com", "Ness Labs: Outsourcing Discovery 🍎", 0.3, false, false, false, "What do we lose when we outsource the process of discovery? This question has been on my mind these past few weeks."],
    ["a2", "王小明", "wxm@company.com", "Q4 产品路线图评审（周三下午）", 1.2, false, true, true, "各位好，附件是 Q4 路线图初稿，周三下午 3 点在 5 楼大会议室过一遍，请提前看一下标黄的部分。"],
    ["a1", "GitHub", "noreply@github.com", "[GitHub] A new SSH key was added to your account", 2.5, true, false, false, "The following SSH key was added to your account. If you believe this key was added in error, you can remove it."],
    ["a1", "Vercel", "notifications@vercel.com", "1 domain needs configuration on team 'personal'", 5, true, false, false, "We noticed that a domain on your team is not configured correctly. Visit the dashboard to finish setup."],
    ["a2", "陈静", "chenjing@company.com", "Re: 报销流程调整", 20, true, false, false, "好的，我这边已经同步给财务了，下周一开始按新流程走，旧的单据这周内提交还可以按原流程处理。"],
    ["a1", "Craft Docs", "hello@craft.do", "Craft 更新 - 快捷指令、小组件、本地化", 26, true, false, false, "这次更新带来了全新的快捷指令支持、主屏小组件，以及完整的简体中文界面。"],
    ["a1", "Linear", "changelog@linear.app", "Changelog: Triage intelligence & faster sync", 30, true, true, false, "This week we shipped triage intelligence, a rebuilt sync engine that is up to 3x faster, and dozens of fixes."],
    ["a2", "IT 服务台", "it@company.com", "【通知】本周六凌晨 2:00-4:00 邮件系统维护", 50, true, false, false, "维护期间邮件收发可能出现短暂延迟，客户端无需任何操作，维护结束后会自动恢复。"],
    ["a1", "长桥证券（香港）", "service@longbridge.hk", "换汇提醒", 60, true, false, false, "尊敬的用户：您的长桥综合账户（H…）本月有一笔换汇申请已完成，请登录 App 查看详情。"],
    ["a1", "Lemon Squeezy", "hello@lemonsqueezy.com", "You've been paid!", 75, true, false, true, "Lemon Squeezy just sent a payout of $1,284.00 to your bank account. It should arrive in 1-3 business days."],
    ["a2", "张磊", "zhanglei@company.com", "接口文档 v2 已更新", 98, true, false, true, "v2 主要改了鉴权部分，老的 token 方式会保留到月底，新接入的服务直接用 OAuth。"],
    ["a1", "DEX 周刊", "weekly@dex.dev", "#367 压缩和解压缩", 120, true, false, false, "本期聊聊 zstd 字典训练、Brotli 在静态资源上的取舍，以及一个用 Rust 写的流式解压库。"],
  ];
  let uid = 500;
  const inbox = { a1: [], a2: [] };
  for (const r of raw) {
    inbox[r[0]].push({ accountId: r[0], folder: "INBOX", uid: uid--, fromName: r[1], fromAddr: r[2], to: "我", subject: r[3], date: now - r[4] * H, seen: r[5], flagged: r[6], attachment: r[7], _prev: r[8] });
  }
  const defaultFolders = (role) => [
    { path: "INBOX", name: "收件箱", role: "inbox" },
    { path: "Drafts", name: "草稿", role: "drafts" },
    { path: "Sent", name: "已发送", role: "sent" },
    ...(role === "a1" ? [{ path: "[Gmail]/All Mail", name: "所有邮件", role: "all" }] : [{ path: "Archive", name: "归档", role: "archive" }]),
    { path: "Junk", name: "垃圾邮件", role: "junk" },
    { path: "Trash", name: "已删除", role: "trash" },
    ...(role === "a2" ? [{ path: "&UXZO1mWHTvZZOQ-", name: "项目资料", role: "other" }] : []),
  ];
  const folders = Object.fromEntries(accounts.map(a => [a.id, defaultFolders(a.id)]));
  const mailboxes = Object.fromEntries(accounts.map(a => [a.id, new Map(folders[a.id].map(f => [f.path, f.path === "INBOX" ? inbox[a.id] : []]))]));
  let nextUid = 600;
  const messages = (a, f = "INBOX") => mailboxes[a]?.get(f) || [];
  const find = (a, u, f = "INBOX") => messages(a, f).find(m => m.uid === u);
  const messageRecipients = accountId => [{ name: "", addr: accounts.find(a => a.id === accountId).email }];
  const newsletterParagraphs = [
    "What do we lose when we outsource the process of discovery? This question has been on my mind these past few weeks, as I've followed the debate around claims to have solved hundreds of longstanding mathematical problems.",
    "As a hypercurious neuroscientist and writer, I know how much joy can be found in research and creative work. So beyond the very real practical concerns, I wanted to explore the human side of the debate.",
  ];
  const newsletterText = ["October 15th, 2026", "A newsletter by Anne-Laure Le Cunff", "Hi friends,", ...newsletterParagraphs, "Stay curious,", "Anne-Laure"].join("\n\n");
  // The text alternative includes the same visible newsletter content as its HTML.
  const messageText = message => message._prev + "\n\n详细内容见附件，有问题随时找我。\n\n祝好\n" + message.fromName + "\nhttps://example.com/docs" + (message.fromAddr.includes("nesslabs") ? "\n\n" + newsletterText : "");
  const bulkActive = status => ["preparing", "scanning", "downloading", "cancelling"].includes(status?.state);
  const publishBulk = (accountId, status) => {
    bulkDownloads[accountId] = status;
    for (const listener of mailListeners) listener({ payload: { type: "bulkDownload", accountId, ...structuredClone(status) } });
  };
  const runBulkFolder = (accountId, job) => {
    if (bulkJobs.get(accountId) !== job || job.cancelled) return;
    const previous = bulkDownloads[accountId], folder = job.plan[previous.foldersDone];
    if (!folder) {
      publishBulk(accountId, { ...previous, state: "complete", detail: previous.filterMode === "none" ? "全部邮件下载完成" : "符合规则的邮件下载完成", currentFolder: null });
      bulkJobs.delete(accountId);
      return;
    }
    if (location.search.includes("bulk-runtime-error")) {
      publishBulk(accountId, { ...previous, state: "error", detail: "部分邮件下载失败：磁盘已满，可选择同一目录重试", failedCount: previous.failedCount + 1, currentFolder: folder.name });
      bulkJobs.delete(accountId);
      return;
    }
    const status = { ...previous, state: "downloading", detail: previous.filterMode === "none" ? "正在下载全部邮件…" : "正在按规则下载邮件…", currentFolder: folder.name, foldersDone: previous.foldersDone + 1 };
    for (const uid of folder.uids) {
      status.processedCount++;
      const message = find(accountId, uid, folder.path);
      if (!message) { status.vanishedCount++; continue; }
      const fields = {
        subject: [message.subject], sender: [message.fromName, message.fromAddr],
        recipient: messageRecipients(accountId).flatMap(address => [address.name, address.addr]),
        body: [messageText(message)],
      };
      const contains = (values, keywords) => keywords.some(keyword => values.some(value => (value || "").toLowerCase().includes(keyword)));
      const hit = status.keywordScope === "fields"
        ? fieldKeys.some(key => contains(fields[key], job.fieldKeywords[key]))
        : contains([...fields.subject, ...fields.sender, ...fields.body], job.keywords);
      if ((status.filterMode === "whitelist" && !hit) || (status.filterMode === "blacklist" && hit)) {
        status.skippedCount++;
        continue;
      }
      const key = `${folder.path}|${message.uid}`;
      if (job.archive.has(key)) status.existingCount++;
      else { job.archive.add(key); status.savedCount++; }
    }
    publishBulk(accountId, status);
    setTimeout(() => runBulkFolder(accountId, job), location.search.includes("bulk-slow") ? 4000 : 650);
  };
  const scanBulkFolder = (accountId, job) => {
    if (bulkJobs.get(accountId) !== job || job.cancelled) return;
    const previous = bulkDownloads[accountId], folder = job.folders[previous.foldersScanned];
    if (!folder) {
      publishBulk(accountId, { ...previous, state: "downloading", detail: "邮件总数统计完成，正在开始下载…", totalCount: job.plan.reduce((sum, entry) => sum + entry.uids.length, 0), currentFolder: null });
      setTimeout(() => runBulkFolder(accountId, job), 100);
      return;
    }
    job.plan.push({ ...folder, uids: messages(accountId, folder.path).map(message => message.uid) });
    publishBulk(accountId, { ...previous, state: "scanning", detail: "正在统计邮件总数…", foldersScanned: previous.foldersScanned + 1, currentFolder: folder.name });
    setTimeout(() => scanBulkFolder(accountId, job), location.search.includes("bulk-slow") ? 700 : 250);
  };
  const html = `<div style="background:#f6f5f3;padding:28px 0"><div style="max-width:560px;margin:0 auto;background:#fff;padding:36px 40px;font-family:Georgia,serif;color:#222">
    <p style="text-align:center;color:#666;font-size:13px">October 15th, 2026<br><i>A newsletter by <a href="https://nesslabs.com">Anne-Laure Le Cunff</a></i></p>
    <p>Hi friends,</p>${newsletterParagraphs.map(paragraph => `<p>${paragraph}</p>`).join("")}
    <img src="https://example.com/banner.png" alt="" width="480"><p>Stay curious,<br>Anne-Laure</p></div></div>`;
  const handlers = {
    snapshot: () => ({
      accounts,
      autoDownloads,
      bulkDownloads,
      live: {
        a1: { state: "online", detail: "实时推送", unread: messages("a1").filter((m) => !m.seen).length, inbox: messages("a1") },
        a2: { state: "online", detail: "实时推送", unread: messages("a2").filter((m) => !m.seen).length, inbox: messages("a2") },
      },
    }),
    list_folders: ({ accountId }) => folders[accountId],
    list_messages: ({ accountId, folder, flaggedOnly, before }) => messages(accountId, folder).filter(m => (!flaggedOnly || m.flagged) && (!before || m.uid < before)),
    get_previews: ({ accountId, folder, uids }) => uids.map((u) => ({ uid: u, text: find(accountId, u, folder)?._prev || "" })),
    get_message: ({ accountId, folder, uid }) => {
      const m = find(accountId, uid, folder);
      if (!m) throw "邮件不存在，请刷新后再试";
      m.seen = true;
      const isNews = m.fromAddr.includes("nesslabs");
      return {
        subject: m.subject, from: { name: m.fromName, addr: m.fromAddr },
        to: messageRecipients(accountId), cc: [],
        date: m.date, html: isNews ? html : null,
        text: messageText(m),
        hasRemote: isNews, attachments: m.attachment ? [{ index: 0, name: "Q4 路线图-初稿.pdf", size: 2_481_000 }, { index: 1, name: "数据附表.xlsx", size: 182_000 }] : [],
        messageId: "x@y", references: [], uidValidity: 20261019,
      };
    },
    set_flag: ({ accountId, folder, uids, flag, value }) => {
      for (const uid of uids) {
        const m = find(accountId, uid, folder);
        if (m) m[flag === "seen" ? "seen" : "flagged"] = value;
      }
      return null;
    },
    move_messages: ({ accountId, folder, uids, role, target }) => {
      if (location.search.includes("move-error")) throw "移动失败：连接中断，请刷新查看邮件所在位置";
      const destination = target || folders[accountId].find(f => f.role === role)?.path || (role === "archive" ? folders[accountId].find(f => f.role === "all")?.path : null);
      const permanent = role === "trash" && folder === "Trash";
      if (!permanent && (!destination || !mailboxes[accountId].has(destination))) throw "目标文件夹不存在";
      const source = messages(accountId, folder);
      if (!uids.every(uid => source.some(m => m.uid === uid))) throw "邮件不存在，请刷新后再试";
      if (!permanent) messages(accountId, destination).push(...source.filter(m => uids.includes(m.uid)).map(m => ({ ...m, folder: destination, uid: nextUid++ })));
      mailboxes[accountId].set(folder, source.filter(m => !uids.includes(m.uid)));
      return null;
    },
    create_folder: ({ accountId, name }) => {
      name = name.trim();
      if (!name) throw "请输入文件夹名称";
      if (/[\/\r\n]/.test(name)) throw "文件夹名称不能包含斜线或换行";
      if (folders[accountId].some(f => f.name.toLowerCase() === name.toLowerCase())) throw "同名文件夹已存在，请换个名称";
      const folder = { path: name, name, role: "other" };
      folders[accountId].push(folder);
      mailboxes[accountId].set(folder.path, []);
      return folder;
    },
    save_signature: ({ accountId, signature }) => {
      const a = accounts.find(a => a.id === accountId);
      a.signature = signature;
      return a;
    },
    choose_download_directory: () => {
      if (location.search.includes("directory-error")) throw "无法打开文件夹选择器";
      return location.search.includes("directory-cancel") ? null : "/Users/me/Documents/邮件归档";
    },
    start_bulk_download: ({ accountId, directory, inheritFilters = false }) => {
      const a = accounts.find(a => a.id === accountId);
      if (!a) throw "邮箱账户不存在";
      if (bulkActive(bulkDownloads[accountId])) throw "此账号已有全量下载任务正在运行";
      if (!directory?.startsWith("/")) throw "请选择有效的本地文件夹";
      if (location.search.includes("bulk-error")) throw "无法开始下载：目录不可写，请选择其他文件夹";
      const archiveKey = `${accountId}|${directory}`;
      if (!bulkArchives.has(archiveKey)) bulkArchives.set(archiveKey, new Set());
      const filterMode = inheritFilters ? a.autoDownload.filterMode : "none";
      const keywordScope = filterMode === "none" ? "any" : a.autoDownload.keywordScope;
      const fieldKeywords = Object.fromEntries(fieldKeys.map(key => [key, a.autoDownload.fieldKeywords[key].map(word => word.toLowerCase())]));
      const job = { archive: bulkArchives.get(archiveKey), cancelled: false, keywords: a.autoDownload.keywords.map(word => word.toLowerCase()), fieldKeywords, folders: structuredClone(folders[accountId]), plan: [] };
      bulkJobs.set(accountId, job);
      const status = { ...bulkIdle(), state: "preparing", detail: "正在读取所有文件夹…", inheritFilters, filterMode, keywordScope, directory: `${directory}/${a.email}`, folderCount: folders[accountId].length };
      publishBulk(accountId, status);
      setTimeout(() => scanBulkFolder(accountId, job), 100);
      return status;
    },
    cancel_bulk_download: ({ accountId }) => {
      if (!accounts.some(a => a.id === accountId)) throw "邮箱账户不存在";
      const previous = bulkDownloads[accountId], job = bulkJobs.get(accountId);
      if (!bulkActive(previous) || !job || job.cancelled) return previous;
      job.cancelled = true;
      const status = { ...previous, state: "cancelling", detail: "正在取消，已保存的邮件会保留…" };
      publishBulk(accountId, status);
      setTimeout(() => {
        if (bulkJobs.get(accountId) !== job) return;
        publishBulk(accountId, { ...status, state: "cancelled", detail: "已取消，已保存的邮件已保留" });
        bulkJobs.delete(accountId);
      }, 100);
      return status;
    },
    save_auto_download: ({ accountId, enabled, directory, filterMode, keywords, keywordScope, fieldKeywords }) => {
      const a = accounts.find(a => a.id === accountId);
      if (!a) throw "邮箱账户不存在";
      if (location.search.includes("auto-save-error")) throw "保存失败：目录不可写，请选择其他文件夹";
      if (enabled && (!directory.startsWith("/") || /[\x00-\x1f]/.test(directory))) throw "请选择文件夹或输入有效的完整路径";
      filterMode = filterMode ?? a.autoDownload.filterMode;
      keywordScope = keywordScope ?? a.autoDownload.keywordScope;
      if (!["none", "whitelist", "blacklist"].includes(filterMode)) throw "请选择有效的关键词过滤模式";
      if (!["any", "fields"].includes(keywordScope)) throw "请选择有效的关键词匹配方式";
      const normalized = normalizeKeywords(keywords ?? a.autoDownload.keywords);
      const fields = fieldKeywords ?? a.autoDownload.fieldKeywords;
      const normalizedFields = Object.fromEntries(fieldKeys.map(key => [key, normalizeKeywords(fields[key] ?? [])]));
      const fieldCount = Object.values(normalizedFields).reduce((count, words) => count + words.length, 0);
      if (fieldCount > 100) throw "四个字段合计最多可设置 100 个关键词";
      if (filterMode !== "none" && !(keywordScope === "fields" ? fieldCount : normalized.length)) throw "请至少填写一个关键词，或选择不过滤";
      a.autoDownload = { enabled, directory, filterMode, keywords: normalized, keywordScope, fieldKeywords: normalizedFields };
      autoDownloads[accountId] = enabled
        ? { state: "waiting", detail: "等待新邮件", savedCount: 0 }
        : { state: "disabled", detail: "未开启自动下载", savedCount: 0 };
      if (enabled && location.search.includes("auto-runtime-error")) autoDownloads[accountId] = { state: "error", detail: "自动下载失败：磁盘已满，将自动重试", savedCount: 0 };
      for (const listener of mailListeners) listener({ payload: { type: "autoDownload", accountId, ...structuredClone(autoDownloads[accountId]) } });
      return a;
    },
    save_message: ({ accountId, folder, uid }) => {
      if (location.search.includes("download-error")) throw "邮件保存失败：下载文件夹不可写";
      const m = find(accountId, uid, folder);
      if (!m) throw "邮件不存在，请刷新后再试";
      const clean = m.subject.replace(/[\x00-\x1f\x7f-\x9f]/g, "").replace(/[\\/:*?"<>|]/g, "_").replace(/^[.\s]+|[.\s]+$/g, "");
      let stem = "", bytes = 0;
      for (const character of clean) {
        const point = character.codePointAt(0), length = point < 0x80 ? 1 : point < 0x800 ? 2 : point < 0x10000 ? 3 : 4;
        if (bytes + length > 180) break;
        stem += character; bytes += length;
      }
      if (!stem) stem = `邮件-${uid}`;
      for (let index = 1; ; index++) {
        const path = `/Users/me/Downloads/${stem}${index === 1 ? "" : ` (${index})`}.eml`;
        if (manualDownloadPaths.has(path)) continue;
        manualDownloadPaths.add(path);
        return path;
      }
    },
    set_badge: () => null, open_link: () => null,
    save_attachment: () => "/Users/me/Downloads/附件.pdf", send_mail: () => new Promise((r) => setTimeout(r, 600)),
    add_account: () => { throw "登录失败：LOGIN failed（请确认填的是授权码/应用专用密码，并已开启 IMAP）"; },
    remove_account: () => null,
  };
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        await new Promise((r) => setTimeout(r, 30));
        if (!handlers[cmd]) throw `mock: 未实现 ${cmd}`;
        return structuredClone(await handlers[cmd](args || {}));
      },
    },
    event: { listen: async (name, listener) => {
      if (name === "mail") mailListeners.add(listener);
      return () => mailListeners.delete(listener);
    } },
  };
  // 预览不同状态：?empty 显示欢迎页，?open 打开第一封
  if (location.search.includes("empty")) {
    handlers.snapshot = () => ({ accounts: [], live: {}, autoDownloads: {}, bulkDownloads: {} });
  }
  if (location.search.includes("open")) {
    setTimeout(() => document.querySelector(".row")?.click(), 300);
  }
  if (location.search.includes("compose")) {
    setTimeout(() => document.querySelector('[data-action="compose"]')?.click(), 300);
  }
  if (location.search.includes("flow")) {
    // 和 App 里自动化测试相同的步骤：打开纯文本邮件 → 回复
    setTimeout(async () => {
      const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
      [...document.querySelectorAll(".row")].find((r) => r.textContent.includes("路线图")).click();
      await sleep(500);
      document.querySelector('[data-action="reply"]').click();
      await sleep(300);
      document.querySelector("#c-body").value = "收到，周三见。" + document.querySelector("#c-body").value;
    }, 400);
  }
  if (location.search.includes("dark")) {
    document.documentElement.style.colorScheme = "dark";
  }
})();
