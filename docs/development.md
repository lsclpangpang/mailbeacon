[返回首页](../README.md) · [使用说明](usage.md)

# 开发指南

以下命令均从仓库根目录执行；CLI 示例会在代码块内切换目录。

## 代码目录

- `src-tauri/src/`：Rust 后端。`sync.rs` / `imap.rs` 负责收信，`smtp.rs` 负责发信，`config.rs` / `credentials.rs` 管理账号与本地凭据。
- `drafts.rs`、`contacts.rs`、`prefs.rs`、`rules.rs`、`threads.rs`、`templates.rs`、`outbox.rs` 分别负责草稿、通讯录、通知偏好、规则、会话、模板和定时发送。
- `ui/`：原生 HTML/CSS/JS，无需前端打包；`mock.js` 提供浏览器演示数据。
- `tests/` / `scripts/*-e2e.mjs`：界面逻辑测试与浏览器交互回归；`cli/` 是早期收信速度原型。

## 运行与构建

```sh
npm ci                     # 安装 Tauri CLI
npx tauri dev              # 在本机运行
npx tauri build --target universal-apple-darwin --bundles dmg
```

Windows 本地开发需安装 MSVC 的 C++ 桌面构建工具、Rust MSVC 工具链、Node.js 和 WebView2，参照 [Tauri Windows 环境要求](https://v2.tauri.app/start/prerequisites/#windows)。在 Windows 执行：

```sh
npm ci
npx tauri dev
npx tauri build --target x86_64-pc-windows-msvc --bundles nsis
```

`tauri.windows.conf.json` 自动合并 Windows 配置：完整主窗口定义、当前用户安装、WebView2 引导安装及通知协议。保留标准 NSIS 目录选择页，允许其他盘符及中文、空格路径；升级复用原目录。账户与邮件数据仍在用户数据目录，安装目录选择只决定程序位置。输出在 `target/x86_64-pc-windows-msvc/release/bundle/nsis/`。不在 Mac 上把通过类型检查等同于 Windows 安装验证。

只看界面可直接用浏览器打开 `ui/index.html`，自动加载模拟数据；地址加 `?open` 打开第一封邮件，`?compose` 打开写信，`?empty` 显示欢迎页。

## 测试与调试

```sh
cargo test -p mailbeacon-app        # Rust 单元测试及内存 IMAP 服务器协议测试
npm test                          # 界面逻辑回归
npm run test:e2e                   # 使用 mock.js 的浏览器交互回归
```

浏览器回归不连接邮箱，需要本机可执行 `google-chrome`，覆盖写信、规则、通知、会话、模板、定时发送、阅读器、侧栏、下拉框和附件菜单。

邮件表格布局需在真实浏览器检查。启动静态服务后打开 `http://127.0.0.1:8765/tests/mail-layout.html`，验证宽窄窗口下的表格、表头换行和图片拦截；不连接邮箱。

```sh
python3 -m http.server 8765 --bind 127.0.0.1
```

<details>
<summary>真实邮件服务器端到端测试（默认跳过）</summary>

使用 Docker 准备 GreenMail（IMAP + SMTP）和 Dovecot（IMAP），测试登录、IDLE、收发信、附件、标记、移动、下载和回复引用。准备自签 CA 与 localhost 证书，GreenMail 使用 PKCS12 格式；下列命令假定 CA 文件位于仓库根目录的 `ca.pem`。

```sh
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_full_flow -- --ignored --nocapture
MAILPULSE_E2E_IMAP=localhost:4993 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_imap_previews -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_folder_create_move_export -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_auto_download_new_mail_restart_and_retry -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_auto_download_keyword_filters -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_bulk_download_all_folders_resume_and_cancel -- --ignored --nocapture
# 另需空的 inherit@localhost / pw，避免共享邮箱数据干扰统计
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_bulk_download_inherited_filters -- --ignored --nocapture
# 另需 progress@localhost / pw 隔离测试账号
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_bulk_download_progress_snapshot -- --ignored --nocapture
# 另需空的 names@localhost / pw
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_bulk_download_subject_filenames -- --ignored --nocapture
MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
  cargo test -p mailbeacon-app e2e_rich_reply_preserves_format_and_inline_image -- --ignored --nocapture
```

`MAILPULSE_EXTRA_CA` 对 IMAP 和 SMTP 均生效，也可用于自建服务器。GreenMail 的部分 FETCH 回包不合 RFC，摘要测试需使用 Dovecot；应用遇到该情况会停用该账号的摘要。

</details>

debug 构建支持 `MAILPULSE_DEBUG_SCRIPT=flow.js`：启动 3 秒后注入指定 JS，用于真实 App 界面调试；发布版不包含该入口。

## 检查与发版

「检查」仅在 PR 创建/更新及手动运行时触发，支持以其他 PR 分支为基线；普通分支推送不重复检查。macOS 运行格式、构建和前后端测试；Windows 运行测试、构建 x64 安装包并在隔离 runner 上安装、启动 WebView2 和卸载，保存日志与截图。PR 另有标题检查。

合并到 `main` 后，「发布」只计算并回写一次版本。macOS 与 Windows 任务检出同一个确切提交，分别构建 universal DMG 和 x64 EXE；核对版本、提交及文件摘要，两边均通过才公开同一 Release。只有准备版本的任务持有 `release-main` 环境中的回写密钥，平台构建任务不接触该密钥。

| PR 标题 | 版本变化 |
| --- | --- |
| `fix: 修复……` | 末位，如 0.2.0 → 0.2.1 |
| `feat: 新增……` | 中间位，如 0.2.0 → 0.3.0 |
| `feat!: ……`（任何类型后加 `!`） | 首位，如 0.2.0 → 1.0.0 |
| `docs:` `test:` `ci:` `build:` `chore:` `refactor:` `style:` | 不发新版本 |

- 可加范围，如 `fix(search): 中文搜索`；冒号支持英文 `: ` 或中文 `：`。不合规标题会导致检查失败，修正后自动重跑。
- 以最新 `v*` tag 为基准，汇总之后所有 PR，按其中最高升级级别发版；未成功发布的变更会在下次补上。自动生成的 `Revert "fix: ……"` 按修复处理。
- 只改 `.md` 的合并不触发发布，变更会计入下次发布。发版后单独修改 `CHANGELOG.md` 则刷新发布说明，不重新打包。
- 工作流同步 Tauri、Cargo、npm 版本及 `CHANGELOG.md`，以 `chore(release): v<版本>` 提交回 `main`，tag 指向该提交；通常不必手动改版本。
- 发布说明默认取 PR 标题和编号；详细说明可写在 `CHANGELOG.md` 最前面的 `## 未发布` 中，发版时替换成版本号。
- 必须指定版本时，使用 `npm run release:version -- 1.0.0` 并补好对应更新记录；只有高于已有 tag 的版本才按指定值发布。手动运行「发布」按当前版本重打包；已有 tag 必须指向同一提交，否则拒绝覆盖安装包。

```sh
npm run release:check
npm run release:notes -- v0.2.0
node scripts/release.js title "fix: ……"
```

`CHANGELOG.md` 以 `# 标题` 开头，每个版本使用 `## 0.3.0` 或 `## v0.3.0`。版本不一致、缺少对应记录或仍写「（待补充）」会使检查和发布失败。标题检查是否阻止合并取决于仓库分支保护设置。

## macOS 本机签名

默认使用 ad-hoc 签名。若本机已有 `MailPulse Local Signing` 证书，以下脚本会自动复用；其他机器回退为 ad-hoc 签名。本地凭据存储不依赖固定签名。

```sh
./scripts/build-macos.sh --bundles app,dmg
```

Windows 安装包暂未签名；公开分发可配置 [Windows 代码签名](https://v2.tauri.app/distribute/sign/windows/)。签名与本地授权码的加密存储无关。

Windows 差异：PDF 交给默认应用，图片仍在应用内预览；附件「打开方式」调用系统选择器，文件管理器入口显示资源管理器。通知通过本机随机票据链接激活应用，启动完成后才定位邮件，账户接收身份不符或票据过期时不打开旧邮件。验收清单见 [Windows 适配记录](plans/2026-10-22-windows-support.md)。

## 实现导航

- **缓存**：查找顺序为内存 → 磁盘缓存 → 下载目录 → 服务器。内存最多 30 封 / 64 MiB；IMAP 磁盘缓存默认 2048 MB、单封超过 32 MiB 不存；预先缓存默认单封上限 5 MB。见 `src-tauri/src/body_store.rs`、`archive_source.rs` 和 `lib.rs`；POP3 不使用磁盘正文缓存及下载目录复用。
- **下载目录复用**：通过账号、文件夹和邮件身份索引定位，按 UIDVALIDITY 校验；只读原文件，不占磁盘缓存配额。离线、无效、超过 32 MiB 或读取超过 2 秒时回退服务器。详见 [下载邮件作为缓存的设计](plans/downloaded-mail-as-cache.md)。
- **索引与列表**：文本搜索查询 `local.db` 的 `mail_index`，后台增量填入摘要和正文开头；首次索引期间只返回已建部分。列表复用本地摘要，前端只渲染可见行；相关入口见 `src-tauri/src/search_index.rs` 和 `ui/app.js`。
- **解析器**：`vendor/mail-parser`（包名 `mailbeacon-parser`）限制 MIME 嵌套和原文复制预算；安全回归通过子进程和 2 MiB 线程栈验证。超限明确报错，下载过滤不会当作跳过或推进检查点，原文归档仍保留完整字节。
- **凭据**：`credentials.rs` 使用 AES-256-GCM，本地数据库和随机密钥分开保存；备份与迁移注意事项见 [使用说明](usage.md)。

## 收信速度原型（CLI）

每个邮箱保持 IMAP IDLE 连接，打印新邮件及「服务器收到 → 本地显示」延迟。

```sh
cd cli
cp accounts.example.toml accounts.toml && chmod 600 accounts.toml   # 填授权码
cargo run --release
```

同一个邮箱配置两次，其中一个加 `mode = "poll"`，即可比较 IDLE 和轮询。服务器时间只精确到秒，延迟有约 1 秒误差。
