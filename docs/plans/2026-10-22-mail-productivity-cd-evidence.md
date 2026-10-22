# 保存搜索与附件中心后端验证记录

日期：2026-10-22。范围仅 C/D 后端；未连接真实账户、未发送邮件。

## 先红后绿

先只增加以下三个行为测试，在未修改生产实现时运行实际 Rust 测试二进制的 `search_index::query_tests::` 过滤器，结果为 6 通过、3 失败：

- `productivity_migration_creates_persistent_views_without_taking_contacts_version`：预期两张持久表，实际为 0。
- `saved_search_can_sort_condition_only_results_oldest_first`：预期 UID `[1,2]`，实际为 `[2,1]`。
- `complete_mime_indexing_records_attachment_metadata_for_the_exact_source`：附件元数据表不存在。

随后实现保存视图、无词条件搜索排序、完整 MIME 附件元数据及真实读取/预览路径。

## 最新验证

- `cargo test -p mailbeacon-app --locked --lib search_index::query_tests::`：13 通过、0 失败。
- `cargo test -p mailbeacon-app --locked --lib saved_views::`：3 通过、0 失败。
- `target/debug/deps/mailbeacon_app_lib-91e857f54eaee3c9 attachment`：25 通过、0 失败。使用上一条 Cargo 命令刚编译的测试二进制，包含已有附件测试和新增测试；此过滤集合与查询集合有重叠，不能累加当作独立用例数。
- 自有变更 `git diff --check` 通过。

新增验证覆盖保存视图重启恢复、版本冲突、未来规则保留但禁执行、固定与动态范围隔离、纯条件升序、迁移失败回滚、完整身份与 MIME part 序号、同名附件与内联排除、元数据重启复用、多页稳定分页、内容/索引变更游标失效、代际隔离、删除回收、过期目录不回绑、rawHash/parserVersion 校验、危险格式拒绝、图片像素预算、预启动取消、账户级取消，以及取消期间排空共享 IMAP 读操作后丢弃结果。现有离线读取测试也在附件过滤器中通过。

## 原生 PDF 实测

在系统临时目录创建 602 字节合成单页 PDF；`/usr/bin/qlmanage -t -s 256 -o <临时目录> document.pdf` 返回 0，生成 2885 字节 PNG。随后启动 `qlmanage -p`，确认进程在运行；取消后退出并清理全部临时文件。生产代码以缩略生成做可取消预检，再启动完整 PDF 预览，持有临时文件直至关闭/取消/超时，并发布按 requestId 匹配的关闭事件。

此实测证明原生 PDF 渲染与进程生命周期；应用整体的焦点、Esc、切换附件和底层快捷键隔离仍需由前后端联调验证。没有把单页缩略图当作完整 PDF 预览，也没有宣称计划的 95 项验收均已执行。

## 接入边界

九个命令由主代理注册，未增加 AppState 字段。`cancel_account(accountId)` 应在账户删除/收件代际变化时调用。附件列表仅检索已解析元数据；显式补充选中范围才读取正文。未知目录覆盖返回 `catalogIncomplete`，不存在附件正文的缓存状态保持 `unknown`，不伪装成全邮箱完整索引。预览、打开和下载均使用完整来源身份、原始内容哈希和解析版本复核后的同一份字节。
