# MailPulse

简约风格的 macOS 邮件客户端，主打「新邮件到服务器后立刻出现」：每个邮箱保持一条常驻 IMAP IDLE 连接，由服务器主动推送。

- **收信**：多账号、所有收件箱合并视图、IDLE 实时推送（不支持 IDLE 的服务器自动改为 15 秒轮询）、列表显示正文摘要、系统通知、Dock 未读数
- **读信**：HTML 邮件放进沙盒显示，默认拦截远程图片（可「显示图片」或「始终显示此发件人的图片」）、内嵌图片、附件保存到「下载」
- **写信**：新邮件、回复、全部回复、转发（纯文本；转发暂不带原附件）。发出后会确认「已发送」里有这封邮件，没有就补存一份，有就不重复保存
- **整理**：星标、标为未读、删除（在废纸篓里再删就是彻底删除）、归档（Gmail 归档到「所有邮件」）、垃圾邮件、移动到任意文件夹、搜索当前列表、只看未读
- **账号**：腾讯企业邮、QQ、163、126、Gmail 自动填服务器；其他邮箱手动填写。授权码存在系统钥匙串，不落盘

## 安装测试版

1. 打开仓库的 Actions →「打包 macOS」→ 最新一次运行，下载 `MailPulse-macOS`，解压得到 dmg（通用版，Apple Silicon 和 Intel 都能用）
2. 打开 dmg，把 MailPulse 拖进「应用程序」
3. 没有 Apple 开发者证书签名，第一次打开会被拦截。在终端执行一次：
   ```sh
   xattr -dr com.apple.quarantine /Applications/MailPulse.app
   ```
   或者先双击打开一次，再到「系统设置 → 隐私与安全性」里点「仍要打开」
4. 添加邮箱时填写**授权码**，不是网页登录密码（各家的获取方式在添加页面有提示）
5. 第一次读取授权码时 macOS 会询问是否允许访问钥匙串，选「始终允许」

快捷键：`J`/`K` 或 `↓`/`↑` 上下封，`R` 回复，`A` 全部回复，`F` 转发，`E` 归档，`S` 星标，`U` 标为未读，`⌫` 删除，`/` 或 `⌘F` 搜索，`⌘N` 写邮件，`⌘↩` 发送，`Esc` 关闭。

## 开发

```
src-tauri/   Rust 后端：IMAP 同步（sync.rs）、读信/文件夹（imap.rs）、发信（smtp.rs）、账号（config.rs）
ui/          界面：原生 HTML/CSS/JS，不需要打包工具；mock.js 是浏览器预览用的假数据
cli/         最早的收信速度原型（命令行），见下文
```

```sh
npm ci                     # 安装 Tauri CLI
npx tauri dev              # 本地运行（Mac 上）
npx tauri build --target universal-apple-darwin --bundles dmg
```

只看界面不需要编译：用浏览器直接打开 `ui/index.html`，会自动加载 `mock.js` 里的假数据。地址后加 `?open` 打开第一封邮件，`?compose` 打开写信，`?empty` 显示首次使用的欢迎页。

### 测试

```sh
cargo test -p mailpulse-app        # 单元测试
```

端到端测试连真实的邮件服务器跑完整流程（登录、IDLE 推送、收信、HTML/附件解析、标记、移动/删除、发信、补存已发送、回复头），默认跳过。用 Docker 起 GreenMail（IMAP + SMTP）和 Dovecot（IMAP）：

```sh
# 自签 CA + localhost 证书，GreenMail 需要 PKCS12 格式
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_full_flow -- --ignored --nocapture
MAILPULSE_E2E_IMAP=localhost:4993 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_imap_previews -- --ignored --nocapture
```

`MAILPULSE_EXTRA_CA` 指向的证书会被额外信任（IMAP 和 SMTP 都生效），自建邮件服务器也可以用。GreenMail 的部分 FETCH 回包不合 RFC（`BODY[]<0>{n}` 少了空格），摘要只能在 Dovecot 上测；App 里遇到这种服务器会自动停用该账号的摘要，其他功能不受影响。

debug 构建支持 `MAILPULSE_DEBUG_SCRIPT=flow.js`：启动 3 秒后把这段 JS 注入页面，用来在真实 App 里自动走界面流程。发布版不包含这段代码。

## 已处理的情况

- **163/126**：登录后先发 IMAP `ID` 命令，否则会报 "Unsafe Login"
- **服务器不支持 IDLE**：自动降级为轮询
- **断线**：立即重连，连续失败才逐步拉长间隔（最长 60 秒）；重连后补收断线期间的邮件，并同步别处做的已读、删除
- **电脑睡眠唤醒**：检测到系统时钟跳变后，所有连接立即重连
- **连接静默失效**：开启 TCP 保活；每次操作超过 20 秒没响应就判定失效；空闲超过一分钟的连接用之前先探活
- **密码错误**：直接停止该账号并提示，不反复重试，避免触发风控
- **SMTP 端口**：587/25/2525 用 STARTTLS，其余（465 等）直接 TLS

## 收信速度原型（cli/）

最早用来验证 IDLE 推送速度的命令行工具：每个邮箱一条常驻连接，新邮件一到就打印出来，并显示「服务器收到 → 本地显示」的延迟，可以和 Apple Mail 对比谁先收到。

```sh
cd cli
cp accounts.example.toml accounts.toml && chmod 600 accounts.toml   # 填授权码
cargo run --release
```

想对比 IDLE 和轮询：同一个邮箱配置两次，其中一个加上 `mode = "poll"`。服务器时间只精确到秒，延迟有大约 1 秒误差。
