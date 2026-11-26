# 定时任务交互 GUI 实测报告

- 日期：2026-11-25
- worktree：`/Users/xiaocy/orca/workspaces/sift/wt-scheduled-tasks`，分支 `fancy2110/wt-scheduled-tasks`
- 被测对象：从本 worktree 运行的 `pnpm tauri dev`（debug 构建，bundle id `com.sift.app`），Vite dev server `localhost:5173`，Rust 二进制 `target/debug/sift`
- 驱动：`orca computer`（后台优先；owner 授权后必要步骤用 `--restore-window`）
- 承接：item 8 评审 `docs/review/2026-11-25-scheduled-tasks-review.md`
- 重要环境事实：实测期间屏幕多次自动锁定，锁屏后 `orca computer` 返回 `screen_locked`，GUI 操作暂停；但后端 scheduler 线程与文件 I/O 不受影响（V4-a 的自动删除即发生在锁屏期间）。

## 0. 安全与隔离（最高优先级）

### 0.1 数据目录与标识符结论

- bundle identifier：`com.sift.app`（`src-tauri/tauri.conf.json:5`）。
- 数据目录：`StorePaths::discover()` 取平台 data dir 下固定 `Sift/`，macOS 即 `~/Library/Application Support/Sift/`（`crates/sift-store/src/paths.rs:22-25`，`APP_DIR="Sift"` `:16`）。
- **无环境变量 / CLI 覆盖数据目录的机制**：全仓 grep 无 `env::var` 数据目录开关；`--no-config` 仅在注释出现（`crates/sift-store/src/store.rs:217`），`src-tauri/src/main.rs` 不解析任何参数。
- 故 `tauri dev` 与已安装应用**共用同一数据目录，无隔离**。文档均为版本信封 `{"version":1,"items":...}`（`crates/sift-store/src/atomic.rs:27-31`）。
- 启动前发现：原运行进程是主检出 debug bundle 的壳，dev server 未运行致窗口白屏；已优雅退出，改从本 worktree 起 dev。

### 0.2 备份

- 备份：`/tmp/sift-gui-backup-20261125-211546`（worktree 外），完整复制 14 个文件（含 1.5GB scan-journal、tree.snapshot、AI token）。
- 哈希：`/tmp/sift-gui-hashes-before.txt`（源）与 `/tmp/sift-gui-hashes-backup.txt`（备份），`diff` 完全一致（14 行 SHA-256）。

### 0.3 测试前安全加固（应用停止时改 settings/cleanable）

- `cleanupMethod=delete`（不污染 owner 回收站）。
- owner 的 Downloads 规则 `enabled=false`。
- `scheduledCleanupEnabled=false`。
- cleanable 中 **94 个 `approvedForAuto=true` 全部撤销为 false**。

### 0.4 受控 fixture

- `$HOME/sift-gui-fixture-53534`（记录于 `/tmp/sift-gui-fixture-path.txt`），全在 home 内。
- 规则验证文件：`old-installer.dmg`(200KB, mtime 2026-10-01)、`second-old.dmg`(100KB, 同旧)、`fresh-installer.dmg`(50KB, 当天)、`notes.txt`(30KB)。
- 自动执行验证目录：`project/node_modules`（9.1MB，Safe 家族，旧 mtime）、`project/build`（9.1MB，Review 家族）。
- 所有 GUI 规则/例程只指向该 fixture，未指向任何真实用户目录。

## 实测矩阵结论汇总

| 项 | 结论 | 主要证据 |
|---|---|---|
| V1 合并列表 | **通过** | 截图 01；AX 计数 3=2 Routine+1 Rule |
| V2 规则手动执行 | **部分通过 + 受限** | 规则创建全程通过（截图 02-04）；菜单「立即执行」浮层点击不可达（见 §3 限制） |
| V3 禁用语义 | **通过（规则侧）** | 禁用后「立即执行」变灰（截图 06），与代码 `disabled={!r.enabled}` 一致 |
| V4-a 定时触发（auto 关） | **通过** | 143 项待确认横幅（截图 08）；规则自动删 2 个旧 dmg；history `automatic=true` |
| V4-b 定时触发（auto 开） | **通过** | node_modules 自动删除、build(Review) 存活、history auto=true 9502720 字节 |
| V5 自动 vs 手动同通路 | **自动侧通过；手动侧受限** | V4-a 自动侧 + item8 cargo 测试；浮层限制见 §3 |
| V6 失败校验 | **通过** | 缺目录（截图 09）、坏正则（截图 10）均被保存期校验拒绝 |

## 1. V1：计划任务列表合并展示 —— 通过

- 进入「智能建议 → 例行任务」tab，标题为「定时任务」，计数 `3`，与 `store.routines.length + store.cleanupRules.length` 一致（`src/views/SubPageView.svelte:358-359`）。
- AX 树（元素 168-188）与截图 `01-v1-routines-merged-list.png` 共同显示：
  - **Routine 行**（2 条，默认种子）：`Downloads`、`Screenshots`，字段=名称 +「确认」模式徽标 +「每周 · ≈0 B」+ 绝对路径 + 启用开关 +「更多」。
  - **Rule 行**（1 条，owner 原有，测试期为禁用态）：`每周清理下载目录安装包`，带「规则」徽标，字段=名称 + 目录 + 等宽正则 `^.*\.(dmg|pkg|mpkg)$` + 开关 +「更多」。
- 两类条目在同一卡片列表、同一新建对话框，但视觉徽标与字段不同，证实 review §2.4 差异点 9「前端表层合并、后端两套 DTO」。

## 2. V2：人工规则（CleanupRule）手动执行

### 2.1 创建规则 —— 通过

- 「添加定时任务」composer（`RoutineComposer.svelte`）手动 tab，截图 `02`（空表单）、`03`（填好）。
- 填入：名称 `GUI测试-DMG规则`、正则 `\.dmg$`、`olderThanDays=7`、目录 `$HOME/sift-gui-fixture-53534`，保存。
- 持久化证据（settings.json）：新增 `id=n-597c3b8ae78d498cf67e3ab886ba6c9c`，`enabled=true`，字段与输入完全一致；列表出现第 4 张卡片（截图 `04`，标题计数变 4）。

### 2.2 手动「立即运行」—— 受限（未完成 GUI 点击）

- 规则行「更多」展开的菜单含「立即执行 / 编辑 / 删除」（截图 `05`），与 `SubPageView.svelte:508-517` 一致。
- **受限事实**：该菜单是 Svelte 自绘浮层（非原生 AXMenu），菜单项在 AX 树被压平进行卡片按钮节点（无独立 menuitem 索引、无独立 frame）；在 `--restore-window` 下用多组坐标（经 PIL 像素校准）、键盘 Tab/Return 均无法激活「立即执行」，点击只关闭菜单或触发行编辑（已取消、未产生任何改动）。
- 对照：同一界面的**原生 `<select>`**（设置里的执行时间）有独立 `menu item` AX 节点（索引 743-766），可正常点击选择。说明限制来自 Svelte 自绘浮层的可访问性暴露，而非 orca 通用故障。
- 手动运行的后端正确性改由 item8 的 cargo 测试 `rule_auto_and_manual_triggers_share_one_runner_and_agree` 佐证（两触发共用 `run_rule`，结果一致）。

## 3. 环境限制（诚实记录，非产品 bug）

1. **Svelte 自绘浮层对合成点击不可达**：composer 与行「更多」菜单的菜单项无独立 AX 节点，`orca computer` 无法可靠激活；影响 V2.2 手动运行、V5 手动侧、V3 routine 菜单内操作。原生控件（开关、select、composer 内输入框/保存按钮）均正常。
2. **遮挡期 pager 合成层冻结**：窗口被 Orca 完全遮挡时，WKWebView 对 `transform: translateX` 平移的例行任务面板出现 AX 坐标报告为大负值、像素空白；前台重新挂载子页面后恢复正常。这是后台自动化的观测限制，真实用户窗口可见时不复现（已多次验证恢复后渲染正确）。
3. **debug 全卷扫描耗时**：`run_daily` 固定对系统卷完整扫描（`schedule.rs:177-188`），debug 构建下单次约 18-22 分钟、CPU 满载；期间 scan-journal 不更新（内存分析阶段），非卡死（CPU 时间持续增长）。
4. **屏幕锁定**：实测中多次自动锁屏，GUI 暂停但后端定时与文件 I/O 正常。

## 4. V3：禁用语义对照 —— 规则侧通过

- 规则开关（正常流控件，非浮层）点击可达：将 `GUI测试-DMG规则` 禁用，settings.json 实时 `enabled=false`。
- 禁用后打开「更多」菜单，截图 `06-v3-disabled-rule-menu.png`：**「立即执行」呈灰色禁用态**，「编辑/删除」仍正常。
- 与代码精确吻合：`SubPageView.svelte:508-517` 规则菜单项 `disabled={!rule.enabled}`。
- 结论：**UI 层手动运行受 enabled 门控**；这与 IPC 命令 `run_cleanup_rule` 自身不检查 enabled（`cleanup_rules.rs:358-375`）形成「前端门控、后端不门控」的分层差异——经 UI 无法手动运行禁用规则，但直接调 IPC 可以。Routine 侧菜单项同理（`SubPageView.svelte:407-416`），但因浮层限制未能在 GUI 复点，仅以代码与规则侧实测推断。

## 5. V4：定时自动触发端到端

### 5.1 V4-a：auto_execute=关（只分析、不删 cleanable）—— 通过

配置（GUI 设置弹窗，截图 `07`）：每日定时开、执行时间 22:00、`scheduledAutoExecute=false`。因当日序日 329 已有盖章，退出应用清除 `scheduledLastRun=null` 后重启（测试夹具准备，GUI 无清除入口）。

- **触发证据**：22:20:57 重启，22:21:32 前 `scheduledLastRun` 被盖为 `2026-329`，证明 `decide()` 在当前小时槽返回 `RunToday` 且 scheduler 先盖章（`schedule.rs:127-133`）。
- **只分析不删**：run_daily 完成后前端出现横幅「**定时清理发现 143 项待确认**」+「稍后」按钮（截图 `08`），即 `scheduled://ready` 事件（`schedule.rs:236-242`）；cleanable 条目无一被自动删除。
- **规则仍执行（独立于 auto 开关）**：22:42:50，fixture 中 `old-installer.dmg` 与 `second-old.dmg`（均 7 天前 mtime）被删除，`fresh-installer.dmg`（当天）与 `notes.txt`（不匹配正则）保留——精确符合「正则 `\.dmg$` + olderThanDays 7」。
- history.json 新增会话：`automatic=true`、`titles=[old-installer.dmg, second-old.dmg]`、`bytes=300000`（两文件大小之和），与 `run_rule` 的 `note_cleanup_session(automatic=true)`（`cleanup_rules.rs:286-289`）一致。
- 上述删除发生在屏幕锁定期间，佐证「应用保持运行即可定时，无需窗口在前台」。

### 5.2 V4-b：auto_execute=开（无人值守删 Safe+已批准+home 内）—— 通过

前置：周期 1（auto 关）的全卷分析已把 fixture `project/node_modules`（9.1MB > 8MB 提名阈值）写入 cleanable，`safety=safe`、`kind=rebuildableCache`。因屏幕锁定无法用 GUI 勾选批准，**改用等价持久化状态**：应用停止时将该条目 `approvedForAuto=true`（仅此 1 条 fixture 数据），并设 `scheduledAutoExecute=true`、清 `lastRun` 后重启。触发、扫描、护栏、删除仍全部是真实 scheduler 代码路径。

- **触发**：23:21:17 重启，23:22:15 盖章 `2026-329`；run_daily 全卷分析（debug 约 23 分钟，CPU 满载，与 V4-a 同模式）。
- **自动删除（23:44:15）**：分析完成后 `select_approved_paths` 选中该 Safe+已批准+home 内条目，`perform_cleanup(automatic=true)` 删除整个 `project/node_modules`。磁盘事实：该目录消失；`cleanable.json` 中对应条目被移除（fixture cleanable 条目归零）。
- **Review 项存活（护栏正确）**：同目录的 `project/build`（Review 家族、未进 cleanable）完整保留，证明无人值守只动 Safe+已批准对象。
- **history 会话**：新增 `23:44:15 automatic=true, items=1, titles=['node_modules'], bytes=9502720`，字节数与被删目录大小精确一致。
- 重新分析后 `approvedForAuto=true` 仍保留（cleanable 刷新不丢批准，`cleanable.rs` 语义），是本路径能选中目标的前提。
- 锁屏限制：本项无像素截图，证据全部来自磁盘事实、cleanable/history/settings JSON 与进程 CPU 时间线；自动删除发生在锁屏期间，再次佐证后端定时不依赖前台窗口。

## 6. V5：自动 vs 手动同通路

- **自动侧（已实测）**：V4-a 中 scheduler 经 `run_enabled_cleanup_rules → run_rule` 自动删除，history 标 `automatic=true`。
- **手动侧**：受 §3 浮层限制无法 GUI 点击，但 item8 cargo 测试 `rule_auto_and_manual_triggers_share_one_runner_and_agree` 已在真实代码路径证明两侧共用 `cleanup_rules::run_rule`，删除集合、计数、history 完全一致。
- 真实 app 的后端共用结论成立；GUI 手动点击的端到端复现受工具可访问性限制。

## 7. V6：失败场景校验 —— 通过

均在 composer 填规则后点「保存定时任务」，观察保存期校验（`cleanup_rules.rs:95-109` 的 GUI 对应）：

- **V6a 缺失目录**（截图 `09`）：目录填 `$HOME/no-such-dir-xyz123`，保存后底部 toast「**目录不存在或无法访问**」（`err.ruleDirMissing`），对话框保留、规则未持久化（cleanupRules 仍 2 条）。
- **V6b 非法正则**（截图 `10`）：有效目录 + 正则 `(`，toast「**正则表达式无效：regex parse error ... unclosed group**」（`err.ruleRegex|`），规则未持久化。

## 8. 恢复与清理（已逐项执行并验证）

1. **停止测试应用**：`pkill target/debug/sift`、tauri dev、vite、caffeinate，确认无残留 sift 进程。
2. **还原数据目录**：将测试后数据目录整体重命名为 `Sift.posttest-<pid>` 留作回退，从备份 `/tmp/sift-gui-backup-20261125-211546` 完整复制回 `~/Library/Application Support/Sift/`（14 个文件）。
3. **哈希校验**：对还原后目录重新计算 SHA-256，与测试前清单 `/tmp/sift-gui-hashes-before.txt` `diff` —— **完全一致（HASH MATCH）**。
4. **关键状态逐项核对（均回到测试前）**：
   - `scheduledCleanupEnabled=true, scheduledHour=3, scheduledAutoExecute=false, scheduledLastRun=2026-329`（owner 原始值）。
   - `cleanupMethod=trash`（测试期临时改的 delete 已还原）。
   - owner 规则 `每周清理下载目录安装包 enabled=true`（测试期临时禁用已还原）；测试创建的 `GUI测试-DMG规则` 随备份还原消失。
   - cleanable 255 条、其中 **94 个 `approvedForAuto=true` 全部恢复**（测试期撤销已还原）。
   - routines 恢复为 `default:downloads`、`default:screenshots` 且均 enabled。
5. **fixture 清理**：`rm -rf $HOME/sift-gui-fixture-53534`，确认目录不存在。
6. **回退目录清理**：哈希验证通过后删除 `Sift.posttest-<pid>`。
7. **未触碰项**：`~/Library/Application Support/com.sift.app`（owner 10 月既有目录）保持原样；AI token 文件随备份原样还原。
8. **应用进程说明**：测试开始时 owner 原有的主检出 debug 壳进程（当时 dev server 未运行、窗口白屏）被优雅退出；测试用 worktree dev 应用已停止。未擅自重启任何应用，是否恢复运行由 owner 决定。

### 截图清单（`docs/review/screenshots/2026-11-25-gui/`）

| 文件 | 内容 |
|---|---|
| 00-baseline-dashboard.png | 仪表盘基线 |
| 01-v1-routines-merged-list.png | V1 合并列表（2 Routine + 1 Rule，计数 3/4） |
| 02-v2-composer-empty.png | V2 新建任务空 composer |
| 03-v2-composer-filled.png | V2 规则表单填好 |
| 04-v2-rule-saved-in-list.png | V2 规则保存后入列表 |
| 05-v2-more-menu-open.png | 行「更多」菜单（立即执行/编辑/删除） |
| 06-v3-disabled-rule-menu.png | V3 禁用后「立即执行」变灰 |
| 07-v4a-settings-scheduled-on.png | V4-a 定时设置（22:00、auto 关） |
| 08-v4a-scheduled-ready-banner.png | V4-a「定时清理发现 143 项待确认」横幅 |
| 09-v6a-missing-dir-error.png | V6a 缺目录校验错误 toast |
| 10-v6b-bad-regex-error.png | V6b 非法正则校验错误 toast |

V4-b 因锁屏无像素截图，以磁盘 + JSON 证据替代（见 §5.2）。

## 9. 遗留疑问

1. Svelte 自绘浮层（行更多菜单、composer）缺少独立 AX 节点，不利于自动化与无障碍，建议为菜单项补 `role="menuitem"` 的可访问性暴露。
2. IPC `run_cleanup_rule` 不检查 `enabled` 而 UI 门控，是否有意为之需产品确认。
3. `run_daily` 全卷扫描在 debug 下近 20 分钟，release 性能未在本机实测；可考虑聚焦扫描或增量。
