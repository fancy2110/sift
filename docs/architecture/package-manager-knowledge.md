# 包管理器知识沉淀：安装、卸载、清理与优化

> 目标：Sift 不只认识“目录”，还要认识“谁在管理这些目录”。对 Homebrew、npm/pnpm/yarn、Cargo、pip、conda、Go、RubyGems、Composer、apt/dnf/pacman、flatpak/snap、winget/scoop/Chocolatey、rustup/nvm/pyenv 等常见包管理器，沉淀出**可检测、可解释、可版本化、可安全执行**的能力目录；并从每次使用中学习哪些命令有效、哪些路径属于哪个工具。

## 1. 为什么需要单独的一层

现有 `crates/sift-analyze/src/cleanup.rs` 已经有一张很小的“工具 → 清理命令”表（cargo、npm、pnpm、yarn、Gradle、Homebrew、pip、Xcode）。它解决的是“这个目录怎么清”，但没有解决：

- 这个工具是否安装、什么版本、当前项目用的是哪个包管理器；
- 这是**项目本地**依赖还是**全局** store/cache；
- 卸载一个全局包 / 旧 toolchain / 旧 runtime 是否安全；
- 命令在当前版本是否还存在（例如 Yarn classic 与 Berry、npm 的 `cache clean` 与 `cache verify`）；
- 命令执行后是否成功、释放了多少空间、下次还应不应该推荐；
- 一个项目同时有 `package-lock.json`、`pnpm-lock.yaml`、`yarn.lock` 时该信谁。

因此需要一层 **PackageManagerProfile**，与 `AppProfile` 并列：应用侧回答“卸载这个 app 要清哪些数据”，包管理器侧回答“这个工具链的依赖、缓存、旧版本、孤儿包怎么安全处理”。

## 2. 与 AppProfile 的边界

| 维度 | AppProfile | PackageManagerProfile |
|---|---|---|
| 主体 | 一个 GUI/桌面应用 | 一个包管理器 / 工具链 |
| 关键身份 | bundle id、签名、安装路径 | 可执行文件、版本、配置根、项目 manifest/lockfile |
| 典型动作 | 卸载应用、清残留 | 清缓存、prune、去重、卸载全局包、移除旧 toolchain |
| 作用域 | 用户域为主 | project / global / system 三档 |
| 风险 | 用户数据、共享容器 | 全局依赖、系统包、项目可复现性 |
| 学习信号 | 安装/使用/卸载 diff | 命令结果、版本变化、项目 lockfile、用户反馈 |

两者共享 `Safety`、`Reason`、隔离区/撤销、模型护栏和 UI 的“计划 → 确认 → 执行 → 反馈”流程。

## 3. 知识模型

```rust
pub struct PmId(&'static str); // "homebrew" | "npm" | "pnpm" | "cargo" | ...

pub struct PackageManagerProfile {
    pub id: PmId,
    pub detected: bool,
    pub version: Option<String>,
    pub executable: Option<PathBuf>,
    pub roots: Vec<PmRoot>,          // global store / cache / config / bin
    pub capabilities: Vec<Capability>,
    pub projects: Vec<ProjectContext>,
    pub last_inventory_ms: i64,
    pub observations: u32,
    pub failures: u32,
}

pub struct PmRoot {
    pub path: PathBuf,
    pub kind: PmRootKind,            // Store/Cache/Config/Bin/Logs
    pub scope: Scope,                // Project/Global/System
    pub shared: bool,                // 可能被多个项目/工具共享
}

pub struct Capability {
    pub action: PmAction,            // Install/Upgrade/Uninstall/Clean/Prune/Verify/Doctor/Dedupe/List
    pub command: CommandTemplate,
    pub scope: Scope,
    pub requires_admin: bool,
    pub destructive: Destructiveness, // Safe/Review/Danger
    pub requires_network: bool,
    pub idempotent: bool,
    pub preconditions: Vec<Precondition>,
    pub impact_key: &'static str,
    pub version_range: VersionRange,
    pub source: KnowledgeSource,     // Curated/Docs/Observed/User
}

pub struct CommandTemplate {
    pub program: String,             // 只允许证书/白名单内的可执行文件
    pub args: Vec<String>,           // 模板参数，必须经过校验
    pub cwd: Cwd,                    // ProjectRoot/Home/None
    pub env: Vec<(String, String)>,
    pub timeout_secs: u64,
}

pub struct ProjectContext {
    pub root: PathBuf,
    pub pm: PmId,
    pub manifest: PathBuf,
    pub lockfile: Option<PathBuf>,
    pub pm_version: Option<String>,  // packageManager 字段 / .tool-versions / lockfile 版本
    pub detected_by: ProjectSignal,  // Lockfile/PackageManagerField/ToolVersions/User
}
```

`Capability` 是**数据**，不是任意 shell。所有参数来自模板 + 已校验的包名/项目路径；禁止拼接未转义字符串，禁止 `sh -c`。

## 4. 检测与项目上下文

### 4.1 工具检测（只读）

1. 已知可执行文件名 + `which`/`where`；
2. 运行 `--version`（仅对白名单内的已知工具；超时、捕获输出、失败不阻塞）；
3. 识别配置根：
   - Homebrew：`/opt/homebrew`、`/usr/local`、`brew --prefix`；
   - npm：`~/.npm`、`npm config get cache`；
   - pnpm：`~/.local/share/pnpm/store`、`pnpm store path`；
   - yarn：`~/.cache/yarn`、`.yarn/cache`；
   - Cargo：`~/.cargo`、`CARGO_HOME`；
   - pip：`~/.cache/pip`、`pip cache dir`；
   - conda：`~/miniconda3`、`~/.conda`、`conda info --json`；
   - Go：`GOMODCACHE`、`GOCACHE`、`go env`；
   - RubyGems：`gem env`；
   - Composer：`~/.composer/cache`、`composer config cache-dir`；
   - apt/dnf/pacman/flatpak/snap/winget/scoop/choco：系统路径 + 包数据库；
   - rustup/nvm/pyenv：工具链根目录。

### 4.2 项目上下文（优先级高于全局默认）

从项目根向上/向下找 manifest 和 lockfile：

| 生态 | manifest | lockfile | 版本信号 |
|---|---|---|---|
| Node | `package.json` | `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` / `bun.lockb` | `packageManager` 字段、`.nvmrc`、`.tool-versions` |
| Rust | `Cargo.toml` | `Cargo.lock` | `rust-toolchain.toml` |
| Python | `pyproject.toml` / `requirements.txt` | `poetry.lock` / `uv.lock` / `Pipfile.lock` | `.python-version` |
| Go | `go.mod` | `go.sum` | `go` 指令版本 |
| Ruby | `Gemfile` | `Gemfile.lock` | `.ruby-version` |
| PHP | `composer.json` | `composer.lock` | `composer.lock` 的 plugin API |
| Java | `pom.xml` / `build.gradle` | — | wrapper 版本 |
| Elixir | `mix.exs` | `mix.lock` | `.tool-versions` |

规则：

- **lockfile 是权威**：项目里有 `pnpm-lock.yaml` 就不要建议 `npm install`；
- `packageManager` 字段优先于“全局默认”；
- 同一项目多个 lockfile → `Review`，让用户选择；
- 项目上下文只影响该项目的动作，不改变全局 profile。

## 5. 能力目录

以下命令是**模板**，执行前还要过版本范围、前置条件和安全校验。以各工具官方文档为准。

### 5.1 Homebrew（macOS/Linux）

| 动作 | 命令 | 作用域 | 安全 |
|---|---|---|---|
| 列已安装 | `brew list --formula` / `--cask` | global | 只读 |
| 查过期 | `brew outdated` | global | 只读 |
| 升级 | `brew upgrade` | global | Review |
| 卸载 formula | `brew uninstall <name>` | global | Review |
| 卸载 cask | `brew uninstall --cask <name>` | global | Review |
| 深度卸载 cask | `brew uninstall --zap --cask <name>` | global | Danger（交给 AppProfile 计划） |
| 清旧下载 | `brew cleanup --prune=all` | global | Safe |
| 清孤儿依赖 | `brew autoremove` | global | Safe/Review |
| 诊断 | `brew doctor` / `brew missing` | global | 只读 |

注意：`--zap` 是应用卸载，不是普通清理；必须走 AppProfile 的共享检测和用户确认，不能由“优化”按钮触发。

### 5.2 Node 生态

| 工具 | 动作 | 命令 | 安全 |
|---|---|---|---|
| npm | 安装 | `npm install` / `npm ci` | Review（会跑 postinstall） |
| npm | 卸载项目依赖 | `npm uninstall <pkg>` | Safe（项目内） |
| npm | 卸载全局包 | `npm uninstall -g <pkg>` | Review |
| npm | 清缓存 | `npm cache verify` | Safe |
| npm | 强制清缓存 | `npm cache clean --force` | Safe（较旧版本） |
| npm | 去重 | `npm dedupe` | Review（改 lockfile） |
| npm | 清多余 | `npm prune` | Safe（项目内） |
| pnpm | 安装 | `pnpm install` | Review |
| pnpm | 卸载 | `pnpm remove <pkg>` / `-g` | Safe/Review |
| pnpm | 清 store | `pnpm store prune` | Safe |
| pnpm | 去重 | `pnpm dedupe` | Review |
| yarn | 安装 | `yarn install` | Review |
| yarn | 卸载 | `yarn remove <pkg>` | Safe |
| yarn | 清缓存 | `yarn cache clean` | Safe |
| yarn berry | 去重 | `yarn dedupe` | Review |

注意：

- 不要手工删 `node_modules` 以外的 pnpm store：store 用硬链接，手工删会破坏其他项目；
- `npm install` 会执行生命周期脚本，属于**变更**而不是只读；
- 全局包可能是用户日常 CLI（`vercel`、`typescript`），卸载前列出依赖它的项目/命令。

### 5.3 Rust

| 动作 | 命令 | 作用域 | 安全 |
|---|---|---|---|
| 列全局安装 | `cargo install --list` | global | 只读 |
| 卸载全局 crate | `cargo uninstall <name>` | global | Review |
| 项目构建清理 | `cargo clean` | project（cwd=项目根） | Safe |
| 清旧 registry 缓存 | `cargo cache --autoclean`（需 `cargo-cache`） | global | Safe/Review |
| 查重复依赖 | `cargo tree -d` | project | 只读 |
| 更新依赖 | `cargo update` | project | Review（改 lockfile） |
| 卸载旧 toolchain | `rustup toolchain uninstall <name>` | global | Review |

注意：`~/.cargo/registry` 不建议手工 `rm -rf`；`cargo clean` 只清项目 `target/`，不是全局缓存。

### 5.4 Python

| 工具 | 动作 | 命令 | 安全 |
|---|---|---|---|
| pip | 列包 | `pip list --format=json` | 只读 |
| pip | 卸载 | `pip uninstall <pkg>` | Review（确认在 venv） |
| pip | 清缓存 | `pip cache purge` | Safe |
| pip | 查过期 | `pip list --outdated` | 只读 |
| conda | 列环境 | `conda env list` | 只读 |
| conda | 删环境 | `conda env remove -n <name>` | Review |
| conda | 清缓存 | `conda clean --all` | Safe |
| poetry | 安装/卸载 | `poetry install` / `poetry remove <pkg>` | Review |
| uv | 同步 | `uv sync` | Review |

注意：系统 Python 的包不要自动卸载；优先识别 venv/conda env，动作必须带明确环境。

### 5.5 Go / Ruby / PHP

| 工具 | 动作 | 命令 | 安全 |
|---|---|---|---|
| Go | 清构建缓存 | `go clean -cache` | Safe |
| Go | 清 test 缓存 | `go clean -testcache` | Safe |
| Go | 清模块缓存 | `go clean -modcache` | Review（需重新下载） |
| Go | 整理依赖 | `go mod tidy` | Review（改 go.mod/go.sum） |
| RubyGems | 清旧版本 | `gem cleanup` | Review |
| RubyGems | 卸载 | `gem uninstall <name>` | Review |
| Composer | 清缓存 | `composer clear-cache` | Safe |
| Composer | 优化 autoload | `composer dump-autoload -o` | Review |
| Composer | 卸载 | `composer remove <pkg>` | Safe（项目内） |

### 5.6 系统包管理器

| 工具 | 清缓存 | 孤儿/旧包 | 卸载 | 权限 |
|---|---|---|---|---|
| apt | `apt clean` / `apt autoclean` | `apt autoremove` | `apt remove` / `apt purge` | admin |
| dnf | `dnf clean all` | `dnf autoremove` | `dnf remove` | admin |
| pacman | `pacman -Sc` | `pacman -Qtdq` + `pacman -Rns` | `pacman -Rns` | admin |
| flatpak | — | `flatpak uninstall --unused` | `flatpak uninstall` / `--delete-data` | user/admin |
| snap | — | 旧 revision 由 `refresh.retain` 控制 | `snap remove --purge` | admin |

注意：

- 系统包管理器**永远不自动执行**；只生成计划、显示影响、要求管理员确认；
- `autoremove` 可能移除用户仍需要的运行时/内核，默认 `Review`；
- 不要把 `apt purge`/`pacman -Rns` 放进“一键优化”。

### 5.7 Windows

| 工具 | 列包 | 卸载 | 清理/优化 |
|---|---|---|---|
| winget | `winget list` | `winget uninstall <id>` | `winget upgrade`、`winget source update` |
| scoop | `scoop list` | `scoop uninstall <app>` | `scoop cleanup *`、`scoop cache rm *` |
| Chocolatey | `choco list` | `choco uninstall <pkg>` | `choco upgrade all` |

注意：Windows 包管理器可能修改系统 PATH/服务；卸载/升级默认 `Review`，系统级需要管理员。

### 5.8 版本管理器

| 工具 | 列版本 | 卸载旧版本 | 清理 |
|---|---|---|---|
| rustup | `rustup toolchain list` | `rustup toolchain uninstall <name>` | `rustup update` |
| nvm | `nvm ls` | `nvm uninstall <version>` | `nvm cache clear` |
| pyenv | `pyenv versions` | `pyenv uninstall <version>` | `pyenv rehash` |

旧版本只有在“不是默认/没有被项目锁定”时才建议移除；`Review` 起步。

## 6. 命令模板与执行安全

### 6.1 只允许白名单可执行文件

`CommandTemplate.program` 只能来自内置能力目录，且必须解析为绝对路径（`which` 结果），避免 PATH 劫持。执行时：

- 不经过 shell；
- `args` 逐项校验（包名来自 inventory，路径来自项目上下文）；
- 设置超时、捕获 stdout/stderr、限制输出大小；
- 清除危险环境变量（`NODE_OPTIONS`、`PYTHONPATH` 等按需保留）；
- 记录退出码、耗时、释放空间。

### 6.2 作用域与权限

| 作用域 | 含义 | 默认 |
|---|---|---|
| Project | 只影响当前项目（cwd=项目根） | 可默认建议 |
| Global | 影响用户全局（`~/.cargo`、`~/.npm`、`brew`） | 需确认 |
| System | 影响系统/其他用户（apt、pacman、Program Files） | 只展示，管理员确认 |

### 6.3 破坏性分级

- `Safe`：只清缓存/旧下载/构建产物，可重建，不需要网络；
- `Review`：会改 lockfile、卸载包、移除旧版本、需要网络；
- `Danger`：系统包、全局运行时、`--zap`、`--delete-data`、删环境/驱动。

`Danger` 永远不自动执行，且必须在 UI 中逐项确认。

### 6.4 前置条件

```rust
pub enum Precondition {
    BinaryExists,
    VersionInRange(VersionRange),
    ManifestExists,
    LockfileMatches(PmId),
    NoRunningProcess,
    NotSystemPython,
    NotDefaultToolchain,
    EnoughDiskSpace,
    UserConfirmed,
    AdminGranted,
}
```

任何前置条件不满足 → 不执行，给出原因。

## 7. 互联网经验集成

### 7.1 官方文档与 `--help`

- 每个能力的 `version_range` 和参数以官方文档为准；
- 只对白名单工具运行 `--help`/`--version` 做**存在性/版本探测**，不解析自由文本作为命令来源；
- 文档更新通过人工/半自动的 catalog PR，不在运行时抓网页。

### 7.2 社区配方

- Homebrew Cask 的 `zap` 用于应用卸载（AppProfile），不是包管理器清理；
- 社区维护的“包管理器优化命令”列表只作为候选，必须经过：
  1. 可执行文件白名单；
  2. 参数模板校验；
  3. 版本范围；
  4. 破坏性分级；
  5. 人工 review。

### 7.3 版本化

每个能力带 `version_range`。例如：

- Yarn classic `yarn cache clean` vs Berry；
- npm 的 `cache clean --force` 在新版本中语义变化；
- pnpm 8/9 的 store 路径与 prune 行为；
- Cargo 的 `cargo cache` 是外部子命令，未安装时不可用。

版本超出范围 → 能力标记 `Stale`，UI 不推荐，等待 catalog 更新。

## 8. 沉淀：被动、主动、反馈

### 8.1 被动学习

每次 inventory：

- 记录工具是否存在、版本、路径；
- 记录项目上下文（manifest/lockfile/packageManager 字段）；
- 记录全局包/环境/旧 toolchain 清单；
- 对比上次：新增/消失的包、版本变化、缓存大小变化。

每次执行后：

- 记录 `action`、命令、退出码、耗时、释放字节、错误摘要；
- 成功 → 提升该 capability 的置信度；
- 失败 → 记录错误类别（命令不存在/版本不支持/权限/网络/锁冲突），标记 `Stale` 或 `NeedsReview`。

### 8.2 主动学习

- “扫描这个项目”：识别项目根、lockfile、包管理器版本，生成项目专属清理建议；
- “教我你的工具链”：用户选择工具/项目，Sift 记录偏好（例如“这个仓库用 pnpm，不要建议 npm”）；
- “试跑一次”：用户对某个只读命令（`outdated`、`list`）授权执行，Sift 记录输出结构（不存内容），用于改进建议；
- “这个建议不对”：反馈到 capability/项目上下文，降权。

### 8.3 反馈与知识生命周期

```
Proposed → Validated (命令成功) → Preferred (用户多次接受)
        ↘ Stale (版本不匹配/命令失败) → Re-probe → Validated
```

- 每个 capability 有 `success_count` / `failure_count` / `last_success_ms`；
- 连续失败 2 次 → 降级为 `Stale`，不再默认推荐；
- 用户手动执行成功 → 记录为 `Observed`，但不自动升级为 `Preferred`，需要再次确认；
- 跨项目只复用**工具级**能力，不复用项目级结论。

### 8.4 隐私

- 默认只存本地；
- 不存命令输出全文，只存退出码、耗时、字节数、错误类别；
- 包名/项目路径属于本地数据；社区贡献只上传归一化的能力模板，不上传项目路径；
- API Key/registry token 永不写入日志。

## 9. 与现有 `cleanup.rs` 的集成

当前 `cleanup.rs` 的 `plan_for_tool(tool, path)` 是静态匹配。建议演进为：

```rust
pub fn plan_for(
    kind: &CandidateKind,
    path: &str,
    pm: Option<&PackageManagerProfile>,
    project: Option<&ProjectContext>,
) -> CleanupPlan
```

查找顺序：

1. 项目上下文 + 包管理器能力（最具体）；
2. 全局包管理器能力；
3. 现有静态表（向后兼容）；
4. 兜底 `TrashItem`。

`CandidateKind::RebuildableCache { tool }` 可以继续携带工具名，但计划由 `PmKnowledge` 生成，而不是 `match tool` 硬编码。这样新增 pnpm 9、uv、bun、deno 等只需要加 catalog，不用改 match。

## 10. 存储、API、UI

### 存储

独立 SQLite `pm_profiles.db`（或与 `app_profiles.db` 同库不同表）：

```sql
CREATE TABLE pm_profiles (
  pm_id TEXT PRIMARY KEY,
  version TEXT,
  executable TEXT,
  detected INTEGER NOT NULL,
  updated_ms INTEGER NOT NULL
);

CREATE TABLE pm_capabilities (
  pm_id TEXT NOT NULL,
  action TEXT NOT NULL,
  scope TEXT NOT NULL,
  command_json TEXT NOT NULL,
  destructive TEXT NOT NULL,
  version_range TEXT NOT NULL,
  source TEXT NOT NULL,
  success_count INTEGER NOT NULL DEFAULT 0,
  failure_count INTEGER NOT NULL DEFAULT 0,
  last_success_ms INTEGER,
  PRIMARY KEY (pm_id, action, scope)
);

CREATE TABLE pm_projects (
  root TEXT PRIMARY KEY,
  pm_id TEXT NOT NULL,
  manifest TEXT NOT NULL,
  lockfile TEXT,
  pm_version TEXT,
  updated_ms INTEGER NOT NULL
);

CREATE TABLE pm_observations (
  id INTEGER PRIMARY KEY,
  pm_id TEXT NOT NULL,
  action TEXT NOT NULL,
  exit_code INTEGER,
  duration_ms INTEGER,
  freed_bytes INTEGER,
  error_kind TEXT,
  at_ms INTEGER NOT NULL
);
```

### Tauri commands

```rust
list_package_managers() -> Vec<PmSummary>
get_pm_profile(pm_id) -> PackageManagerProfile
detect_project(path) -> Option<ProjectContext>
plan_pm_action(pm_id, action, project) -> PmPlan
execute_pm_action(plan_id) -> PmReport
record_pm_feedback(pm_id, action, verdict) -> ()
```

### UI

- 设置/工具页面新增 “Package managers” 分区；
- 项目视图显示 “本项目使用 pnpm 9 / Cargo”；
- 清理建议里标注 `Project`/`Global`/`System`；
- 每个动作展示：命令、作用域、影响、是否需要网络、是否改 lockfile；
- 系统包管理器动作需要二次确认 + 管理员授权说明。

## 11. 分阶段落地

### Phase A：只读目录（1 周）

- 检测工具、版本、项目 lockfile；
- 展示全局包/环境/旧 toolchain；
- 不执行任何变更。

### Phase B：安全清理（1–2 周）

- `brew cleanup`、`npm cache verify`、`pnpm store prune`、`pip cache purge`、`cargo clean`、`go clean -cache`、`conda clean`、`gem cleanup`；
- 只在用户确认后执行；
- 记录结果。

### Phase C：项目级建议（2 周）

- 项目上下文 + lockfile 权威；
- `npm prune`、`pnpm dedupe`、`cargo update`、`go mod tidy` 等 Review 动作；
- 改 lockfile 前提示。

### Phase D：全局卸载/旧版本（2 周）

- 全局包、旧 toolchain、旧 runtime；
- 依赖检测（哪些项目/命令可能受影响）；
- 默认 Review，不自动。

### Phase E：系统包管理器（按需）

- apt/dnf/pacman/winget/choco：只生成计划，管理员确认；
- 不做“一键优化”。

### Phase F：沉淀与社区目录

- 命令结果反馈、版本失效检测；
- 公共 capability catalog（人工审核）；
- 跨平台覆盖。

## 12. 测试与指标

### 测试

- 假包管理器可执行文件（fixture）注入 PATH，验证检测/版本/超时；
- fixture 项目：`package-lock`、`pnpm-lock`、`yarn.lock`、`Cargo.lock`、`poetry.lock`、`go.mod` 等，验证 lockfile 权威；
- 命令渲染 golden test：断言没有 shell、参数顺序/引号正确；
- 前置条件：缺二进制、版本不匹配、系统 Python、默认 toolchain、权限不足；
- 失败分类：命令不存在、非零退出、网络失败、锁冲突；
- 结果记录：释放字节、耗时、错误摘要；
- 安全：恶意包名/参数、PATH 劫持、符号链接、`--zap` 不得由优化触发。

### 指标

- 检测覆盖率（Top 工具）；
- 命令成功率；
- 用户接受率 / 撤销率；
- 单次释放空间；
- 版本失效导致的 `Stale` 比例；
- 因建议导致的构建失败/回滚（目标为 0）。

## 13. 风险与开放问题

| 风险 | 缓解 |
|---|---|
| 命令在不同版本语义变化 | `version_range` + 失败即 `Stale`，人工 catalog review |
| 全局卸载破坏项目 | 项目依赖检测 + 默认 `Review` + 撤销/重装提示 |
| 手工删 store 破坏硬链接 | 只用工具命令，禁止直接删 store |
| 系统包管理器误删 | 永不自动，管理员确认，`Danger` 分级 |
| 运行 install 触发脚本 | 归为变更动作，明确提示“会执行 postinstall” |
| 网络/代理失败 | 超时、错误分类、可重试，不标记工具损坏 |
| 包管理器版本管理器互相嵌套 | 记录 `toolchain → pm` 关系，先解析项目版本 |
| 输出解析脆弱 | 优先 `--json`/`--format=json`，失败则降级为“只读展示” |

**开放问题**

1. 是否把 `uv`、`bun`、`deno`、`mise` 纳入第一版 catalog？建议先做 Top 10，再按用户反馈扩展。
2. 全局包卸载后的“重装命令”是否自动生成？建议生成但需确认。
3. 是否允许用户自定义命令模板？建议只允许从 catalog 选择 + 参数填空，不允许自由 shell。
4. 系统包管理器是否值得做？建议先做只读 inventory + 计划，不做执行。

## 14. 结论

包管理器知识应该和 AppProfile 一样，作为**可沉淀的数据**存在：

- **检测**：工具、版本、项目 lockfile；
- **能力**：安装/卸载/清理/优化/诊断的命令模板；
- **安全**：作用域、管理员、破坏性、前置条件；
- **沉淀**：执行结果、版本失效、用户偏好；
- **复用**：项目级优先于全局，lockfile 优先于默认。

这样 Sift 对“包管理器”的认识会随使用越来越准，而不是停留在 `match tool { "cargo" => ... }` 的静态表。

## 15. 相关设计

- [扫描时环境上下文](scan-context.md)——每次扫描如何刷新 PM/项目上下文、做增量模型判定并沉淀维护建议。
