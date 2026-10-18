# 代码评审：发版前 Post-merge Review

- 评审基线：`origin/main` @ `a0da56f`（含 #29–#36）
- 范围：近期合入的扫描引擎、清理/回收站、进度算法、调度任务、凭据、窗口主题等核心路径
- 规模：约 34.6k 行 Rust（6 crate + Tauri 适配层）+ Svelte/TS 前端
- 评审环境：独立 worktree `.worktrees/code-review`
- 状态：**发现项已修复**（分支 `fix/review-findings`），见 §6

## 0. 结论

**可以发内测（beta）。** 未发现阻断性安全或正确性缺陷。核心安全承诺（删除只进回收站、逐条批准、删除前指纹复核、不跨卷、token 不落盘）在代码层都成立且有测试覆盖。

评审发现的 1 个纵深防御缺口（计划任务 home 判定）与若干健壮性/精度/技术债项已在本分支修复；另有 1 个**功能缺口**（例行任务的自动模式未接线）记录在 §5，需产品决策。

## 1. 验证基线

修复前（基线 `a0da56f`）：

- `cargo test --workspace`：全绿（含 `--features remote-ai`）。
- `pnpm check`：0 错误 0 警告。
- `cargo clippy --workspace --all-targets`：23 条既有风格项。

修复后（本分支）：

- `cargo test --workspace`：全绿；新增 3 个计划任务选择测试（含符号链接逃逸）。
- `cargo clippy --workspace --all-targets`：**0 警告**。
- `pnpm check`：0 错误 0 警告（前端未改动）。

## 2. 安全相关（重点核查项，均通过）

| 承诺 | 结论 | 位置 |
|---|---|---|
| 删除只进回收站，失败逐项返回，绝不永久删除 | ✅ | `sift-platform::trash`，`analyze::perform_cleanup_with` |
| 未知路径拒绝（不是 cleanable 也不在上次分析里） | ✅ 返回 `deleteNotConfirmed` | analyze.rs |
| `Keep` 判定即使在 cleanable 也拒绝 | ✅ | analyze.rs |
| 删除前重新核对指纹（size/mtime/is_dir），变化即跳过 | ✅ `fingerprint_still_matches` | analyze.rs |
| 仅已逐条批准的 Safe + home 内才允许自动清理 | ✅ 三重条件（修复后 home 用 realpath 判定） | schedule.rs `select_approved_paths` |
| 自动清理运行前再次检查开关（防止一小时扫描期间被关闭） | ✅ `still_allowed` | schedule.rs |
| 跨设备边界不扫描（dev 不匹配跳过）+ firmlink/挂载去重 | ✅ visited_dirs | engine.rs |
| API token：macOS 走 Keychain；非 macOS 创建即 0600；永不写 settings.json | ✅ | credentials.rs |
| 远程 AI：enabled + consent + endpoint/model 齐全才用；只发脱敏路径 | ✅ `is_usable`、`redact_path` | settings.rs / remote.rs |
| 字节/文件计数全部 saturating，无溢出 panic | ✅ | size.rs / tree.rs |
| panic 兜底：清理任务 panic 时把所有请求项报为失败而非静默成功 | ✅ | clean_paths unwrap_or_else |
| 事件 epoch 过滤，旧扫描批次不污染当前视图 | ✅ | store.svelte.ts |
| 硬链接去重表有容量上限，超上限宁多算不 OOM | ✅ | tree.rs |

## 3. 发现的问题与处置

### P2-1 计划任务的 home 判定只用词面前缀（已修复）

- 位置：`src-tauri/src/schedule.rs`，`select_approved_paths`。
- 问题：monitor 真正执行删除时用 `resolved_inside_home`（双端 `canonicalize`，防符号链接逃逸，无法证明在内即拒绝）；计划任务**挑选**待删路径时只用 `Path::starts_with`。`starts_with` 是组件级比较，兄弟前缀（`/Users/me-evil` vs `/Users/me`）不会误命中，但 home 内一个指向外部的符号链接会被词面判定为"在内"。
- 可利用门槛：该路径还需是 cleanable 中的 `Safe` 且被用户逐条批准自动清理，属纵深防御缺口而非直接越权。
- 修复：选择器改用 `sift_monitor::policy::resolved_inside_home`，与删除侧共用同一边界规则；无法解析的路径视为"不在内"（它本来也会被指纹复核拒绝）。
- 测试：新增真实临时目录 fixture，覆盖仅 Safe+批准+在内被选中、缺 home 全不选、路径消失不选、**home 内符号链接指向外部不被选中**。

### P3-1 调度时间精度只到小时（保留，已记录）

`decide()` 只比较小时：到点那一小时内任意唤醒都会触发；补跑窗口按整点小时差算，最坏晚约 2 小时。每日清理非精确任务，可接受；如需精确需再比较分钟。

### P3-2 `decide()` 内不可达分支（已修复）

`catchup_hours == 0 || hours_after_slot > catchup_hours` 中第一个条件在进入该分支时已被前面 `catchup_hours > 0 && ...` 否定。改为直接 `return SkipMissed`。

### P3-3 巨型目录读取中途无进度心跳（已记录）

进度已按文件条目加权（#36），99% 长尾虚高已消除；但单个百万条目目录仍在一次连续 readdir 内完成，期间计数不连续。`getattrlistbulk` 按批返回，未来可在批次间用共享原子计数平滑尾部。属体验增强，不影响正确性。

### P3-4 托盘图标 `.expect()`（已修复）

非 macOS 分支在缺少 bundled icon 时会 panic。改为记录错误并跳过托盘（窗口已创建，托盘只是便利入口），不再让缺资源拖垮应用。

### P3-5 clippy 技术债 23 条（已清理）

while-let 循环、可派生 Default、重复文档注释、`len` 补 `is_empty`、`entry().or_insert_with`、冗余闭包、示例中的冗余绑定与空 `if` 等。清理后全 workspace 0 警告。

### P3-6 生产代码 unwrap（复核后保留）

均为 `Mutex::lock().unwrap()`（中毒才 panic）或启动期必需资源；托盘那一处已改。其余保持。

## 4. 架构与质量正面评价

- 分层清晰：`sift-core/platform/scan/analyze/store/monitor` 纯 Rust 可单测，`src-tauri` 仅做适配与命令翻译，产品逻辑没有泄漏到 UI 层。
- 安全护栏设计到位：删除链路多重校验、自动清理独立于交互清理且不可被 UI 取消、补跑先盖章防重复执行。
- 性能可度量：启动有常驻 trace（FCP ≤500ms）、扫描引擎有 timing 原子统计；进度算法有真机数据支撑并配回归测试。
- 取消响应性：coordinator 50ms 切片轮询、删除逐项之间检查，取消不产生半删除（在途单项交系统完成）。

## 5. 新发现：例行任务的自动模式未接线（功能缺口）

- `RoutineEntry.mode`（`Auto` / `Approve`）可持久化、可切换，但**没有任何后台调度消费它**：`run_routine` 仅在用户点击时执行，且忽略 `mode`。
- `routines.rs` 的模块注释称"实际调度在 monitor/background 层"，但 monitor 与 schedule 中都没有例行任务触发逻辑。
- 影响：REQUIREMENTS R6.3「例行任务调度器 + 自动模式（仅提醒/执行前确认/自动执行）」目前只实现了手动运行；`Auto` 是空操作。
- 风险：若未来直接接线，必须同时补齐 home/逐条批准/指纹复核（复用 `perform_cleanup` 已有护栏），否则会出现绕过审批的自动删除。
- 建议：要么在 UI 明确"当前版本需手动运行"，要么实现调度（后台线程按 cadence 触发 → 仅 `Safe` + `approved_for_auto` + home 内）。

## 6. 修复清单（本分支）

| 编号 | 处置 | 文件 |
|---|---|---|
| P2-1 | home 判定改 realpath + 3 个新测试 | `src-tauri/src/schedule.rs` |
| P3-2 | 删除不可达分支 | `src-tauri/src/schedule.rs` |
| P3-4 | 托盘图标缺失不再 panic | `src-tauri/src/tray.rs` |
| P3-5 | clippy 全清（13 个唯一警告） | core/platform/scan/store/tauri/examples |

## 7. 建议的发版前行动

1. 合并本修复分支后重新跑三平台 CI。
2. 按 `docs/release/preparation.md` 完成签名账号、元数据、CHANGELOG、三平台真机冒烟。
3. 对 §5 的例行任务自动模式做产品决策（本版说明"手动运行"或补齐调度）。
4. 先发 `v0.1.0-beta` pre-release 小范围内测，重点观察：回收站删除/还原、全盘/外接卷权限引导、计划任务自动清理、深浅色与托盘驻留。
