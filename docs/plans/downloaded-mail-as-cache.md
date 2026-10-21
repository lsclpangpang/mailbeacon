# 方案：把已下载的邮件当作只读缓存

> 状态：已实现（分支 `feat/archive-as-cache`）。实现与计划的差异见文末「实现记录」。
> 基线：main `65e4372`（v0.7.2，已包含 #14 邮件缓存设置、#15 本地列表索引、#16 已缓存邮件秒开等）。
> 交付：单独一个 PR，标题 `feat: 已下载的邮件直接用作缓存，不再重复下载`。

## 1. 目标

账号做过「自动下载」或「全量下载」时，本地已经有邮件的完整原文（`.eml`）。目标：

1. 读信、引用回复、转发、保存附件时，先用已下载的原文，不连服务器；
2. 后台预先缓存跳过已下载的邮件，不重复下载，也不占缓存空间；
3. 下载的文件是用户自己的，**只读**：不改、不删、不改时间、不计入缓存总量。

不做：

- 不把缓存里的邮件写进下载目录；
- POP3 账号不参与（编号只在本次运行内有效，对不上）；
- 不默认把已下载的邮件复制进缓存（见决定 D3）。

## 2. 现状（以代码为准）

| 项 | 位置 | 说明 |
|---|---|---|
| 保存原文 | `auto_download.rs::save_atomic_with_status` | 自动下载和全量下载都用它；内容是服务器原文，重复保存时用 `verify_original` 逐字节核对 |
| 身份名 | `auto_download.rs::archive_name` | `MailBeacon-<base64url(账号id)>-<UIDVALIDITY>-<UID>.eml`（只用到 checkpoint 里的 uid_validity） |
| 身份索引 | `<下载目录>/.mailpulse-index/identities/<身份名>.json` | 内容 `ArchiveRecord { version: 1, identity, filename }`，`filename` 是该目录下的实际文件名 |
| 读索引 | `auto_download.rs::read_record`（私有） | 已校验：版本、文件名不含路径分隔符和控制字符、以 `.eml` 结尾、不是符号链接 |
| 自动下载目录 | `Account.auto_download.directory` | 已持久化；只处理 **INBOX** |
| 全量下载目录 | `bulk_download::Status.directory` | **只在内存里，重启后丢失**；结构为 `<父目录>/<邮箱>-<stable_id(账号id)>/<文件夹名>-<stable_id(IMAP 路径)>/` |
| 目录身份 | `bulk_download.rs::identified_directory` | 每层目录有 `.mailpulse-identity`，内容 `account:<账号id>` / `folder:<IMAP 路径>` |
| 注意 | `account_directory` / `stable_folder_directory` | 这两个函数会**创建**目录和标记文件，查找时不能用，要另写只读版本 |
| 读原文入口 | `lib.rs::read_raw`、`read_opened`、`prefetch_raw` | 都先 `select_validity` 拿到 UIDVALIDITY，再 `cached_body` 查内存和磁盘缓存 |
| 秒开快路径（#16） | `lib.rs::get_message` | 本次会话确认过 UIDVALIDITY（`known_body_validity`，同步连接和每次打开都会记）时，**不连服务器**直接 `cached_body` 出邮件；后台再 SELECT 校验，UIDVALIDITY 变了就清掉这一代的内存和磁盘缓存 |
| 附件/引用快路径（#16） | `lib.rs::cached_raw` → `raw_at` | 同样不连服务器，直接 `cached_body` |
| 唯一的查缓存函数 | `lib.rs::cached_body(cache, key)` | 上面所有路径都经过它：内存 → `BodyCache.disk`（磁盘缓存）。只拿到 `MsgKey`，拿不到 `Account`，也拿不到服务器的邮件大小 |
| 本地列表索引（#15） | `search_index.rs` | 不存邮件大小，快路径上没有 RFC822.SIZE 可核对 |
| 预先缓存 | `cache_warm.rs::warm_folder` | 用 `BodyStore::cached_uids` 跳过已缓存的 UID |

结论：

- 给定「账号 + 文件夹 + UIDVALIDITY + UID」，能直接算出索引文件的路径，不用扫描文件；唯一缺的是全量下载目录的持久化。
- 只要把下载目录接进 `cached_body` 这一个函数，读信、秒开快路径、附件、引用、转发、预取就全部覆盖，不用逐个改调用点。

## 3. 设计决定（默认采用推荐项，开发前可改）

| 编号 | 问题 | 推荐 | 备选 |
|---|---|---|---|
| D1 | 全量下载目录是否持久化 | 是：每次全量下载成功启动时记入账号配置；设置里也能手动添加或移除 | 否：只用自动下载目录，覆盖面小 |
| D2 | 怎么判断下载的文件没被改过 | 不核对大小（秒开快路径根本拿不到服务器大小）：身份索引匹配 + 是普通文件 + 能解析成邮件就用。文件是本程序原子写入的原文，被手改的概率低 | 给 `ArchiveRecord` 加 `size`/`sha256`（version 2），以后下载的文件可以核对；老文件仍按推荐项处理 |
| D3 | 命中后是否复制进磁盘缓存 | 否：只放进内存缓存（30 封以内） | 是：适合下载目录在移动硬盘、经常不在线的情况 |
| D4 | 收件时间（INTERNALDATE）从哪来 | 用文件的修改时间兜底；界面显示本来就优先用邮件头的 Date | 再去服务器取一次 INTERNALDATE（多一次往返，不值得） |

## 4. 数据模型

### 4.1 账号配置（`config.rs`）

```rust
/// 用作只读缓存的下载目录
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveDir {
    /// 用户选的父目录（全量下载时选的那个；自动下载就是它的保存目录）
    pub path: String,
    /// "bulk"：全量下载（按账号/文件夹分子目录）
    pub kind: String,
}

pub struct Account {
    // ...
    #[serde(default)]
    pub archive_dirs: Vec<ArchiveDir>,
}
```

- 自动下载目录**不存进** `archive_dirs`，每次直接读 `auto_download.directory`（设置改了自动跟着变）。
- `Store::add_archive_dir(id, dir)`：去重（路径规范化后比较），落盘失败回滚；
- `Store::remove_archive_dir(id, path)`：只删记录，不碰磁盘。

### 4.2 运行时状态

`cached_body` 只拿得到 `MsgKey`，所以下载目录要预先登记到运行时对象里（做法同 `BodyStore::preset_limit`）：

- `BodyCache` 新增字段 `archives: Option<Arc<archive_source::ArchiveSource>>`，和 `disk` 并列；
- `ArchiveSource` 内含：
  - `dirs: HashMap<账号id, Dirs { auto: Option<PathBuf>, bulk: Vec<PathBuf> }>`：启动时按每个账号的配置登记；改自动下载设置（`lib.rs` 调 `set_auto_download` 处）、增删下载目录、删除账号时同步更新；
  - 「(账号, 父目录, 文件夹路径) → 文件夹子目录」的查找缓存，避免每次读信都扫描子目录。目录被删或改名时缓存失效（读取失败时清掉该项）。

## 5. 模块设计

### 5.1 `auto_download.rs` 新增（索引格式的知识只放在这里）

```rust
/// 只读：在一个下载目录里查这封邮件保存成了哪个文件。不加锁、不创建任何东西。
pub(crate) fn find_archived(dir: &Path, account_id: &str, uid_validity: u32, uid: u32)
    -> Result<Option<PathBuf>>;

/// 只读：列出一个下载目录里某账号、某一代 UIDVALIDITY 已保存的 UID（预先缓存用）。
pub(crate) fn archived_uids(dir: &Path, account_id: &str, uid_validity: u32)
    -> Result<HashSet<u32>>;
```

- `find_archived`：算出身份名，然后 `read_record(identities/<身份名>.json)`。要求 `record.identity == 身份名`；目标文件 `dir.join(record.filename)` 必须是普通文件，不能是符号链接。
- `archived_uids`：读一次 `identities/` 目录，按前缀 `MailBeacon-<base64url(账号)>-<UIDVALIDITY>-` 解析 UID。只看文件名，不打开文件。

### 5.2 `bulk_download.rs` 新增

```rust
/// 只读：在父目录下找这个账号的输出目录（看 .mailpulse-identity = "account:<id>"）
pub(crate) fn find_account_directory(parent: &Path, account_id: &str) -> Option<PathBuf>;
/// 只读：在账号目录下找这个文件夹的子目录（看 .mailpulse-identity = "folder:<path>"）
pub(crate) fn find_folder_directory(account_dir: &Path, folder_path: &str) -> Option<PathBuf>;
```

- 先试按规则算出的名字（`<邮箱>-<stable_id>`、`<文件夹名>-<stable_id>`）。对不上时，再遍历一层子目录比对标记，因为文件夹显示名可能变了。
- 标记文件和目录都要是普通的，不能是符号链接，用法同 `real_directory`。

### 5.3 新文件 `archive_source.rs`

```rust
pub struct ArchiveSource { folder_dirs: Mutex<HashMap<(String, PathBuf, String), PathBuf>> }

/// 登记/更新/移除一个账号的下载目录（POP3 账号不登记）
pub fn set_dirs(&self, account_id: &str, auto: Option<PathBuf>, bulk: Vec<PathBuf>);
pub fn remove_account(&self, account_id: &str);

/// 在这个账号的所有下载目录里找原文。任何错误都当作「没有」。
pub fn read(&self, account_id: &str, folder: &str, validity: u32, uid: u32)
    -> Option<(Vec<u8>, i64)>;

/// 这个文件夹里已下载的 UID（预先缓存跳过用）
pub fn uids(&self, account_id: &str, folder: &str, validity: u32) -> HashSet<u32>;
```

查找顺序：

1. 文件夹是 INBOX，并且自动下载目录不为空：在自动下载目录里 `find_archived`；
2. 对每个 `archive_dirs`（kind = bulk）：`find_account_directory`，然后 `find_folder_directory`（结果缓存），然后 `find_archived`。

读文件的规则：

- 大小超过 `body_store::MAX_FILE`（32 MB）不读；
- `crate::mime::parse` 解析失败，当作没有（决定 D2）；
- 日期取文件的修改时间（决定 D4）；
- **只用 `fs::read`，不调 `set_modified`，不写任何东西**；
- 整个函数是同步的，调用方放在 `spawn_blocking` 里跑。

### 5.4 接入点

| 位置 | 改动 |
|---|---|
| `lib.rs::cached_body` | 签名不变，顺序变成：内存 → 磁盘缓存 → `archives.read`（磁盘缓存和下载目录在同一个 `spawn_blocking` 里查，外面套 2 秒超时）。命中下载文件时只 `insert` 进内存缓存，不写磁盘缓存（D3） |
| `read_raw` / `read_opened` / `prefetch_raw` / `get_message` 快路径 / `cached_raw` | **不用改**，都经过 `cached_body` 自动生效 |
| `get_message` 后台校验 | 不用改：UIDVALIDITY 变了时已经会清掉这一代的内存缓存；下载目录按新 UIDVALIDITY 本来就查不到旧文件 |
| `BodyCache::with_disk` / 启动 setup | 带上 `ArchiveSource`，并按每个账号的配置 `set_dirs` |
| `cache_warm::warm_folder` | 跳过集合 = `cached_uids` ∪ `archives.uids`，两者在同一个 `spawn_blocking` 里算；已下载的计入 `done`；新增进度字段 `archived`（来自下载的封数） |
| `lib.rs::start_bulk_download` | 目录校验通过、任务启动后调 `Store::add_archive_dir` |
| 新命令 | `add_archive_dir(account_id)`（弹出选择目录）、`remove_archive_dir(account_id, path)`、`archive_status(account_id)` → `[{ path, kind, online, count }]` |
| 改自动下载设置（`set_auto_download` 命令） | 保存成功后 `archives.set_dirs` |
| 删除账号（`remove_account` 命令） | `archives.remove_account`，只清内存里的登记和查找缓存，不碰下载目录 |

### 5.5 界面（`ui/body-cache.js` + 设置页「邮件缓存」）

- 状态行增加：「另有 N 封来自已下载的邮件」；
- 「已下载的邮件」列表：每行显示目录路径、类型（自动下载 / 全量下载）、是否在线，可以移除（自动下载那一行不能移除，提示到自动下载设置里改）；
- 「添加已下载目录…」按钮。

## 6. 边界情况（每条都要有测试）

| # | 情况 | 期望 |
|---|---|---|
| E1 | 下载目录不存在（移动硬盘没插） | 当作没有；状态显示「不在线」 |
| E2 | 索引在，`.eml` 被删 | 当作没有 |
| E3 | `.eml` 被改成不是邮件的内容（解析失败） | 当作没有，写日志；内容被改但仍是合法邮件的情况按 D2 不检测 |
| E4 | 索引 JSON 损坏，或文件名含路径（`../x.eml`） | 当作没有（`read_record` 已拒绝） |
| E5 | `.eml` 或目录是符号链接 | 当作没有 |
| E6 | UIDVALIDITY 不同（文件夹重建过） | 不命中 |
| E7 | 邮件移到别的文件夹，UID 变了 | 不命中 |
| E8 | 同一个父目录里有别的账号的全量下载 | 看 `account:<id>` 标记，不会串号 |
| E9 | 文件夹显示名改了，子目录名和算出来的不一致 | 遍历比对 `folder:<path>` 标记后找到 |
| E10 | 下载正在写入同一个目录 | 只读不加锁；文件是临时文件改名发布的，读不到半封 |
| E11 | 超过 32 MB 的原文 | 不读，交给服务器 |
| E12 | POP3 账号 | 不查 |
| E13 | 读取前后文件的修改时间 | 不变 |
| E14 | 秒开快路径（不连服务器）上命中下载文件 | 直接出邮件，零条 IMAP 命令 |
| E15 | 下载目录在很慢的盘上 | 超过 2 秒当作没有，走服务器 |

## 7. TDD 步骤

每一步都按：**先写测试 → 跑测试确认失败（红）→ 实现到通过（绿）→ 整理 → 独立审查**。全部做完再整体审查一遍。

命令：

```sh
cd src-tauri && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=<共用 target> cargo test --lib -q <过滤词>
cd .. && npm test
```

测试数据统一用 `example.com`，不出现真实的组织名和邮箱。

### 第 1 步：只读索引查找（`auto_download.rs`）

先写测试（`auto_download::tests`）：

1. `find_archived_returns_the_file_saved_for_that_mail`：用 `save_atomic_with_status` 在临时目录存两封（UID 1、2），`find_archived(dir, "a", 7, 2)` 返回第 2 封的路径，内容和存进去的一样。
2. `find_archived_misses_other_generation_account_or_uid`：UIDVALIDITY 8、账号 "b"、UID 3 都返回 `None`。
3. `find_archived_rejects_tampered_index`：把索引 JSON 改成 `"filename": "../x.eml"`、改成坏 JSON、把 `.eml` 换成符号链接，都返回 `None` 或 `Err`（调用方都当作没有）。对应 E4、E5。
4. `find_archived_creates_nothing`：对空目录调用后，目录里仍然是空的（不能建出 `.mailpulse-index`）。
5. `archived_uids_lists_one_generation_of_one_account`：存账号 a 的 UIDVALIDITY 7 下 UID 1、2，UIDVALIDITY 8 下 UID 3，账号 b 下 UID 4；`archived_uids(dir, "a", 7) == {1, 2}`。

实现：`find_archived`、`archived_uids`，复用 `archive_name` 和 `read_record`。

### 第 2 步：只读目录定位（`bulk_download.rs`）

先写测试：

1. `finds_account_and_folder_directories_by_identity`：用现有的 `account_directory` 和 `stable_folder_directory` 建好结构，`find_account_directory` 和 `find_folder_directory` 都能找到。
2. `finds_folder_after_display_name_changed`：把文件夹子目录改名（标记不变），仍然能找到（E9）。
3. `does_not_mix_accounts`：同一个父目录里有账号 a、b 两个子目录，查 b 只返回 b 的（E8）。
4. `lookup_creates_nothing_and_ignores_symlinks`：目录不存在时返回 `None` 且不创建；标记文件或目录是符号链接时返回 `None`（E5）。

实现：两个 `find_*` 函数。

### 第 3 步：账号配置（`config.rs`）

先写测试：

1. `archive_dirs_default_empty_and_persist`：老配置读出来是空列表；`add_archive_dir` 后重新 `load` 能读到；同一路径加两次只有一条。
2. `archive_dir_changes_roll_back_on_save_failure`：仿照现有用例，让 `accounts.json.tmp` 是个目录，`add_archive_dir` 报错，内存里的值不变。
3. `removing_an_archive_dir_only_forgets_it`：移除后磁盘上的目录还在。

实现：`ArchiveDir`、两个 `Store` 方法，并给测试里所有 `Account { .. }` 字面量补上 `archive_dirs: Vec::new()`。

### 第 4 步：`ArchiveSource`（新模块）

先写测试（临时目录 + `save_atomic_with_status` 造数据）：

1. `reads_inbox_mail_from_the_auto_download_directory`：`set_dirs("a", Some(自动下载目录), [])` 后，`read("a", "INBOX", 7, 1)` 返回原文。
2. `auto_download_directory_is_only_used_for_inbox`：同样的数据，用文件夹 "Archive" 查，返回 `None`。
3. `reads_any_folder_from_a_bulk_directory`：`archive_dirs` 里有一个全量下载目录，查 "Archive" 的 UID 能命中。
4. `unparsable_file_is_ignored`：把文件改成乱码后返回 `None`（E3）。
5. `missing_directory_or_file_is_just_a_miss`：E1、E2。
6. `reading_never_touches_the_files`：读之前记下修改时间、大小、目录列表，读之后完全一样（E13）。
7. `oversize_file_is_not_read`：E11（用 sparse 文件或小一点的上限常量注入）。
8. `folder_directory_lookups_are_cached_and_dropped_when_stale`：第二次查同一个文件夹不再遍历子目录（用计数或查看缓存）；把目录删掉后，下一次查找能正确返回 `None` 并清掉缓存项。
9. `uids_unions_all_directories`：自动下载目录和全量下载目录各有一部分 UID，`uids` 返回并集。
10. `unregistered_or_removed_account_reads_nothing`：没 `set_dirs` 的账号、`remove_account` 之后的账号，`read` 都是 `None`。

实现：`archive_source.rs`。

### 第 5 步：接入读原文路径（`lib.rs`）

先写测试（沿用 `command_tests` 的 `scripted_session`：服务器只回写好的应答，多发一条命令就会断开报错，正好用来证明「没连服务器」）：

1. `cached_body_falls_back_to_downloaded_mail`：纯单测，内存和磁盘缓存为空、下载目录里有 UID 1，`cached_body` 返回下载的原文；顺序是内存 → 磁盘缓存 → 下载目录（磁盘缓存里有同一封时用磁盘缓存的）。
2. `opening_a_downloaded_mail_needs_only_select`：脚本只给 `selected(7)`，`read_opened` 返回下载的原文，并且没有发 `RFC822.SIZE`/`BODY.PEEK[]`。
3. `downloaded_mail_goes_to_memory_cache_only`：命中后 `BodyCache` 里有，`BodyStore` 里没有（D3），缓存占用不变。
4. `quote_and_attachment_paths_use_downloaded_mail`：`read_raw(peek=true, expected_validity=Some(7))` 命中下载的原文；`cached_raw(.., Some(7))` 不连服务器也命中（E14）。
5. `fast_open_uses_downloaded_mail_without_server`：`remember_body_validity("a","INBOX",7)` 后走 `get_message` 快路径用到的同一段逻辑，零条 IMAP 命令出邮件（E14）。如果快路径还写在命令函数里不好测，先把它抽成 `open_known(state, key) -> Option<MessageView>` 再测。
6. `prefetch_skips_downloaded_mail`：`prefetch_raw` 返回 `false`，不发 FETCH。
7. `slow_archive_read_times_out_as_miss`：注入一个会阻塞的读取（测试用的钩子或很小的超时常量），超时后 `cached_body` 返回 `None`（E15）。
8. `pop3_never_consults_archives`：POP3 的 `pop3_raw` 路径不变（E12），用现有测试加断言即可。

实现：`BodyCache` 加 `archives` 字段，`cached_body` 加第三级来源和超时；setup 里登记每个账号的目录；`set_auto_download`、`remove_account` 命令里同步更新。

### 第 6 步：预先缓存跳过已下载的邮件（`cache_warm.rs`）

先写测试（`test_imap` 假服务器 + 临时下载目录）：

1. `warm_up_skips_mail_already_downloaded`：收件箱 3 封，其中 2 封已下载，只发 1 次 `BODY.PEEK[]`，内容只含第 3 封的 UID；`done == total == 3`，`archived == 2`。
2. `downloaded_mail_does_not_use_cache_space`：做完后磁盘缓存只有 1 封。
3. `offline_archive_directory_is_ignored`：下载目录不存在时照常把 3 封都缓存。

实现：`Plan` 增加 `archived: HashSet<u32>`（由调用方在 `spawn_blocking` 里算好传入），`Progress` 增加 `archived` 字段。

### 第 7 步：命令和全量下载登记（`lib.rs`）

先写测试：

1. 把「全量下载启动后登记目录」抽成纯函数 `register_bulk_archive(store, account_id, parent)` 再测：登记后 `archive_dirs` 有这一项，重复登记不重复。
2. `archive_status` 抽成纯函数 `archive_status_of(acct, source) -> Vec<Status>`：在线和离线目录都列出来，自动下载目录那一项 `removable: false`。

实现：三个命令，并在 `start_bulk_download` 里调用登记。

### 第 8 步：前端（`ui/body-cache.js`、`ui/app.js`）

先写测试（`tests/body-cache.test.js` 追加）：

1. `statusText` 在 `archived > 0` 时多出「另有 N 封来自已下载的邮件」；
2. `archiveRows(status)` 返回的行文字和是否可移除：自动下载那行不能移除，离线的标「不在线」；
3. 设置页点「移除」会调 `remove_archive_dir`，成功后刷新列表；点「添加已下载目录…」会调 `add_archive_dir`；
4. 老的测试脚手架照样能跑（mock.js 补上三个命令）。

实现：纯函数放在 `body-cache.js`，DOM 部分放在 `app.js`。用无头 Chrome 加 mock 截图检查亮色和暗色两种界面。

### 第 9 步：收尾

- `cargo clippy --all-targets`：不新增警告；
- `npm run test:e2e` 全过；
- README 的「邮件缓存」一段写清楚：已下载的邮件只读使用、不计入总量、不修改；
- CHANGELOG `## 未发布` 加一条；
- 检查 diff 里没有真实的组织名或邮箱；
- 找独立审查，有问题就修，修完复审，通过后开 PR。

## 8. 手动验证清单

1. 对一个账号做全量下载，完成后断网，打开收件箱和其他文件夹里的历史邮件，都能直接打开。
2. 打开前后看下载目录里文件的修改时间，没有变化。
3. 设置页「邮件缓存」显示「另有 N 封来自已下载的邮件」，「立即更新缓存」只下载剩下的邮件，缓存占用增加的量和剩下的邮件大小相当。
4. 把下载目录改个名（模拟移动硬盘拔掉），设置页显示「不在线」；打开邮件会从缓存或服务器取，不报错。
5. 手动把一个 `.eml` 改成乱码，打开它时显示服务器上的原文。
6. 打开过一封收件箱邮件后（UIDVALIDITY 已确认），断网再点开另一封已下载但没缓存过的邮件，能秒开。

## 9. 风险

| 风险 | 应对 |
|---|---|
| 下载目录在网盘或移动硬盘上，读文件很慢 | 读取在 `spawn_blocking` 里做，`cached_body` 里加 2 秒超时，超时当作没有（E15） |
| 下载的文件被改过但仍是合法邮件（D2 不检测） | 只影响本地显示，服务器上的邮件不受影响；需要时按 D2 备选给索引加校验值 |
| 秒开快路径用的 UIDVALIDITY 过期（文件夹刚被重建） | 和现有磁盘缓存同样的风险，后台校验后清掉内存里的这一代；下次打开按新 UIDVALIDITY 查不到旧文件 |
| 用户以为删除缓存会删掉下载的文件 | 文案明确写「只读使用，不会修改或删除」 |
| 下载索引格式以后变了 | 格式知识只放在 `auto_download.rs` 里，用 `find_archived` 统一封装；`read_record` 已校验 version 字段 |
| 父目录里有大量子目录，遍历慢 | 先试按规则算出的名字，只有对不上才遍历，结果缓存 |

## 10. 实现记录（与计划的差异）

- `archive_dirs` 用 `Vec<String>`（只存全量下载的父目录），没有 `kind` 字段：目前只有全量下载一种需要记录，自动下载目录仍直接读 `auto_download.directory`。
- `archived_uids` 返回 `BTreeSet`，目录读不了时返回空集合而不是 `Err`；`find_archived` 也认旧版直接以身份命名的文件。
- 查找父目录下的账号/文件夹子目录时，不需要邮箱或显示名：先看名字以 `-<stable_id>` 结尾的，找不到再逐个比对 `.mailpulse-identity`。没找到的文件夹 30 秒内不重复扫描。
- 「是不是邮件」的判断：能解析，并且至少有 From / Date / Subject / Message-ID 之一（mail-parser 对乱码也能「解析」出东西）。
- 读取超时用通用的 `blocking_within(limit, f)`，测试直接测它，没有在生产代码里加钩子。
- 手动添加目录时只读校验（不写探测文件）：目录里必须有这个账号的全量下载；选成账号目录本身也认。
- 第 7 步「登记目录」没有抽成单独的纯函数：就是 `Store::add_archive_dir` + `ArchiveSource::register`，两者各自有测试。
- 状态行里的文字是「其中 N 封直接用已下载的邮件」，跟在「正在缓存 / 已全部缓存」后面。
- 审查后补充：
  - 读下载目录统一走 `ArchiveSource::within`（限时；读信超时后这个账号 60 秒内不再读下载目录）。后台预先缓存用 `scan_within`，超时不标记账号。
  - 预先缓存只确认「还没缓存过」的 UID 是否真能读到（索引在但文件被删、太大的不算），最多花 20 秒，到时间返回已确认的部分，剩下的照常缓存。
  - 读文件：lstat → open → fstat，核对 dev/ino，按打开的文件判断大小，`take` 限量读取。
  - 自动下载目录如果是别的文件夹的全量下载目录（`.mailpulse-identity` 不是 `folder:INBOX`），不用于收件箱；相对路径不用。
  - 设置页目录列表：请求带序号防止旧回复覆盖；不阻塞缓存状态；全量下载开始、自动下载设置保存后自动刷新；读取失败显示空列表。
