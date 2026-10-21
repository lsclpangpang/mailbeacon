# TDD：写信弹窗加大

## 背景

用户反馈「新邮件」写信弹窗偏小，正文可写区域不够用。当前实现：

- 通用 `.sheet` 宽度：`min(680px, calc(100vw - 48px))`（`ui/style.css`）
- 写信窗高度以内联样式写死：`height:min(640px, calc(100vh - 80px))`（`ui/app.js` → `openCompose`）

## 目标（Acceptance）

1. **仅写信弹窗**（`.compose-sheet`）明显变大；文件夹/规则等其他 `.sheet` 尺寸不变。
2. 写信窗默认尺寸（大屏、视口充足时）：
   - **宽度**：至少 **960px**（仍用 `min(..., calc(100vw - 48px))`，小屏不溢出）
   - **高度**：至少 **820px**（仍用 `min(..., calc(100vh - 80px))`，小屏不溢出）
3. 尺寸定义放在 **CSS**（`.compose-sheet`），`openCompose` **不再**内联写死 `640px` 高度。
4. 布局行为保持：弹窗仍居中、可滚动、页眉/收件人/工具栏/页脚不挤掉；正文区随弹窗变高而变高（`flex` 行为不变）。
5. 自动化测试锁定上述尺寸常量，防止回退。

## 非目标

- 不做可拖拽改大小、全屏写信、记住用户自定义尺寸（可后续迭代）。
- 不改动其他设置类 sheet 的宽度。

## 测试计划（先失败后实现）

文件：`tests/compose-size.test.js`

| # | 用例 | 期望 |
|---|---|---|
| T1 | 解析 `ui/style.css` 中 `.compose-sheet` | 含 `width: min(960px, calc(100vw - 48px))`（或等价写法） |
| T2 | 解析 `ui/style.css` 中 `.compose-sheet` | 含高度 `min(820px, calc(100vh - 80px))`（或 `height`/`max-height` 组合达到同等约束） |
| T3 | 解析通用 `.sheet` 规则 | 仍为 `680px` 宽，未被改成 960 |
| T4 | 读取 `ui/app.js` 中 `openCompose` 模板 | 不含 `640px` 高度内联样式 |

实现前：T1–T4 应失败（或 T3 通过、T1/T2/T4 失败）。  
实现后：全部通过。

## 实现要点（供实现阶段，本阶段不写代码）

- 在 `ui/style.css` 为 `.compose-sheet` 覆盖 `width` / `height` / `max-height`。
- 去掉 `openCompose` 里 `style="height:min(640px, …)"`。
- 回归：跑现有 `tests/compose-*.test.js` + 新 `compose-size.test.js`。

## 分支与发布

- 分支：`feat/larger-compose-sheet`（**不**直接推 `main`）
- 流程：TDD → Claude Code review → 实现 → Claude Code review → PR
