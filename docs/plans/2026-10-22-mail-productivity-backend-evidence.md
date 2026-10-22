# A/B 后端实现与 TDD 证据（2026-10-22）

实现范围：持久发送缓冲与撤销、稍后处理、未回复提醒、普通/定时发送的写信会话占用。未连接真实邮箱，未发送真实邮件。IMAP 行为通过本地假服务器测试。

## 红灯记录

以下断言均在对应生产修复前执行失败；类型错误不计为红灯。

| 用例/命令过滤器 | 修复前结果 | 当次日志 |
| --- | --- | --- |
| `accepted_timestamp_is_persistent` | 1 failed：缺少独立、持久的 acceptedAt | 本轮工具输出（首次编译后） |
| `mail_reminders_`（重复回复版本） | 6 passed / 1 failed：同一邮件头重复同步再次修改提醒 | `/tmp/mail-productivity-reminder-red.log` |
| `send_buffer_manual_retry` | 1 failed：重发成功后没有继承跟进提醒 | `/tmp/mail-productivity-retry-red.log` |
| `send_buffer_`（会话占用版本） | 10 passed / 1 failed：撤销和重启后旧会话不应恢复编辑 | `/tmp/mail-productivity-session-red.log` |
| `mail_reminders_`（历史回复版本） | 7 passed / 1 failed：历史邮件首次索引误唤醒稍后任务 | `/tmp/mail-productivity-historical-red.log` |
| `mail_reminders_`（源失效版本） | 8 passed / 2 failed：被删账户可改时间、UIDVALIDITY/UID 失效未处理 | `/tmp/mail-productivity-source-red.log` |
| `productivity_` | 2 passed / 4 failed：同会话二次发布、失败发布残留 MIME、跨文件夹代际变化、定时会话占用 | `/tmp/mail-productivity-review-red.log` |
| `send_buffer_active_sends` | 1 failed：历史分页之外的暂停/提交中任务不可见 | `/tmp/mail-productivity-active-red.log` |

## 绿灯记录

最终命令：

```sh
cargo test -p mailbeacon-app --locked --lib
```

结果：**516 passed / 0 failed / 12 ignored**，测试运行 8.57 秒。日志 `/tmp/mail-productivity-full-backend-green.log`。

前置定向回归 `--lib send_records` 为 33 passed；`--lib drafts` 为 23 passed，分别保存在 `/tmp/mail-productivity-backend-green.log`、`/tmp/mail-productivity-drafts-green.log`。后续补充的会话、源失效及活跃列表测试已包含在最终全量结果中。现有 vendor parser 的 cfg 警告和既有未使用方法警告未在此任务扩大修改。

## 关键约束

- `send_records` 是唯一执行状态权威；原文和完整草稿先持久化，短 SQLite 事务发布元数据；同请求幂等，同会话第二个原始请求拒绝。未发布事务残留的原文允许安全重建，已经发布的内容不可替换。
- 撤销与领取通过同一状态事务竞争；撤销持久恢复意图，不等待网络；重启将缓冲变为暂停、提交中变为未知。未知结果不自动重发。
- Accepted、不可变 acceptedAt 和跟进提醒在同一事务提交；手动重发创建新提交并继承提醒意图。复制已发送副本不改 acceptedAt。
- 撤销恢复保留 Bcc、回执、正文和附件；本地草稿编号/恢复映射耐重启且幂等，不覆盖已编辑草稿、不复活被删除草稿。
- 稍后引用包含账户、receiveEpoch、文件夹、UIDVALIDITY、UID；锚点来自精确本地摘要。无 Message-ID 的邮件明确只按时间恢复。
- 未回复负向判断要求到期后开始的完整目录扫描；获取真实完整邮件头、排除自动回复/回执/自身邮件；首扫全 UID、后续仅取未索引身份，逐夹并整轮结束复核代际。失败、取消、目录变更保留待核对。展示最早快照时间作为保守核对截至时间。
- 定时发送在 outbox 原子描述文件中保存会话，删除前持久化占用墓碑；恢复入口同时检查普通和定时发送，跨入口共享发布门。旧定时数据按 serde 默认值兼容。

边界：POP3 的持久稍后/回复跟进不支持；定时发送与跟进组合由入口明确拒绝。应用关闭时不执行；重新打开缓冲需用户确认。系统通知为尽力投递，持久提醒列表为权威。真实 SMTP、真实服务商 IMAP、macOS 通知授权与应用打包另行验收。
