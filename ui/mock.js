// 只在普通浏览器里预览界面时使用：模拟 Tauri 后端，返回假数据。App 里不会加载。
(function () {
  const now = Date.now();
  const H = 3600e3;
  const accounts = [
    { id: "a1", kind: "gmail", email: "jarvis@gmail.com", name: "Jarvis Li", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465 },
    { id: "a2", kind: "exmail", email: "li@company.com", name: "李嘉文", imapHost: "", imapPort: 993, smtpHost: "", smtpPort: 465 },
  ];
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
  const folders = (role) => [
    { path: "INBOX", name: "收件箱", role: "inbox" },
    { path: "Drafts", name: "草稿", role: "drafts" },
    { path: "Sent", name: "已发送", role: "sent" },
    ...(role === "a1" ? [{ path: "[Gmail]/All Mail", name: "所有邮件", role: "all" }] : [{ path: "Archive", name: "归档", role: "archive" }]),
    { path: "Junk", name: "垃圾邮件", role: "junk" },
    { path: "Trash", name: "已删除", role: "trash" },
    ...(role === "a2" ? [{ path: "&UXZO1mWHTvZZOQ-", name: "项目资料", role: "other" }] : []),
  ];
  const all = () => [...inbox.a1, ...inbox.a2];
  const find = (a, u) => all().find((m) => m.accountId === a && m.uid === u);
  const html = `<div style="background:#f6f5f3;padding:28px 0"><div style="max-width:560px;margin:0 auto;background:#fff;padding:36px 40px;font-family:Georgia,serif;color:#222">
    <p style="text-align:center;color:#666;font-size:13px">October 15th, 2026<br><i>A newsletter by <a href="https://nesslabs.com">Anne-Laure Le Cunff</a></i></p>
    <p>Hi friends,</p><p>What do we lose when we outsource the process of discovery? This question has been on my mind these past few weeks, as I've followed the debate around claims to have solved hundreds of longstanding mathematical problems.</p>
    <p>As a hypercurious neuroscientist and writer, I know how much joy can be found in research and creative work. So beyond the very real practical concerns, I wanted to explore the human side of the debate.</p>
    <img src="https://example.com/banner.png" alt="" width="480"><p>Stay curious,<br>Anne-Laure</p></div></div>`;
  const handlers = {
    snapshot: () => ({
      accounts,
      live: {
        a1: { state: "online", detail: "实时推送", unread: inbox.a1.filter((m) => !m.seen).length, inbox: inbox.a1 },
        a2: { state: "online", detail: "实时推送", unread: inbox.a2.filter((m) => !m.seen).length, inbox: inbox.a2 },
      },
    }),
    list_folders: ({ accountId }) => folders(accountId),
    list_messages: ({ accountId, flaggedOnly }) => (flaggedOnly ? inbox[accountId].filter((m) => m.flagged) : []),
    get_previews: ({ accountId, uids }) => uids.map((u) => ({ uid: u, text: find(accountId, u)?._prev || "" })),
    get_message: ({ accountId, uid }) => {
      const m = find(accountId, uid);
      const isNews = m.fromAddr.includes("nesslabs");
      return {
        subject: m.subject, from: { name: m.fromName, addr: m.fromAddr },
        to: [{ name: "", addr: accounts.find((a) => a.id === accountId).email }], cc: [],
        date: m.date, html: isNews ? html : null,
        text: m._prev + "\n\n详细内容见附件，有问题随时找我。\n\n祝好\n" + m.fromName + "\nhttps://example.com/docs",
        hasRemote: isNews, attachments: m.attachment ? [{ index: 0, name: "Q4 路线图-初稿.pdf", size: 2_481_000 }, { index: 1, name: "数据附表.xlsx", size: 182_000 }] : [],
        messageId: "x@y", references: [],
      };
    },
    set_flag: () => null, move_messages: () => null, set_badge: () => null, open_link: () => null,
    save_attachment: () => "/Users/me/Downloads/附件.pdf", send_mail: () => new Promise((r) => setTimeout(r, 600)),
    add_account: () => { throw "登录失败：LOGIN failed（请确认填的是授权码/应用专用密码，并已开启 IMAP）"; },
    remove_account: () => null,
  };
  window.__TAURI__ = {
    core: {
      invoke: async (cmd, args) => {
        await new Promise((r) => setTimeout(r, 30));
        if (!handlers[cmd]) throw `mock: 未实现 ${cmd}`;
        return handlers[cmd](args || {});
      },
    },
    event: { listen: async () => () => {} },
  };
  // 预览不同状态：?empty 显示欢迎页，?open 打开第一封
  if (location.search.includes("empty")) {
    handlers.snapshot = () => ({ accounts: [], live: {} });
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
