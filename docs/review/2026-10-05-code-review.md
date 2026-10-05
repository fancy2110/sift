# Sift 代码审查与改进计划（2026-10-05）

> 范围：对照 README / REQUIREMENTS / docs/architecture/overview.md 声称的目标，审查全部 6 个 Rust crate、src-tauri 适配层与 Svelte 前端。
> 审查基线：264 个测试通过、3 个 ignored、0 失败。
>
> **执行方式**：下列 23 个改进点，一个改进点一个分支，验证通过后各自提交一个 PR，按编号顺序推进。P3 轻微项不在本次范围。

## 总体结论

核心安全模型经逐行核实成立：远程模型无法单独授予 Safe（`sanitize_model_verdict` 三重条件）、结构家族不上传、API Key 不落盘、trash-only、monitor 纯策略正确。

主要问题集中在：

1. 三条删除路径中只有后台 monitor 的 `clean_now` 遵守全部护栏；计划任务与手动 `perform_cleanup` 能绕过逐条批准与指纹复核，而测试恰好只覆盖正确的那条。
2. 跨平台声称未经验证——Windows target 当前无法编译。
3. 扫描生命周期边界（切盘、陈旧事件）、删除后数据一致性存在缺陷。
4. Treemap、习惯学习消费侧、浏览器模式、例行任务页属文档声称但未交付。

## 改进清单

### P0 严重（安全/正确性破口）

| # | 分支 | 问题与位置 | 验收标准 |
|---|---|---|---|
| 1 | `fix/p0-1-windows-build` | `sift-monitor/src/policy.rs:373-379`：`last_write_time()` 返回 `u64` 却当 `Option::map`，Windows 无法编译 | `cargo check --target x86_64-pc-windows-msvc`（或可用 windows target）通过；为 `modified_ms` 补单测 |
| 2 | `fix/p0-2-trash-rule-home` | `sift-analyze/src/rules.rs:245-248`：`in_home` 为重言式（任何绝对路径恒 true），任意位置名为 Trash 的目录被提名 Safe | home/库目录之外的 `Trash` 目录不提名（或降级 Review）；补正反向用例 |
| 3 | `fix/p0-3-scheduled-approval` | `src-tauri/src/schedule.rs:89-101` 与 `analyze.rs run_routine:1050-1073` 只按 Safe+home 前缀筛选，不要求 `approved_for_auto` | 两条路径均加 `approved_for_auto` 条件；未批准条目不被删除；补测试 |
| 4 | `fix/p0-4-cleanup-fingerprint` | `src-tauri/src/analyze.rs:345-454 perform_cleanup` 删除前不调用 `still_matches` 复核指纹 | 删除前对每个条目复核 size/mtime/类型，不匹配跳过；与 monitor 口径一致；补测试 |
| 5 | `fix/p0-5-calibrate-saturating` | `sift-core/src/tree.rs:579-590` + `size.rs:84-89`：向下校准差值 wrapping 后被 saturating_add 钳到 u64::MAX，祖先链变 18 EB | 向下校准后总数等于实测值；补向下校准测试 |
| 6 | `fix/p0-6-prune-current-dir` | `src/lib/store.svelte.ts:794-804`：删除当前目录后取 parent 时记录已删，currentNodeId 恒 null，用户困在空白视图 | 删除当前目录后跳到幸存祖先；面包屑/列表可用；补场景验证 |

### P1 功能差距

| # | 分支 | 问题与位置 | 验收标准 |
|---|---|---|---|
| 7 | `fix/p1-7-scan-switch-epoch` | `store.svelte.ts:331-347` + `scanner.rs:243-245`：扫描中切盘被后端拒绝、旧事件回填、无代次隔离 | 切盘前先取消旧扫描；事件按 epoch 过滤；任意时刻头部与树数据一致 |
| 8 | `fix/p1-8-size-recompute` | `store.svelte.ts:752-807 pruneIds`：剪枝后祖先大小不回减（违反 R4.4） | 删除后祖先 size/fileCount 实时重算；watcher 删除同路径生效 |
| 9 | `feat/p1-9-treemap` | R3.3：前端无 Treemap（core 已有布局算法与测试） | Treemap 视图接实时树，可点击下钻、与列表/面包屑焦点一致 |
| 10 | `feat/p1-10-habit-learning` | `habits.rs:138-152`、`reason.rs:153`：`is_habitually_kept`/`Learned` 无生产调用方 | 反复 Keep 接入分析主路径并抑制/降级对应提示；端到端可验证 |
| 11 | `feat/p1-11-browser-mode` | `App.svelte:11`、`store.svelte.ts:289-300`：无 Tauri 环境时 init 中断，`pnpm dev` 不可用 | 非 Tauri 环境降级 mock，页面可浏览、操作有明确提示，无未处理拒绝 |
| 12 | `feat/p1-12-routines-page` | R6.5：例行任务仅在设置弹窗内可操作，无独立页面 | 独立例行任务页：启动/启停/删除接真实后端 |

### P2 实现问题

| # | 分支 | 问题与位置 | 验收标准 |
|---|---|---|---|
| 13 | `fix/p2-13-supplementary-groups` | `deletable.rs:245-262`、`dir.rs:377-389,935-968`：只比 euid/egid 忽略补充组（`/Applications` 误禁） | 统一用 `getgroups` 判定 group 写权限；与 `classify_denial` 口径一致 |
| 14 | `fix/p2-14-inside-home-canonical` | `policy.rs:322-327`：纯词法前缀，`..` 与指向卷外的符号链接可绕过 | 规范化/realpath 后判定；无法证明在家时拒绝 |
| 15 | `fix/p2-15-focus-normalize` | `engine.rs:1602-1617`：focus 字符串未归一化，软链/末尾斜杠导致节流全程不放开，全盘退化为单线程 | 比较前做路径归一化（组件级），focus 列出后及时放开并发；补测试 |
| 16 | `fix/p2-16-cache-guardrails` | `adjudicate.rs:237-247`：缓存命中直接返回旧 Safe，不重过当前策略/同意 | 缓存 Safe 在授予前重过当前护栏（阈值/开关/同意），收紧策略后旧 Safe 不再自动可用 |
| 17 | `fix/p2-17-size-only-hardlinks` | `engine.rs:2096-2118 size_only_walk`：预算拒绝子树无硬链接去重，总量虚高 | size-only 路径维护 seen_links，与主路径口径一致；补测试 |
| 18 | `fix/p2-18-cancel-responsiveness` | `engine.rs:427-430,2096-2118`：大子树遍历不响应取消；`remote.rs:216-223` 重试睡眠不响应取消/consent 撤回 | size_only_walk 设取消点；重试等待可中断且重检 consent |
| 19 | `fix/p2-19-async-delete` | `analyze.rs:345` 等：同步命令在主线程跑 Finder AppleScript，UI 冻结 | 删除走 async / spawn_blocking，UI 不阻塞；逐项结果不变 |
| 20 | `fix/p2-20-journal-errors` | `journal_sqlite.rs:51-71`：写盘错误全静默 + 无界通道 | 写盘错误可观测（日志/回调/计数）；通道有界并在背压时可控 |
| 21 | `fix/p2-21-immutable-flag` | `deletable.rs:233-242`：未检测 macOS `uchg/schg`，不可删目录误报 deletable | 检出 immutable flags 并据此标记；补用例 |
| 22 | `fix/p2-22-close-dir-idempotent` | `tree.rs:518-537`：`close_dir` 二次调用会重复传播 size，现有测试只覆盖空目录 | 非空目录二次关闭不双计；补非空幂等测试 |
| 23 | `fix/p2-23-monitor-result-align` | `monitor.rs:456-469`：按位置 zip 对齐 Remover 结果，后端重排即错配 | 按条目路径/键对齐结果；错配不再发生 |

## 不在本次范围（P3 轻微项，择机处理）

Toast 失败仍显示成功样式；卷容量 0 时 NaN%；右键菜单无 Escape 关闭；图标按英文路径硬编码；后端用户文案硬编码（中英）；习惯日按 UTC；远程 prompt 契约矛盾（array vs json_object）；`ScanHandle::join` 忙等；token 文件先写后 chmod；SQLite schema 无版本/迁移；计划任务错过不补跑；`home_dir` 命令未注册；`last_report.clear()` 导致短期不可删。

## 每个 PR 的统一验证

- `cargo test`（相关 crate + workspace）
- `cargo clippy --all-targets`
- `pnpm check`（涉及前端时）
- 涉及跨平台声称时至少做对应 target 的 `cargo check`
