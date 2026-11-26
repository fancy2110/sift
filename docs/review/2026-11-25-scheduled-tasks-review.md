# 定时任务机制 Review、五个候选任务与统一触发测试

- 日期：2026-11-25
- 基线：`origin/main` @ `b23cede`（v0.6.0）
- worktree：`/Users/xiaocy/orca/workspaces/sift/wt-scheduled-tasks`，分支 `fancy2110/wt-scheduled-tasks`
- 范围：仓库内全部定时 / 周期 / 调度相关代码；结论均给出 `文件:行号` 依据。本轮只新增测试与文档，不改生产行为。

---

## 0. 核心结论（TL;DR）

1. 仓库**已有**一套真正运行的定时机制：每日定时清理线程（`src-tauri/src/schedule.rs`），按本地小时槽触发，支持漏跑补跑窗口、先盖章后执行、只分析不删除的保守默认。
2. 「例行任务」（Routine，含习惯挖掘 / AI 草稿 / 手动保存 / 默认种子四个来源）与「清理规则」（CleanupRule）是**两套数据模型、两个持久化文件、两个执行函数**；前端把它们合并成一个「计划任务」列表展示（`src/views/SubPageView.svelte:358-359`），后端并未统一。
3. **Routine 的 `cadence`（daily/weekly/monthly）与 `mode`（Auto/Approve）当前没有任何调度器消费**：Routine 只能手动「立即执行」。这与既有评审 `docs/review/post-merge-review.md:110-111` 的结论一致，也意味着需求 `REQUIREMENTS.md:37`（R6.3「cron 式周期 + 自动模式」）尚未完整落地。
4. CleanupRule 是目前唯一同时具备自动与手动触发、且两者**已共享同一执行入口**（`cleanup_rules::run_rule`）的任务形态。
5. 本轮新增 5 个测试（`src-tauri/src/schedule.rs` 测试模块）：Layer 1 钉住规则任务的共享通路与失败语义；Layer 2 用测试内薄 harness + 生产组件（真实 `decide` 时钟决策 + 真实 `perform_cleanup` 护栏漏斗）验证 Routine 形态的目标统一模式。全部通过，全工作区回归零失败。

---

## 1. 现状 review：定时 / 调度相关代码地图

### 1.1 三条后台机制

| 机制 | 触发性质 | 入口 | 执行函数 | 持久化状态 |
|---|---|---|---|---|
| 每日定时清理 | **时间触发**（每 20s 唤醒，小时槽） | `schedule::start`，setup 时启动 `src-tauri/src/lib.rs:193` | `run_daily` `src-tauri/src/schedule.rs:167` | `settings.json` 的 `scheduled_*` 字段 |
| 低空间监控 | **阈值事件触发**（默认 300s 采样 + 手动唤醒） | `start_monitor` `src-tauri/src/analyze.rs:757` | `sift_monitor::monitor::clean_now` `crates/sift-monitor/src/monitor.rs:458` | `settings.json` 的 monitor 阈值字段 |
| 文件系统监听 | **事件触发**（删除事件） | `watch_fs` `src-tauri/src/watcher.rs:32` | 不执行清理，只发 `fs://deleted` 失效事件 | 无 |

另有托盘菜单的手动入口 `clean_safe_items`（`src-tauri/src/tray.rs:507`），不属于定时机制，但复用同一清理漏斗。

### 1.2 每日定时清理链路（唯一真正的 scheduler）

- 线程：`std::thread` 命名 `sift-schedule`，应用生命周期内常驻，20 秒唤醒一次（`src-tauri/src/schedule.rs:21-27,95`）。
- 纯时钟决策：`decide(now_ordinal, now_hour, scheduled_hour, catchup_hours, last_run)` 返回 `Wait / RunToday / RunMissed / SkipMissed`（`schedule.rs:53-84`），无时钟、无 App 依赖，可单测。
- 开关与时间槽来自 `MonitorSettings.scheduled_cleanup_enabled / scheduled_hour / scheduled_catchup_hours / scheduled_last_run`（`crates/sift-store/src/settings.rs:111-152`）。该开关独立于低空间 `auto_mode`（`schedule.rs:99-108` 注释明确说明）。
- 先持久化「当日已盖章」再执行，崩溃不会引发第二次无人值守清理（`schedule.rs:127-133`）。
- `run_daily` 的动作序列（`schedule.rs:167-244`）：
  1. 找系统卷 → 复用 `ScanManager` 扫描（`schedule.rs:171-188`，1 小时超时）；
  2. `analyze_current_with` 分析并落库（`schedule.rs:190`）；
  3. 无论自动开关如何，先跑所有 `enabled` 的 CleanupRule（`schedule.rs:218-219` → `run_enabled_cleanup_rules` `schedule.rs:146-164`）；
  4. `scheduled_auto_execute = true`：选 `Safe + approved_for_auto + home 内（realpath 证明）` 的条目，走 `analyze::perform_cleanup(..., automatic=true)`（`schedule.rs:221-229`，选择器 `select_approved_paths` `schedule.rs:265-278`）；
  5. 否则弹窗前移并发 `scheduled://ready` 事件，只分析不删除（`schedule.rs:232-242`）。
- 命令面：`set_scheduled_cleanup(enabled, hour, auto_execute)`（`analyze.rs:739-755`），前端封装 `src/lib/ipc.ts:431-436`。

### 1.3 低空间监控链路（阈值触发，非定时）

- 后台线程 `sift-monitor`，`Monitor::start`（`crates/sift-monitor/src/monitor.rs:221-279`），命令入口 `start_monitor`（`src-tauri/src/analyze.rs:758`）；采样 → 纯函数 `policy::evaluate` → `act`（`monitor.rs:282-436`）。
- 唯一无人值守删除出口是 `clean_now`（`monitor.rs:458-518`），删除前逐条重做 home 边界与指纹复核；`Remover` 是注入缝（`monitor.rs:70-82`），生产用系统回收站，测试可替身。
- 手动「立即采样」走同一线程的 wake 通道（`MonitorControl::sample_now` `monitor.rs:151-153`），即手动与自动在 monitor 内也是同一通路。
- 该链路只消费 cleanable 列表，**不认识 Routine 也不认识 CleanupRule**。

---

## 2. 两条任务链路：自动整理产生 vs 人工添加

### 2.1 链路 A：自动整理产生的任务

| 来源 | 产生方式 | 落库形态 | 初始状态 |
|---|---|---|---|
| 习惯挖掘 | 决策日志 `DecisionLog::suggest_routines(3,3)`（`crates/sift-analyze/src/habits.rs:212-253`，要求 ≥3 次显式决策且跨 ≥3 个本地日）经命令 `routine_suggestions` 写入（`analyze.rs:1164-1197`） | `RoutineEntry`（kind-based，`paths=[]`） | **`enabled=false`**，`Approve` 模式（`analyze.rs:1180-1189`） |
| 默认种子 | `seed_default_routines`，每次启动调用（`lib.rs:86`；实现 `analyze.rs:1509-1548`）：Downloads 与 Desktop 截图两个路径型例程 | `RoutineEntry`（`customPath`） | `enabled=true`，`Approve`，每安装只种一次（`defaults_seeded`） |
| AI 草稿 | `ai_routine` 只产出配置 JSON，模型不能执行任何动作（`src-tauri/src/ai_routine.rs:1-9,199-208`）；用户确认后 `save_routine_draft` 才落库（`ai_routine.rs:280-320`） | `RoutineEntry`（`customPath`，id 前缀 `ai:`） | `enabled=true`，强制 `Approve`（`ai_routine.rs:315`） |

关键事实：AI 与挖掘产物都必须经过用户确认或启用开关，没有任何来源能自行开始删除（`routines.rs:44-47` 对 `enabled` 语义有明确注释）。

### 2.2 链路 B：人工添加的任务

| 来源 | 命令 | 落库形态 |
|---|---|---|
| Smart 列表 / Explorer 保存例程 | `save_routine(title, kind, average_bytes, path?)`（`analyze.rs:1831-1885`） | `RoutineEntry`，id 前缀 `custom:` / `suggestion_key`，固定周度标签、`Approve` 模式 |
| 确认 AI 草稿 | `save_routine_draft`（`ai_routine.rs:280`） | 同上 |
| 目录 + 正则规则 | `save_cleanup_rule`（`src-tauri/src/cleanup_rules.rs:310-334`），保存前 `validate_rule`（`cleanup_rules.rs:95-109`）+ 保护目录校验 | `CleanupRule`，嵌入 `settings.json` |

前端手动执行入口：
- Routine：`runRoutine` IPC（`src/lib/ipc.ts:496-497`）→ store 方法 `runSavedRoutine`（`src/lib/store.svelte.ts:1907`）→ 后端 `run_routine`（`analyze.rs:1939-1973`）。
- Rule：`runCleanupRule` IPC（`ipc.ts:402-403`）→ `run_cleanup_rule` 命令（`cleanup_rules.rs:358-375`）。
- UI 按钮分别在 `src/views/SubPageView.svelte:415` 与 `:516`。

### 2.3 两种任务的数据模型与持久化

| 维度 | Routine（`RoutineEntry`） | CleanupRule |
|---|---|---|
| 定义 | `crates/sift-store/src/routines.rs:30-48` | `crates/sift-store/src/settings.rs:39-57` |
| 持久化 | 独立文档 `routines.json`（`crates/sift-store/src/paths.rs:67-69`；加载 `store.rs:152-162`；写盘 `store.rs:652`） | `settings.json` 内 `MonitorSettings.cleanup_rules`（`settings.rs:122-123`），上限 50 条（`settings.rs:222-226`） |
| 容量上限 | 100（`routines.rs:14`，`DEFAULT_ROUTINE_CAP`） | 50 |
| 调度表达 | `cadence` 翻译键 daily/weekly/monthly（`routines.rs:36`），**仅展示** | 无自己的周期，搭每日调度器的便车 |
| 模式 | `RoutineMode::{Auto,Approve}`（`routines.rs:19-25`），**执行时不被读取** | 仅 `enabled` 布尔 |
| 目标解析 | kind-based：运行时从 cleanable 列表按 family + Safe + 已批准选路径（`select_routine_paths` `analyze.rs:1977-1990`）；或显式 `paths`（存在性过滤 `analyze.rs:1953-1961`） | 运行时扫盘：目录 + 文件名正则 + 年龄，带 2000 文件 / 20GB / 深度 32 硬上限（`cleanup_rules.rs:22-24,117-198`） |
| 自动触发 | **无** | 每日调度器 `run_enabled_cleanup_rules`（`schedule.rs:146-164`） |
| 手动触发 | `run_routine`（`analyze.rs:1939`） | `run_cleanup_rule`（`cleanup_rules.rs:358`） |
| 执行函数 | `cleanup_off_thread(automatic=true)` → `perform_cleanup_with`（`analyze.rs:1972` → `:473-499` → `:536`） | `run_rule`（`cleanup_rules.rs:227-299`），内部自己调回收站 / delete |
| 禁用语义 | 禁用项拒绝运行（`analyze.rs:1949-1951`） | 自动链路过滤 `enabled`（`schedule.rs:152`）；**手动命令不检查 enabled**（`cleanup_rules.rs:358-375` 无判断） |
| 历史记录 | `note_cleanup_session(automatic=true)`（`analyze.rs:692`） | 同样 `automatic=true`（`cleanup_rules.rs:286-289`） |

### 2.4 两者是否已是同一套模式？——不是。差异清单

1. **数据模型不同、文件不同**：`routines.json` vs `settings.json` 内嵌数组；容量、schema、迁移独立。
2. **调度语义不同**：Routine 带 cadence 标签但无消费方；Rule 无周期字段却能每日自动跑。
3. **自动通路只覆盖 Rule**：调度器 `run_daily` 完全不枚举 `store.routines()`（全文件 grep 可证：`schedule.rs` 无 routine 引用）。
4. **执行漏斗不同**：Routine/交互/托盘/调度器的 cleanable 条目统一走 `perform_cleanup_with`（成员校验 → Keep 拒绝 → 指纹复核 → trash/delete → 记决策/历史，`analyze.rs:536-697`，trash/delete 分支在 `analyze.rs:630-648`）；Rule 走独立的 `run_rule`（自扫描、自保护校验、自调 trash、自带批量上限）。
5. **第三个删除出口**：低空间 monitor 的 `clean_now`（`monitor.rs:458`）与前两者又不共享函数，仅共享「home + 指纹 + 已批准」的策略思想（`crates/sift-monitor/src/policy.rs`）。
6. **`RoutineMode::Auto` 是空操作**：`run_routine` 不读 `mode`（`analyze.rs:1939-1973`），切换按钮只改持久化值（`toggle_routine_mode` `analyze.rs:1911-1916`）。
7. **enabled 语义不一致**：Routine 禁用即手动也拒跑；Rule 手动「立即运行」无视 enabled（差异点已在测试中固化，见 §4.2）。
8. **错误上抛方式不同**：Rule 手动命令返回 `Result<RuleRunDto,String>`；自动包装器 `run_enabled_cleanup_rules` 吞错只 `eprintln!` 并继续下一条（`schedule.rs:156-161`）。
9. 前端已做表层合并：同一列表、同一计数、同一新建对话框（`SubPageView.svelte:358-360`，CHANGELOG v0.1 条目「share one Scheduled tasks list」`CHANGELOG.md:192`），但后端契约仍是两套 DTO（`RoutineDto` `analyze.rs:1743-1756` vs `CleanupRule` 直接序列化）。

### 2.5 统一模式方案（本轮只设计，不改生产代码）

目标：同一数据模型、同一执行入口、同一触发通路；自动与手动仅「来源 / 触发参数」不同。

**方案要点（建议分阶段）**

1. 统一任务模型：以 `RoutineEntry` 为基座吸收 `CleanupRule`，新增
   - `trigger: { DailyAt(hour) | Weekly | Monthly | LowSpace }`（把 cadence 从标签升级为可计算调度）；
   - `selector: { Kind(token) | Paths(Vec) | DirRule{dir,pattern,older_than_days} }`；
   - 保留 `enabled`、`mode(Approve/Auto)`、`last_run` 戳；`source` 记录 mined/ai/default/manual 仅作展示与审计。
   - 落盘统一到 `routines.json`（`StorePaths::routines` 已存在），settings 里的 `cleanup_rules` 做一次带版本号的迁移。
2. 统一触发通路：扩展现有 `schedule.rs` 线程（不要新开线程），把 `decide()` 从「每日小时槽」泛化为「按任务 trigger 计算到期」，每个到期任务向一个任务队列投递 `task_id`；手动命令与 monitor 阈值也投递同一个队列。唤醒机制可复用 monitor 的 `mpsc` wake 模式（`monitor.rs:355-368`）。
3. 统一执行入口：新增 `run_task(task, trigger)`，内部按 selector 解析路径后**统一调用 `perform_cleanup_with`**；把 `run_rule` 的扫盘/上限/保护校验降级为 `DirRule` selector 的解析器，产出同样的路径清单后并入同一漏斗。`Remover` 缝（`monitor.rs:70-82`）下沉为漏斗的删除后端，让 `clean_now` 也并入，消除第三个删除出口。
4. 模式语义落地：`Approve` 走现有 `scheduled://ready` 事件（`schedule.rs:236`）；`Auto` 仅对 `Safe + approved_for_auto + home 内 + 指纹匹配` 生效——这些护栏 `perform_cleanup_with` 已全部具备，无需新写安全逻辑。
5. 涉及文件（仅列举，本轮不动）：
   - `crates/sift-store/src/routines.rs`、`settings.rs`（模型与迁移）、`store.rs`（CRUD/迁移加载）；
   - `src-tauri/src/schedule.rs`（泛化调度、任务队列）、`analyze.rs`（`run_routine`/`run_cleanup_rule` 改为投递统一入口）、`cleanup_rules.rs`（降级为 selector 解析器）；
   - `crates/sift-monitor/src/monitor.rs`（阈值事件改投任务队列）；
   - 前端 `src/lib/ipc.ts`、`store.svelte.ts`、`SubPageView.svelte` 合并 DTO（UI 层已合并，改动较小）。
6. 安全红线（沿用既有设计，见 `crates/sift-monitor/src/lib.rs:19-27`）：统一不得削弱 home 边界、逐条批准、指纹复核、回收站优先、批量上限与冷却。

---

## 3. 五个候选定时任务（仅依据仓库已分析数据）

每个候选注明仓库内数据证据；「同通路」均指 §2.5 目标模式下自动与手动收敛到同一 `run_task → perform_cleanup` 漏斗，其中候选 2 在现有生产代码中已经是共享通路。

### 候选 1：可重建依赖缓存周度例程（rebuildableCache 家族）

- 动作：对 `node_modules / DerivedData / __pycache__ / .gradle / .venv` 等可重建目录，按周扫描 → 仅清理 `Safe + 已批准 + home 内` 条目，默认 Approve。
- 建议周期：每周。
- 数据证据：
  - 规则表把这些目录判为结构性 `Safe`：`crates/sift-analyze/src/rules.rs:106-172`（`dir.node_modules`、`dir.xcode_derived_data`、`dir.python_pycache`、`dir.gradle`、`dir.python_venv` 等）。
  - 习惯挖掘对 ≥8 个不同决策日的习惯建议周度：`crates/sift-analyze/src/habits.rs:425-431`。
  - 端到端测试反复证明 node_modules 四次清理即生成周度建议：`crates/sift-monitor/tests/cleanup_loop.rs:313-358`。
  - 前端 mock 已有同款建议数据（node_modules，weekly，6 次 / 5 天）：`src/lib/mock/backend.ts:354-361`。
- 同通路：kind-based Routine（`select_routine_paths` `analyze.rs:1977`）；手动入口是现有 `run_routine`，自动入口待 §2.5 调度器接线，两者共用 `perform_cleanup_with`。

### 候选 2：Downloads 安装包老化清理规则（dmg/pkg/iso 等，7 天）

- 动作：对下载目录内匹配安装包后缀且超 N 天未修改的文件移入回收站；带 2000 文件 / 20GB 上限。
- 建议周期：每周（规则本身随每日调度器评估，是否真正删除由年龄条件决定）。
- 数据证据：
  - 安装包后缀家族定义（Review 级）：`crates/sift-analyze/src/rules.rs:201-202`。
  - 文件 pass 默认枚举 Downloads 等目录：`crates/sift-analyze/src/analyzer.rs:122`。
  - 前端 mock 已内置一条同形规则「每周清理下载目录安装包」，正则 `^.*\.(dmg|pkg|mpkg)$`、`olderThanDays: 7`、enabled：`src/lib/mock/backend.ts:329-337`。
  - 集成测试 fixture 含 `Downloads/Installer.dmg`：`crates/sift-monitor/tests/cleanup_loop.rs:91-95`。
- 同通路：**生产已实现**——自动经 `run_enabled_cleanup_rules`（`schedule.rs:146`）、手动经 `run_cleanup_rule`（`cleanup_rules.rs:358`），二者都调用 `run_rule`（`cleanup_rules.rs:227`）；本轮测试 §4.2 已钉住。

### 候选 3：日志与崩溃报告清理（月）

- 动作：针对应用日志目录与诊断报告目录，按月清理 `*.log / *.crash / *.ips / *.dmp / *.diag`；结构性 Safe，可作为 Auto 模式候选但默认仍走 Approve。
- 建议周期：每月。
- 数据证据：
  - 目录规则 `dir.app_logs`（Logs）与 `dir.crash_reports`（DiagnosticReports/CrashReporter）均为 `Safe`：`crates/sift-analyze/src/rules.rs:179-190`。
  - 日志后缀表：`crates/sift-analyze/src/rules.rs:210`。
  - mock 历史数据里已有一次自动清理 `system.log` / `crash-report.crash` 的会话记录：`src/lib/mock/backend.ts:340-343`。
  - AI 例程系统提示把 logs 列为优先目标：`src-tauri/src/ai_routine.rs:69`。
- 同通路：可实现为 DirRule selector（走 `run_rule` 共享通路）或路径型 Routine（走 `perform_cleanup_with`）；统一模式后两者同漏斗。

### 候选 4：应用缓存目录月度例程（~/Library/Caches）

- 动作：路径型例程，每月对 `~/Library/Caches` 做一次扫描分析，Approve 模式下产出待确认清单。
- 建议周期：每月（缓存回涨快，月度与体积阈值互补；阈值侧已有 monitor 兜底）。
- 数据证据：
  - `dir.app_cache` 对 `Caches` 目录给结构性 `Safe`：`crates/sift-analyze/src/rules.rs:173-178`。
  - AI 提示词以 `~/Library/...` 缓存为首选目标并给了 JSON 范例：`src-tauri/src/ai_routine.rs:69,77`；路径校验强制 home 内、排除 home 根本身：`ai_routine.rs:123-153`。
  - 路径型 Routine 的保存与 id 规则已具备：`analyze.rs:1838-1852`、`ai_routine.rs:304-317`。
- 同通路：路径型 Routine，手动 `run_routine` 已通；自动侧待调度器按 cadence 投递同一任务 id。

### 候选 5：久未动用大文件月度巡检（StaleLargeFile，只提醒不自动删）

- 动作：每月跑一次全量分析，把超 256MB 且 180 天未动的大文件推入 Smart 列表并通过 `scheduled://ready` 提醒用户确认；**不做无人值守删除**。
- 建议周期：每月。
- 数据证据：
  - 家族定义 `CandidateKind::StaleLargeFile`：`crates/sift-analyze/src/candidate.rs:33-34`，属于必须模型/人工判定的内容依赖型家族（`candidate.rs:88-96`），不允许结构性 Safe。
  - 默认阈值：`RuleThresholds`（256MB / 180 天）`crates/sift-analyze/src/rules.rs:31-44`；设置项 `analysis.stale_days = 180`：`crates/sift-store/src/settings.rs:278`。
  - 文件 pass 搜索目录：`crates/sift-analyze/src/analyzer.rs:122`（Downloads/Desktop/Documents/Movies）。
  - 「只分析、弹窗待确认」通路生产已存在：`schedule.rs:232-242` 的 `scheduled://ready`，前端接收 `src/lib/ipc.ts:655-658`、处理 `src/lib/store.svelte.ts:1275-1287`。
- 同通路：定时与手动「立即分析」都经 `analyze_current_with` 落同一 cleanable 存储，用户确认后统一走 `clean_paths → perform_cleanup_with`。

> 证据缺口的诚实说明：仓库没有任何真实用户的扫描结果数据（测试 fixture 均为运行时临时构造，无落库样本），因此以上候选的「体积收益」无法用仓库数据量化，只能依据规则定义、测试 fixture、mock 数据与需求文档给方向。曾考虑「自动清空回收站」：`CleanupMethod::EmptyTrash` 计划与 token 已定义（`crates/sift-analyze/src/cleanup.rs:125-130`、`crates/sift-store/src/cleanable.rs:56`），但 src-tauri 侧没有按该 method 分支的执行器（`perform_cleanup_with` 只走 trash/delete，`analyze.rs:629-648`），当前交付不了，列为遗留项而非候选。

---

## 4. Mock 任务与触发测试

### 4.1 布局与复用的现有抽象

- 全部新增测试在 `src-tauri/src/schedule.rs` 的既有 `#[cfg(test)] mod tests` 内（`:503` 起的统一触发测试区），不新建测试模块、不动前端、不碰生产数据；临时 fixture 全部在 `std::env::temp_dir()` 下并带 Drop 清理。
- 唯一生产代码增量是一个测试专用构造器 `AppServices::with_store_for_test`（`src-tauri/src/analyze.rs:103-115`，`#[cfg(test)]`，`pub(crate)`），让测试能用内存 Store 组装服务而不打开平台数据目录；非测试构建零影响。
- 复用的现有抽象：
  - 时钟决策 `decide()` / `SchedAction`（`schedule.rs:53-84`）——不重写时钟；
  - 规则任务的生产共享入口 `cleanup_rules::run_rule` 与调度器包装 `run_enabled_cleanup_rules`；
  - Routine 形态的生产护栏漏斗 `analyze::perform_cleanup`（`analyze.rs:505`，其内部即交互/例程/托盘/调度共用的 `perform_cleanup_with` `analyze.rs:536`）；
  - 真实扫描分析管线 `ScanEngine + Analyzer + RuleAdjudicator + Store::record_analysis + approve_all_structural`，与 `crates/sift-monitor/tests/cleanup_loop.rs` 同一套；
  - 内存 Store `Store::in_memory()`，删除方式设为 `delete` 以避免依赖 Finder/回收站权限，护栏代码路径不变。

### 4.2 测试清单（5 个新增）

Layer 1 —— 规则任务，生产已共享同一 runner：

1. `rule_auto_and_manual_triggers_share_one_runner_and_agree`（`schedule.rs:592`）：同一 fixture 形状下，自动侧调用调度器同款 `run_enabled_cleanup_rules`，手动侧调用命令同款 `run_rule`；断言匹配/删除计数、文件存活、历史会话（`automatic=true`、items/bytes 全等）；并钉住禁用规则对自动链路不可见（`(0,0)`）。
2. `rule_failures_reach_both_triggers_through_the_same_guardrails`（`schedule.rs:652`）：保护目录（`err.protectedPath`，自动侧 preview 计数 1 但 removed=0 的包装差异也被断言）、目录缺失（`err.ruleDirMissing`）、非法正则（`err.ruleRegex|`）、Unix 非 root 下只读目录导致逐项失败（failed=1、文件存活，两个触发入口一致）。

Layer 2 —— Routine 形态 mock 任务，测试内薄统一 harness（生产尚缺自动调度，见 §4.4）：

- harness 组件：`TaskTrigger{AutoScheduled,Manual}`（`:734`）、`MockTask`（`:744`）、`task_due`（包真实 `decide`，`:766`）、统一执行函数 `run_mock_task`（`:786`，两个触发变体落到同一个 `perform_cleanup` 调用，trigger 不产生分支差异）、`AnalyzedFixture`（`:827`，真实 scan→analyze→approve 产出的 Safe 缓存 + Review 构建产物）。
3. `mock_task_clock_drives_an_automatic_fire_that_matches_the_manual_fire`（`:919`）：模拟小时唤醒序列（槽前 Wait、槽点 RunToday、盖章后同槽不再触发）；自动 fire 与槽外手动 fire 的逐项结果签名、文件效果、cleanable 清空、历史会话完全一致，Review 项两边都存活。
4. `mock_task_honors_catchup_and_never_runs_an_expired_missed_slot`（`:982`）：12h 补跑窗内 RunMissed 真实执行一次；窗外 SkipMissed 只盖章不触达漏斗（缓存存活、cleanable 仍在、无历史）。
5. `mock_task_failures_are_handled_identically_by_both_triggers`（`:1033`）：未知路径 → `err.deleteNotConfirmed`；批准后对象被异类替换（目录变文件）→ 指纹失败 `err.fingerprintChanged` 且对象存活；目标全部消失 → 空结果成功返回；两个触发的结果签名逐项相等。

### 4.3 验证命令与输出证据

```sh
cargo test -p sift --lib schedule::
```

结果（本机实测）：

```text
running 13 tests
test schedule::tests::before_the_slot_it_waits ... ok
test schedule::tests::fires_in_the_scheduled_hour_once_per_day ... ok
test schedule::tests::missed_slot_is_made_up_inside_the_catchup_window ... ok
test schedule::tests::a_vanished_path_is_not_selected ... ok
test schedule::tests::missing_home_selects_nothing ... ok
test schedule::tests::enabled_rules_run_without_the_findings_auto_switch ... ok
test schedule::tests::a_symlink_inside_home_that_points_outside_is_not_selected ... ok
test schedule::tests::only_safe_approved_entries_inside_home_are_selected ... ok
test schedule::tests::rule_auto_and_manual_triggers_share_one_runner_and_agree ... ok
test schedule::tests::mock_task_honors_catchup_and_never_runs_an_expired_missed_slot ... ok
test schedule::tests::mock_task_clock_drives_an_automatic_fire_that_matches_the_manual_fire ... ok
test schedule::tests::mock_task_failures_are_handled_identically_by_both_triggers ... ok
test schedule::tests::rule_failures_reach_both_triggers_through_the_same_guardrails ... ok

test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 47 filtered out
```

全工作区回归（本机实测 `cargo test --workspace`）：sift 57 通过 / 3 ignored（ignored 为既有的联网 live 测试）、sift-analyze 102、sift-core 57、sift-monitor 35 + 集成 4、sift-platform 25、sift-scan 26、sift-store 80、sift-apps 20，**0 失败**。

静态检查：`cargo clippy -p sift --tests --all-features` 对改动文件零警告；输出中仅有的 3 条警告位于未触碰的 `sift-store`/`sift-apps` 既有代码。`cargo build -p sift`（非测试构建）通过，`#[cfg(test)]` 构造器不进入产物。

前端无测试框架（`package.json` 只有 `svelte-check` 类型检查，无 vitest/playwright），本轮无前端改动，故未运行前端检查。

### 4.4 测试证明的边界与仍缺的生产重构

- 已证明：在「同一任务定义 + 同一执行函数」的目标模式下，时钟自动触发与手动触发结果、副作用、失败语义完全一致；且所需零件（时钟决策、护栏漏斗、内存存储、扫描分析管线）生产环境齐备。
- 未证明 / 待生产化：
  1. 没有真实调度器枚举 `RoutineEntry` 并按 `cadence` 投递——`MockTask` 是测试替身；生产需实现 §2.5 的 trigger 泛化与任务队列。
  2. `run_mock_task` 直接调同步的 `perform_cleanup`；生产 `run_routine` 经 `cleanup_off_thread` 包装（Finder Apple Event 不能阻塞主线程），统一入口落地时需保留 off-thread 调度。
  3. `RoutineMode::Auto/Approve` 的行为分支仍未接线（harness 固定按 unattended 语义，与现网 `run_routine` 传 `automatic=true` 一致）。
  4. 三个删除出口（`perform_cleanup_with` / `run_rule` / `clean_now`）的函数级合并未做，本轮仅证明模式可行，未做大规模生产重构（符合任务约束）。

---

## 5. 变更清单

| 文件 | 性质 | 内容 |
|---|---|---|
| `src-tauri/src/schedule.rs` | 仅测试代码 | 测试模块新增统一触发测试区：2 个 Layer-1 规则测试 + 3 个 Layer-2 mock 任务测试及 harness（`TaskTrigger`/`MockTask`/`task_due`/`run_mock_task`/`AnalyzedFixture`/`TempTree`） |
| `src-tauri/src/analyze.rs` | 测试辅助 | 新增 `#[cfg(test)] pub(crate) fn AppServices::with_store_for_test`（`:103-115`），无生产行为变化 |
| `docs/review/2026-11-25-scheduled-tasks-review.md` | 文档 | 本报告 |

未 commit / 未 push / 未开 PR；工作区保留未提交改动，可直接盘点。

## 6. 遗留疑问（需 owner / 后续决策）

1. Routine 自动调度的产品形态：是把 cadence 真正接成 cron 式触发（R6.3 原文），还是统一收编为「每日槽 + 任务自身周期判断」？后者与现有 `decide()` 更契合。
2. `run_cleanup_rule` 手动运行无视 `enabled`（`cleanup_rules.rs:358-375`）而 `run_routine` 拒绝禁用项（`analyze.rs:1949-1951`）——统一时以哪个语义为准？
3. `EmptyTrash` 有计划无执行器（§3 候选说明），是否要补一个专属执行分支？
4. 统一模型迁移时，`settings.json` 内 50 条规则上限与 `routines.json` 100 条上限如何合并（建议统一 100 并在迁移期保留双写）。
5. 监控阈值触发（monitor）是否纳入同一任务队列，还是保留独立通路仅共享删除漏斗？这决定 §2.5 改造的波及面。
