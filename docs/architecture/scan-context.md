# 扫描时环境上下文与维护知识沉淀

> 需求：每次扫描时，应用、包管理器、项目等环境信息都要进入分析视野；模型要能基于这些上下文判断；判断结果和维护方式要持续沉淀。
>
> 关键取舍：**“模型分析到”不等于“每次把所有 App/PackageManager 都发给模型”**。正确做法是每次扫描刷新**环境上下文**，本地规则先判，模型只处理新增、变化和歧义项；未变化的结论用带上下文哈希的缓存复用。这样既满足“每次都在视野里”，又不会把扫描变成一次昂贵的全量 LLM 调用。

## 1. 目标与非目标

**目标**

1. 每次扫描都产出当次 `ScanContext`：安装了哪些应用、哪些包管理器、哪些项目、版本、关键维护能力。
2. 应用/包管理器的残留候选进入同一次分析；模型能看到上下文并解释“这个路径为什么属于它”。
3. 模型可以提出**维护建议**（清缓存、prune、卸载孤儿包、移除旧 toolchain），但只能是封闭动作枚举，不能是 shell。
4. 每次扫描结束把事实、判断、维护建议写回 `app_profiles` / `pm_profiles` / `maintenance_hints`。
5. 上下文和结论可失效、可追溯、可解释；扫描失败或模型失败只降级，不影响扫描本身。

**非目标**

- 不在扫描热路径里做全盘应用数据枚举；
- 不把真实路径、文件内容、项目源码发给模型；
- 不让模型直接执行命令或授权自动删除；
- 不要求每次扫描都调用远程模型。

## 2. 扫描管线

```
Scan start
  │
  ├─ 0. 读取上次 EnvironmentSnapshot / MaintenanceKnowledge
  │
  ├─ 1. 文件系统扫描（现有 sift-scan，并行）
  │
  ├─ 2. 环境增量扫描（有界、带缓存）
  │     ├─ App inventory   （安装目录变化才重扫）
  │     ├─ PM inventory    （二进制/版本/配置根）
  │     └─ Project context （manifest + lockfile）
  │
  ├─ 3. 候选生成
  │     ├─ 现有目录/文件规则
  │     └─ 应用残留 + 包管理器缓存/孤儿候选
  │
  ├─ 4. 本地规则判定（全部候选）
  │
  ├─ 5. 模型批次（只送新增/变化/歧义 + 上下文摘要）
  │     ├─ 候选判定
  │     └─ 维护建议（封闭动作）
  │
  ├─ 6. 合并缓存结论 + 新结论
  │
  └─ 7. 持久化
        ├─ ScanContext 快照
        ├─ AppProfile / PmProfile / ProjectContext
        ├─ MaintenanceHint（Proposed/Validated/Rejected）
        └─ 判定缓存（带 context_hash）
```

### 2.1 每次扫描必须做的事

| 步骤 | 每次扫描 | 说明 |
|---|---|---|
| 文件系统扫描 | 是 | 现有行为 |
| 环境快照刷新 | 是（增量） | 只扫变化的安装目录/二进制/项目根 |
| 本地规则判定 | 是（全量） | 确定性、免费、可审计 |
| 上下文进入 prompt | 是 | 即使没有新候选，也带上环境摘要 |
| 模型调用 | 否（按需） | 没有新增/变化/歧义时不调用 |
| 缓存结论复用 | 是 | 带 context_hash，仍计入“模型已分析” |
| 维护知识写回 | 是 | 每次扫描结束都更新 `last_seen`/`status` |

> “模型都要分析到”在实现上意味着：**每个相关对象在当次分析中都有结论**——要么是本次模型新判的，要么是上下文未变、由缓存复用的模型结论，要么是本地规则结论。三者都带来源，不会出现“这次扫描完全没看过它”。

## 3. `ScanContext` 数据模型

```rust
pub struct ScanContext {
    pub scanned_at_ms: i64,
    pub roots: Vec<PathBuf>,
    pub apps: Vec<AppFact>,
    pub package_managers: Vec<PmFact>,
    pub projects: Vec<ProjectFact>,
    pub knowledge_version: u32,
    pub stale: bool,
}

pub struct AppFact {
    pub key: AppKey,
    pub name: String,
    pub version: Option<String>,
    pub bundle_id: Option<String>,
    pub install_path: String,       // 展示用脱敏路径
    pub last_used_days: Option<u32>,
    pub artifact_count: u32,
    pub maintenance: Vec<MaintenanceHintId>,
}

pub struct PmFact {
    pub id: PmId,
    pub version: Option<String>,
    pub detected: bool,
    pub roots: Vec<String>,         // 展示用
    pub projects: Vec<String>,      // 项目根，脱敏
    pub capabilities: Vec<PmAction>, // 只给能力枚举，不给 shell
    pub maintenance: Vec<MaintenanceHintId>,
}

pub struct ProjectFact {
    pub root: String,               // 脱敏
    pub pm: PmId,
    pub manifest: String,
    pub lockfile: Option<String>,
    pub pm_version: Option<String>,
    pub dirty: bool,                // 有未提交改动时提高谨慎
}
```

上下文只放**分析需要的最小事实**：

- 不放文件内容、不放真实用户名路径、不放环境变量值；
- 项目根用 `~/code/app` 这类脱敏形式；
- 包管理器只给能力枚举，不给命令字符串；
- 应用只给身份、版本、路径、最近使用天数、残留数量。

## 4. 环境增量扫描

### 4.1 应用清单

**macOS**

- 监听 `/Applications`、`~/Applications` 的目录 mtime；只有变化时才重扫；
- 读取 `Contents/Info.plist` 的 `CFBundleIdentifier`、`CFBundleName`、`CFBundleShortVersionString`；
- 代码签名身份（Team ID）作为身份校验；
- 首次全量，之后增量：新增/删除/版本变化的应用进入 `changed_apps`。

**Windows**

- 注册表卸载键的变化检测（键的 `LastWriteTime`）；
- MSIX 包清单；
- 首次全量，之后按注册表/安装目录变化增量。

**Linux**

- `.desktop` 目录 mtime；
- dpkg/rpm/flatpak/snap 数据库的 mtime/版本；
- 增量同理。

### 4.2 包管理器清单

- 二进制存在性 + `--version`（只对白名单工具，超时 2s）；
- 配置根存在性；
- 版本变化 → 对应 `PmFact` 标记 changed，相关 capability 重新校验；
- 项目 lockfile 变化 → 对应 `ProjectFact` 标记 changed。

### 4.3 项目上下文

- 只扫描本次扫描根之下的项目根，或用户显式打开的项目；
- 向上查找 manifest/lockfile 有深度上限（如 8 层）；
- 单次最多 N 个项目（如 200），超出按最近修改优先；
- lockfile 是权威：`pnpm-lock.yaml` → pnpm，`package-lock.json` → npm，`yarn.lock` → yarn；
- `packageManager` 字段优先于全局默认。

### 4.4 缓存与失效

`ScanContext` 的每个事实带 `observed_at_ms` 和 `source_hash`：

- 应用：安装路径 + bundle id + 版本 + Info.plist mtime；
- 包管理器：二进制路径 + 版本 + 配置根 mtime；
- 项目：manifest/lockfile mtime + 内容哈希（可选）。

失效触发：

1. 应用安装/卸载/升级；
2. 包管理器版本变化；
3. lockfile/manifest 变化；
4. 候选指纹（路径、大小、mtime）变化；
5. 维护知识 catalog 版本变化；
6. 用户反馈。

## 5. 模型 prompt 扩展

### 5.1 上下文摘要

在现有 system prompt 后追加 `Environment context` 段，用户消息里放紧凑 JSON：

```json
{
  "scanned_at_days": 0,
  "apps": [
    {"id": "com.foo.app", "name": "Foo", "version": "1.2.3", "install": "/Applications/Foo.app",
     "last_used_days": 12, "artifacts": 8}
  ],
  "package_managers": [
    {"id": "pnpm", "version": "9.12.0", "detected": true, "projects": ["~/code/app"],
     "capabilities": ["cleanCache", "pruneStore", "dedupe"]}
  ],
  "projects": [
    {"root": "~/code/app", "pm": "pnpm", "lockfile": "pnpm-lock.yaml", "pm_version": "9.12.0"}
  ],
  "knowledge": [
    {"subject": "pm:pnpm", "action": "pruneStore", "status": "validated", "confidence": 0.9}
  ]
}
```

约束：

- 摘要 token 预算（如 ≤ 1500 tokens）；超出按“最近使用/最近变化/大体积”截断；
- 不含真实绝对路径、用户名、文件内容；
- 不知道的事实写 `unknown`，不要让模型补全。

### 5.2 响应 schema 扩展

```json
{
  "verdicts": [
    {"id": "...", "safety": "safe|review|keep", "confidence": 0.0, "reason": "...", "impact": "..."}
  ],
  "maintenance": [
    {
      "subject": "pm:pnpm",
      "action": "pruneStore",
      "confidence": 0.9,
      "reason": "一条中文",
      "impact": "一条中文"
    }
  ]
}
```

`action` 是**封闭枚举**，映射到包管理器能力目录：

```
cleanCache | pruneStore | dedupe | removeOrphan | removeOldVersion |
uninstallPackage | verifyIntegrity | updatePackageIndex | repair
```

模型**不能**输出命令、参数、shell、路径。动作到命令的映射由本地 `PmKnowledge` 完成。

### 5.3 模型边界

沿用现有护栏：

- 模型不能授权 `AutoSafe`；`Safe` 仍需家族允许 + 置信度阈值；
- 模型不能决定共享路径；
- 模型输出只进入 `MaintenanceHint::Proposed`，不直接执行；
- 系统包管理器、管理员动作、`Danger` 动作永远不自动。

## 6. 判定缓存与“每次模型分析到”

### 6.1 缓存键

```rust
CacheKey {
  candidate_fingerprint, // path + size + mtime + kind
  context_hash,          // scan context 的相关子集哈希
  prompt_version,
  model_id,
}
```

缓存命中条件：

- 候选指纹不变；
- 相关应用/包管理器/项目事实不变；
- prompt 版本和模型不变；
- 未超过复用窗口（如 7 天）。

### 6.2 缓存失效

- 应用升级 → 该应用相关候选全部失效；
- 包管理器升级 → 该 PM 的能力/候选失效；
- lockfile 变化 → 项目相关候选失效；
- 用户反馈 → 对应候选/维护建议失效。

### 6.3 报告里的来源

每个结论都带 `source`：

- `rule`：本地规则；
- `model`：本次模型；
- `model:cached`：缓存的模型结论；
- `user`：用户显式决定。

UI 不能把 `model:cached` 显示成“这次没分析”。它仍然是该对象在当次上下文下的有效结论。

## 7. 维护知识沉淀

### 7.1 数据模型

```rust
pub struct MaintenanceHint {
    pub id: MaintenanceHintId,
    pub subject: MaintenanceSubject, // app:... | pm:... | project:...
    pub action: PmAction,
    pub source: HintSource,          // Model | Rule | User | Catalog
    pub status: HintStatus,          // Proposed | Validated | Rejected | Stale
    pub confidence: f32,
    pub reason: String,
    pub impact: String,
    pub version_range: Option<VersionRange>,
    pub created_at_ms: i64,
    pub last_seen_ms: i64,
    pub last_validated_ms: Option<i64>,
    pub success_count: u32,
    pub failure_count: u32,
}
```

状态机：

```
Proposed ──用户接受/执行成功──► Validated
   │                              │
   └──用户拒绝/执行失败──► Rejected / Stale
                                  │
                         版本变化/重新探测
                                  ▼
                              Proposed
```

### 7.2 写入时机

每次扫描结束：

1. 更新 `AppProfile` / `PmProfile` / `ProjectContext` 的 `last_seen`；
2. 合并模型返回的 `maintenance`：同 subject + action 去重，更新置信度/理由；
3. 标记本次未出现的 subject 为 `missing`（不立即删除）；
4. 记录 `ScanRun { id, started, finished, context_hash, model_used, cache_hits, new_hints }`。

### 7.3 只沉淀“可复用”的知识

- 应用级：某个路径属于某个应用（安装/使用/卸载证据）；
- 包管理器级：某个能力在某个版本范围内有效；
- 项目级：这个项目使用哪个 PM、哪个 lockfile；
- 不沉淀：一次性路径、用户临时文件、模型猜测的路径。

## 8. 存储 schema

```sql
CREATE TABLE scan_runs (
  id TEXT PRIMARY KEY,
  started_at_ms INTEGER NOT NULL,
  finished_at_ms INTEGER,
  context_hash TEXT NOT NULL,
  model_used INTEGER NOT NULL,
  cache_hits INTEGER NOT NULL,
  new_hints INTEGER NOT NULL
);

CREATE TABLE scan_contexts (
  hash TEXT PRIMARY KEY,
  scanned_at_ms INTEGER NOT NULL,
  payload TEXT NOT NULL
);

CREATE TABLE maintenance_hints (
  id TEXT PRIMARY KEY,
  subject TEXT NOT NULL,
  action TEXT NOT NULL,
  source TEXT NOT NULL,
  status TEXT NOT NULL,
  confidence REAL NOT NULL,
  reason TEXT NOT NULL,
  impact TEXT NOT NULL,
  version_range TEXT,
  created_at_ms INTEGER NOT NULL,
  last_seen_ms INTEGER NOT NULL,
  last_validated_ms INTEGER,
  success_count INTEGER NOT NULL DEFAULT 0,
  failure_count INTEGER NOT NULL DEFAULT 0,
  UNIQUE(subject, action)
);
```

与 `app_profiles.db` / `pm_profiles.db` 同库不同表，或独立 `scan_context.db`；建议同库，减少连接数。

## 9. UI

- 扫描完成后新增 “Environment” 面板：
  - 检测到 N 个应用、M 个包管理器、K 个项目；
  - “本次新增/变化”列表；
  - 维护建议按 subject 分组，标注 `AI 建议` / `已验证` / `已拒绝`；
- 维护建议必须展示：动作、影响、是否需要管理员、是否需要网络、是否会改 lockfile；
- 扫描报告里的每条结论显示来源（规则/模型/缓存/用户）；
- 提供“这次不用再分析这个应用”的 dismiss，写入偏好，不影响安全结论。

## 10. 失败与降级

| 失败 | 行为 |
|---|---|
| 应用清单超时/权限不足 | 使用上次快照，`ScanContext.stale = true`，扫描继续 |
| 包管理器 `--version` 超时 | `version = unknown`，能力降级为 `Review` |
| 项目扫描超限 | 只保留最近修改的 N 个，标记 `truncated` |
| 模型超时/失败 | 本地规则结论照常；上下文和缓存不变 |
| 缓存损坏 | 丢弃缓存，重新判定；不阻塞扫描 |
| 维护建议冲突 | 以 `Validated` > `Proposed` > `Rejected` 排序；冲突写审计 |

## 11. 成本与性能预算

- 环境增量扫描：目标 < 300ms（缓存命中时 < 50ms）；
- 应用重扫：仅安装目录 mtime 变化时；
- 项目扫描：最多 200 个，深度 8；
- 模型上下文：≤ 1500 tokens；
- 模型候选：沿用现有 batch（≤ 40）；
- 缓存命中：不调用模型；
- 每次扫描最多新增 N 条维护建议（如 10），避免刷屏。

## 12. 分阶段落地

### Phase 1：上下文骨架（1 周）

- `ScanContext` 类型 + 空实现 + prompt 注入 + 持久化；
- 扫描报告显示“环境：0 个应用 / 0 个 PM”（先通链路）。

### Phase 2：macOS inventory（2 周）

- `/Applications` + `Info.plist` + 签名身份；
- Homebrew/npm/pnpm/yarn/cargo/pip/conda/go 检测；
- 项目 lockfile 扫描；
- 上下文摘要进入 prompt。

### Phase 3：缓存与失效（1–2 周）

- `CacheKey` + `model:cached` 来源；
- 应用/PM/项目版本变化失效；
- 扫描报告显示缓存命中。

### Phase 4：维护建议（2 周）

- 模型 `maintenance` 输出 + 封闭动作映射；
- `MaintenanceHint` 状态机；
- UI “AI 建议/已验证/已拒绝”。

### Phase 5：跨平台与增量优化（持续）

- Windows 注册表/MSIX、Linux desktop/dpkg/flatpak/snap；
- 目录 mtime 增量、项目上限、上下文截断；
- 社区 capability catalog。

## 13. 测试与指标

**测试**

- 上下文序列化不含真实路径/用户名；
- token 预算截断按优先级；
- 缓存命中/失效矩阵：应用升级、PM 升级、lockfile 变化、候选 mtime 变化；
- 模型返回非法 action → 丢弃并记录；
- 模型返回 `safe` → 仍走 `sanitize_model_verdict` 降级；
- 应用清单超时 → stale 上下文 + 扫描成功；
- 维护建议去重/状态机；
- 每次扫描都产生 `ScanRun` 记录。

**指标**

- 上下文构建耗时 P95；
- 缓存命中率；
- 每次扫描模型调用次数/ token；
- 新增维护建议的接受率；
- `Proposed → Validated` 转化率；
- 因上下文缺失导致的 `review` 比例。

## 14. 与现有代码的接入点

| 现有 | 改动 |
|---|---|
| `sift-analyze::Analyzer::analyze` | 增加 `context: &ScanContext` 参数；候选生成后合并 app/PM 候选 |
| `sift-analyze::prompt::build_at` | system/user 增加 environment 段 |
| `sift-analyze::remote` | 解析 `maintenance` 数组；非法动作丢弃 |
| `sift-analyze::adjudicate` | 缓存键加 `context_hash`；来源加 `model:cached` |
| `sift-store` | 新增 `scan_runs` / `maintenance_hints` / `scan_contexts` |
| `src-tauri/src/analyze.rs` | 扫描完成后构建/刷新上下文，传入 analyzer，持久化 |
| `sift-apps` / `sift-pm` | 提供 inventory 和 `ScanContext` 构建 |
| UI | Environment 面板、来源标注、维护建议卡片 |

## 15. 结论

“每次扫描模型都分析到”应该落成：

1. **每次扫描都刷新环境上下文**（增量、带缓存）；
2. **本地规则先判全量**，模型只处理新增/变化/歧义；
3. **未变化的模型结论用 context_hash 缓存复用**，仍算“已分析”；
4. **每次扫描都写回维护知识**，形成 `Proposed → Validated → Stale` 的生命周期；
5. **模型只给封闭动作建议**，命令映射、权限、执行仍由本地能力目录控制。

这样扫描既完整又可控：不会因为“每次都要模型看所有应用”而变得昂贵、缓慢或不可审计。
