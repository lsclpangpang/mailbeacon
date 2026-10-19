# MailPulse

简约风格的 macOS 邮件客户端，主打「新邮件到服务器后立刻出现」：每个邮箱保持一条常驻 IMAP IDLE 连接，由服务器主动推送。

- **收信**：多账号、所有收件箱合并视图、IDLE 实时推送（不支持 IDLE 的服务器自动改为 15 秒轮询）、列表显示正文摘要、系统通知、Dock 未读数
- **读信**：HTML 邮件放进沙盒显示，默认拦截远程图片（可「显示图片」或「始终显示此发件人的图片」）、内嵌图片、附件保存到「下载」；也可下载完整 `.eml` 邮件，保留原始邮件头、正文和附件
- **自动保存**：按邮箱设置本地目录，自动下载启用后新到收件箱的完整 `.eml` 邮件；保存进度跨重启保留，断线或重开应用后补下载，不改变邮件的已读状态
- **写信**：新邮件、回复、全部回复、转发。回复内容与原邮件引用分开编辑，保留原邮件的 HTML 排版、表格、链接和内嵌图片（普通附件不会自动随回复或转发带出）。每个账号可设置独立签名，写信时自动加入并可编辑。发出后会确认「已发送」里有这封邮件，没有就补存一份，有就不重复保存
- **整理**：星标、标为未读、删除（在废纸篓里再删就是彻底删除）、归档（Gmail 归档到「所有邮件」）、垃圾邮件、新建服务器文件夹（支持中文）、移动到任意文件夹或新建后立即移动、搜索当前列表、只看未读
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

「设置」中的每个邮箱账户可点击账户标题展开或收起，各账户独立控制；收起不会清空尚未保存的输入，账户标题仍显示进行中的全量下载进度。签名按账号填写并点击「保存签名」，清空后保存即可关闭。新邮件、回复和转发都会带入签名；切换发件账号会替换尚未修改的自动签名，已经手动修改的内容会保留。

回复时在上方输入新内容，下方显示保留格式的原邮件。远程图片仍按图片显示设置处理；原邮件为 HTML 时，发信同时带有 HTML 和纯文本版本，内嵌图片从原邮件中保留。尚未编辑时，取消、右上角 × 或 Esc 可直接关闭；修改后会显示「继续编辑 / 放弃修改」。发送过程中会暂时锁定窗口，避免重复发送或丢失正在发送的内容。

在列表里右键点击邮件，即可「下载邮件（.eml）」或「移动到…」，无需先打开邮件，也不会因此标为已读。键盘聚焦邮件后可按 Shift+F10 打开同一菜单。下载会保存到系统「下载」目录并在访达中显示，文件名采用邮件主题，同名文件不会覆盖；阅读区右上角「更多」也有下载和移动入口。侧栏「文件夹」旁的加号可新建文件夹；「移动到…」可选择已有文件夹，也可选择「新建文件夹并移动…」。

在「设置」中找到要保存的邮箱，启用「自动下载新邮件」，选择本地文件夹后保存设置。每个邮箱独立配置，默认关闭；只下载生效后新到收件箱的邮件，不会批量下载历史邮件。自动保存的 `.eml` 包含邮件头、正文和附件，已有文件不会被覆盖，也不会逐封弹出访达。

每个邮箱可选择一种关键词过滤模式：「不过滤」保存全部新邮件；「白名单」仅保存命中关键词的邮件；「黑名单」跳过命中关键词的邮件。黑白名单互斥，关键词每行一个，任意关键词出现在邮件主题、发件人名称或地址、正文中即算命中，不区分大小写，不匹配附件或 HTML 代码。最多 100 个关键词，每个最多 200 个字符；开启黑白名单时至少填写一个。切回「不过滤」会保留关键词，方便再次启用。手动下载单封邮件不受这些规则限制。

自动下载需要 MailPulse 运行；关闭应用时暂停，重新打开后会补下载关闭期间收到的新邮件。保存失败会在该邮箱的设置中显示原因并重试；命中规则成功保存或按规则跳过后记录进度，重启后不会反复处理。关闭自动下载不会删除已经保存的本地邮件。重新启用、更换保存目录或更改有效过滤规则时，会从当时的新邮件重新开始，不追溯旧邮件；保存未更改的设置会保留原下载进度。已有账户默认「不过滤」，升级后保持原下载行为。

要备份历史邮件，在「设置」中找到对应账号，在「下载此账号全部邮件」中选择本地文件夹并开始。该操作读取服务器上所有可访问的邮件文件夹，包括收件箱、已发送、草稿、自建文件夹、垃圾邮件和已删除；完整 `.eml` 原文含附件，按账号和邮件文件夹分别保存，不改变已读状态。邮件若同时出现在多个服务器文件夹中，会分别保留各文件夹的副本。

单封、自动和全量下载均以邮件主题命名，例如 `会议纪要.eml`；同名时使用 `会议纪要 (2).eml` 等安全编号，空主题回退为 `邮件-UID.eml`。文件名保留中文等 Unicode 文字，路径特殊字符会净化，过长主题会截短；这些处理只影响文件名，邮件原文字节和附件保持不变。

「继承该账号的黑白名单规则」默认不勾选，下载所有邮件；勾选后使用该账号已保存的过滤模式和关键词，黑名单跳过命中邮件，白名单只保存命中邮件。即使自动下载关闭，也可继承已保存的规则；如果该账号选择「不过滤」，仍保存全部邮件。修改规则后需先点击上方「保存设置」，再开始下载。每次任务使用开始时的规则，中途修改设置不影响当前任务；此选项不会更改自动下载设置或进度。已保存到本地的邮件不会因为更改规则而被删除。

全量下载先统计所有文件夹中实际存在的邮件，统计时显示已扫描文件夹数；统计完成后显示「已处理 / 总计」、百分比和进度条。总计是本次扫描固定的服务器邮件数（过滤前），同一邮件在不同文件夹的副本分别计数。已处理包含已保存、已存在、按规则跳过、单封处理失败及扫描后从服务器移除的邮件，明细分别显示；处理到 100% 不代表没有失败，应同时查看任务结果。文件夹扫描失败时不展示不完整的总数或百分比，统计中也可取消。

下载在后台运行，关闭设置或收起账户不会停止任务，退出应用会中断；再次选择同一父目录会重新扫描核对已有文件，保留已下载内容且不重复生成文件。已有文件内容不一致、邮件无法解码以检查规则或部分文件夹下载失败时会明确报错，不会覆盖文件或显示全部完成。下载范围以本次扫描的邮件边界为准，之后新到的邮件可由已启用的自动下载按其规则处理，或下次全量下载时补齐。

旧版保存的标识式文件名，会在同一目录再次下载到同一邮件、核对完整原文一致后迁移为主题名称；同名的其他文件会保留，通过编号避让。原有关联文件内容不一致时会保留文件并报错，不会覆盖。升级不会在后台批量改名，自动下载也不会追溯已经处理过的旧邮件。归档目录中的隐藏索引用于识别和核验已有邮件，请与邮件文件一并保留。

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
npm test                          # 签名、邮件操作和自动下载设置回归
```

邮件表格布局回归需使用真实浏览器：在项目根目录运行 `python3 -m http.server 8765 --bind 127.0.0.1`，打开 `http://127.0.0.1:8765/tests/mail-layout.html`。页面直接调用阅读器和回复引用的生产代码，检查宽、窄窗口下的表格宽度、居中、表头换行及图片拦截策略；不连接邮箱。

端到端测试连真实的邮件服务器跑完整流程（登录、IDLE 推送、收信、HTML/附件解析、标记、移动/删除、发信、补存已发送、回复头），默认跳过。用 Docker 起 GreenMail（IMAP + SMTP）和 Dovecot（IMAP）：

```sh
# 自签 CA + localhost 证书，GreenMail 需要 PKCS12 格式
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_full_flow -- --ignored --nocapture
MAILPULSE_E2E_IMAP=localhost:4993 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_imap_previews -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_folder_create_move_export -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_auto_download_new_mail_restart_and_retry -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_auto_download_keyword_filters -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_bulk_download_all_folders_resume_and_cancel -- --ignored --nocapture
# 继承规则测试另需空的 inherit@localhost / pw，避免共享邮箱数据干扰统计
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_bulk_download_inherited_filters -- --ignored --nocapture
# 下列进度测试还需要 progress@localhost / pw 隔离测试账号
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_bulk_download_progress_snapshot -- --ignored --nocapture
# 下列文件名测试还需要空的 names@localhost / pw 隔离测试账号
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_bulk_download_subject_filenames -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailpulse-app e2e_rich_reply_preserves_format_and_inline_image -- --ignored --nocapture
```

`MAILPULSE_EXTRA_CA` 指向的证书会被额外信任（IMAP 和 SMTP 都生效），自建邮件服务器也可以用。GreenMail 的部分 FETCH 回包不合 RFC（`BODY[]<0>{n}` 少了空格），摘要只能在 Dovecot 上测；App 里遇到这种服务器会自动停用该账号的摘要，其他功能不受影响。

debug 构建支持 `MAILPULSE_DEBUG_SCRIPT=flow.js`：启动 3 秒后把这段 JS 注入页面，用来在真实 App 里自动走界面流程。发布版不包含这段代码。

解析器安全回归通过子进程和 2 MiB 线程栈验证深层 MIME，覆盖普通邮件附件、base64、quoted-printable、digest、截断输入和内存复制放大。仓库内的 `vendor/mail-parser` 保留了上游许可证和最小补丁说明：限制嵌套层级与嵌套原文复制预算。超过安全上限的邮件会明确报错；需要解析正文的下载过滤不会将此错误当作跳过，也不会推进检查点。原文归档仍保留完整字节。

最近邮件原文缓存同时限制为 30 封和 64 MiB，超大邮件不留在缓存中。读取缓存前会重新确认服务器文件夹的 UIDVALIDITY；原邮件身份发生变化时，附件下载和引用内嵌图片会要求重新打开邮件。单封邮件下载采用临时文件写完后原子发布，避免中断留下看似完整的 `.eml`。


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

### macOS 本机签名

没有 Apple Developer 账号时，仓库默认使用 ad-hoc 签名，便于 GitHub Actions 构建。若本机已创建 `MailPulse Local Signing` 证书，使用下面的脚本构建会自动复用该证书，避免 macOS 每次把应用识别成新的程序并重复询问钥匙串权限：

```bash
./scripts/build-macos.sh --bundles app,dmg
```

本机证书只对创建它的 Mac 有效；其他机器会自动回退到 ad-hoc 签名。
