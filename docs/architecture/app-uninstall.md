# 应用卸载与数据沉淀：AppProfile 深度清理方案

> 目标：用户卸载一个应用时，Sift 能给出**完整、可解释、可撤销**的清理计划；并且这套知识会随安装、使用、卸载和用户反馈不断沉淀，覆盖长尾应用，而不是只维护一张静态路径表。

## 1. 问题定义

“卸载应用”远不止删除 `.app` / `Program Files` 目录。一个应用的数据通常散落在：

| 类别 | macOS 示例 | Windows 示例 | Linux 示例 |
|---|---|---|---|
| 应用本体 | `/Applications/X.app` | `C:\Program Files\X` | `/usr/share/x`、AppImage |
| 用户数据 | `~/Library/Application Support/X` | `%APPDATA%\X` | `~/.local/share/x` |
| 缓存 | `~/Library/Caches/<bundle-id>` | `%LOCALAPPDATA%\X\Cache` | `~/.cache/x` |
| 偏好 | `~/Library/Preferences/<bundle-id>.plist` | `HKCU\Software\X` | `~/.config/x` |
| 沙盒容器 | `~/Library/Containers/<bundle-id>` | `%LOCALAPPDATA%\Packages\<PFN>` | Flatpak `~/.var/app/<id>` |
| 共享容器 | `~/Library/Group Containers/<group>` | 共享包 | 共享运行时 |
| 后台项 | LaunchAgents、Login Items | 服务、计划任务、启动项 | systemd user units、autostart |
| 日志/崩溃 | `~/Library/Logs/X` | `%LOCALAPPDATA%\X\Logs` | `~/.local/state/x`、journal |
| 密钥/授权 | Keychain、TCC | 凭据管理器、注册表 | secret service、polkit |
| 收据/安装器 | `pkgutil` receipts | 注册表 Uninstall 键 | dpkg/rpm 数据库 |

难点不在“知道常见路径”，而在：

1. **身份**：目录名不等于 bundle id，改名、旧版本、厂商目录、共享容器都会误导；
2. **归属**：一个路径可能被多个应用共享，删错会破坏另一个应用；
3. **安全**：用户数据、密钥、系统扩展不能靠启发式删除；
4. **长尾**：不可能为每个应用手写规则，需要从安装/使用/卸载过程中学习；
5. **可撤销**：卸载是不可逆操作，必须有计划、预览、审计和恢复。

## 2. 设计原则

1. **身份先于路径**：先解析出稳定的 `AppIdentity`，再用身份去发现路径；不按目录名猜。
2. **证据分级**：每条路径都带来源、权重、时间；结论可解释，不输出“黑盒建议”。
3. **结构与内容分离**：缓存/日志的“结构性安全”由确定性规则判断；用户数据的“内容性安全”才需要模型或用户确认。
4. **共享物不自动删**：检测到另一个已安装应用也引用该路径时，降级为 `Review`/`Keep`。
5. **默认不自动删用户数据**：自动只覆盖缓存、日志、保存状态等可重建类别；其余默认勾选但需用户确认。
6. **先隔离、后清理**：删除先进入可恢复的隔离区/回收站，记录 manifest，支持撤销。
7. **本地优先**：配方和画像默认只存本地；社区共享只上传归一化后的路径模式，且显式同意。
8. **模型只解释、不授权**：模型可以给原因、判断类别，但不能单独把某个路径变成可自动删除。

## 3. 总体架构

```
┌─────────────────────────────────────────────────────────────┐
│                        UI: Applications                      │
│  应用列表 / 卸载计划 / 逐项确认 / 恢复 / 教会 Sift / 反馈     │
└───────────────┬─────────────────────────────────────────────┘
                │ Tauri commands
┌───────────────▼─────────────────────────────────────────────┐
│                      sift-apps (new crate)                   │
│                                                             │
│  Inventory ──► Identity ──► Discovery ──► Planner ──► Exec   │
│      │            │            │             │          │    │
│      │            │            │             │          │    │
│  Recipes ◄──── Knowledge ◄── Observations ◄── Feedback   │    │
│      ▲            ▲            ▲                         │    │
│      │            │            │                         │    │
│  ingest tool   AppProfile DB   monitor / diff            │    │
└──────┬─────────────┬─────────────┬─────────────┬────────────┘
       │             │             │             │
  Homebrew zap   sift-store     sift-monitor   sift-platform
  vendor docs    (SQLite)       (processes)    (trash/quarantine)
  XDG/registry
```

新增一个 `sift-apps` crate，避免把应用知识塞进扫描/分析 crate：

- 依赖 `sift-core`（路径安全、`NodeKey`）、`sift-platform`（trash、进程、卷）、`sift-store`（SQLite）、`sift-analyze`（`Safety`、`Reason`、`Verdict` 词汇）。
- 不依赖 `sift-scan`，应用清单是独立的、按需的轻量枚举，不需要全盘扫描。

## 4. 核心数据模型

```rust
pub struct AppKey(String); // 稳定哈希：platform + 主身份

pub struct AppIdentity {
    pub key: AppKey,
    pub platform: Platform,
    pub bundle_id: Option<String>,      // macOS / iOS
    pub package_family: Option<String>, // MSIX / Store
    pub flatpak_id: Option<String>,
    pub snap_name: Option<String>,
    pub name: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub team_id: Option<String>,        // macOS 代码签名
    pub install_paths: Vec<PathBuf>,
    pub aliases: Vec<String>,           // 旧 bundle id、改名、可执行名
    pub source: IdentitySource,         // Bundle/Receipt/Registry/DesktopFile/User
}

pub struct Artifact {
    pub path: PathBuf,
    pub kind: ArtifactKind,
    pub scope: ArtifactScope,           // User / System / Shared / Unknown
    pub disposition: Disposition,       // AutoSafe / Review / Keep / Forbidden
    pub confidence: f32,                // 0..1
    pub evidence: Vec<Evidence>,
    pub shared_with: Vec<AppKey>,
    pub requires_admin: bool,
    pub selected_by_default: bool,
    pub first_seen_ms: i64,
    pub last_seen_ms: i64,
    pub size_bytes: Option<u64>,
}

pub struct AppProfile {
    pub identity: AppIdentity,
    pub artifacts: Vec<Artifact>,
    pub status: ProfileStatus,          // Provisional / Confirmed / Archived
    pub observations: u32,
    pub confirmations: u32,
    pub false_positives: u32,
    pub updated_ms: i64,
}

pub struct Evidence {
    pub source: EvidenceSource,         // Recipe/Convention/InstallDiff/UsageDiff/User/Feedback/Audit
    pub rule: String,                   // 稳定的规则 id
    pub weight: f32,                    // 可正可负
    pub detail: Option<String>,
    pub at_ms: i64,
}
```

`ArtifactKind` 至少覆盖：

`Bundle`, `Support`, `Cache`, `Preferences`, `Container`, `GroupContainer`,
`SavedState`, `Log`, `CrashReport`, `HttpStorage`, `WebKit`, `Cookie`,
`LaunchAgent`, `LoginItem`, `Service`, `Driver`, `KeychainRef`, `Tcc`,
`Receipt`, `Installer`, `RegistryKey`, `StartMenu`, `ScheduledTask`,
`FlatpakData`, `SnapData`, `SharedRuntime`, `Other`.

### 4.1 处置默认值

| Kind | 默认处置 | 说明 |
|---|---|---|
| Cache | `AutoSafe` | 可重建；应用自己的缓存 |
| Log / CrashReport | `AutoSafe` | 可重建，保留排障价值已降低 |
| SavedState | `AutoSafe` | 窗口恢复状态 |
| HttpStorage / WebKit | `Review` | 可能含登录态 |
| Preferences | `Review` | 卸载时默认勾选，但提示“设置会丢失” |
| Support | `Review` | 可能含用户数据/草稿 |
| Container | `Review` | 沙盒数据，可能含用户内容 |
| GroupContainer | `Keep`/`Review` | 与其他应用共享时 `Keep` |
| LaunchAgent / LoginItem / Service | `Review` | 需要先卸载服务，删除可撤销 |
| KeychainRef | `Forbidden` | 不自动删，只给手动指引 |
| Tcc | `Forbidden` | 不直接改系统数据库 |
| Driver / SystemExtension | `Forbidden` | 交给厂商卸载器，通常需重启 |
| Bundle | `Review` | 卸载对象本身，必须显式确认 |
| SharedRuntime | `Keep` | 其他应用可能依赖 |

## 5. 应用身份解析

### 5.1 清单来源

**macOS**

- `/Applications`、`~/Applications` 的 `.app`；
- `mdfind "kMDItemContentType == 'com.apple.application-bundle'"`（Spotlight 可用时）；
- `Info.plist`：`CFBundleIdentifier`、`CFBundleName`、`CFBundleDisplayName`、`CFBundleShortVersionString`、`CFBundleExecutable`；
- 代码签名：`SecCodeCopySigningInformation` 或 `codesign -dvvv`，取 `TeamIdentifier`、`Identifier`、`Authority`；
- 安装来源：`pkgutil --pkgs`、Homebrew Cask 清单、`system_profiler SPApplicationsDataType`（作为兜底）。

**Windows**

- 注册表卸载键：`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\*`、`HKLM\...\Uninstall\*`、`HKLM\...\WOW6432Node\...`；
- 读取 `DisplayName`、`DisplayVersion`、`Publisher`、`InstallLocation`、`UninstallString`、`QuietUninstallString`；
- MSIX：`Get-AppxPackage` → `PackageFamilyName`、`InstallLocation`；
- `winget list --output json` 作为补充；
- 可执行文件版本信息（`FileVersionInfo`）补全 vendor/name。

**Linux**

- `.desktop` 文件：`/usr/share/applications`、`~/.local/share/applications`，读 `Name`、`Exec`、`Icon`、`StartupWMClass`、`X-Flatpak`；
- `dpkg-query -W` / `rpm -qa`；
- `flatpak list --columns=application,name,version`；
- `snap list`。

### 5.2 稳定 key 与别名

优先级：

1. macOS `bundle_id`；
2. Windows `PackageFamilyName`（MSIX）或“注册表 publisher + name”的稳定哈希；
3. Linux `flatpak_id` / `snap_name` / `.desktop` 的 `StartupWMClass`；
4. 兜底：`platform + install_path + name` 的哈希。

`aliases` 用于吸收改名和旧版本：旧 bundle id、`CFBundleName`、可执行文件名、厂商目录名。配方可以显式声明 `aliases = ["com.old.name"]`。

**代码签名校验**：macOS 上如果签名与 bundle id 不一致，或应用未签名，降低 `IdentitySource` 的可信度，并在计划里标注“身份未验证”。

## 6. 残留发现：四层证据

### 第 1 层：内置配方（互联网经验）

这是冷启动覆盖率的关键。首选 **Homebrew Cask 的 `zap` 配置**：

- 通过 `brew info --cask <name> --json=v2` 或官方 `cask.json` 获取；
- `artifacts` 给出 `app`、`pkg`、`binary`、`font`、`manpage` 等安装物；
- `zap` 给出 `trash` 路径、`rmdir`、`login_item`、`launchctl`、`pkgutil`、`kext`、`delete` 等；
- Homebrew 使用 BSD-2-Clause，适合作为可再分发的配方来源（仍需在 NOTICE 中保留归属）。

补充来源：

- 厂商官方卸载文档（Adobe、Microsoft、Docker、Zoom 等）；
- 发行版包管理器语义：`apt purge`、`flatpak uninstall --delete-data`、`snap remove --purge`；
- Windows 的 `winget` manifest + 注册表卸载信息；
- 社区卸载配置/脚本：逐项核对许可证与质量，不直接整库导入。

配方统一为：

```toml
[recipe]
app = "com.example.app"
name = "Example"
platform = "macos"
source = "homebrew-cask"
license = "BSD-2-Clause"
retrieved_at = "2026-11-10"
version_range = ">=1.0"

[[artifact]]
path = "~/Library/Application Support/Example"
kind = "support"
scope = "user"
confidence = 0.9

[[artifact]]
path = "~/Library/Caches/com.example.app"
kind = "cache"
scope = "user"
confidence = 0.95
```

变量：`$HOME`、`$APPDATA`、`$LOCALAPPDATA`、`$XDG_CONFIG_HOME`、`{bundle_id}`、`{app_name}`、`{vendor}`。

**配方准入校验**（编译期/CI）：

- 展开后必须是绝对路径；
- 拒绝 `/`、home 根、`/System`、`/usr`、`/bin`、`/etc`、`/Library`（除明确缓存）、`C:\Windows`、`Program Files` 根；
- 拒绝 `.app`/`.framework` 内部路径（除非该配方就是“应用本体”）；
- 拒绝 Keychain、TCC、系统扩展；
- 每条路径必须带 `kind`/`scope`/`confidence`；
- 去重、排序、生成内容哈希，便于审计。

### 第 2 层：平台约定（无配方也能覆盖）

按 `ArtifactKind` 生成候选路径，用身份 token 展开：

**macOS**

```
~/Library/Application Support/{bundle_id}
~/Library/Application Support/{name}
~/Library/Application Support/{vendor}/{name}
~/Library/Caches/{bundle_id}
~/Library/Caches/{name}
~/Library/Preferences/{bundle_id}.plist
~/Library/Containers/{bundle_id}
~/Library/Group Containers/{group}
~/Library/Saved Application State/{bundle_id}.savedState
~/Library/Logs/{name}
~/Library/Logs/DiagnosticReports/{name}*
~/Library/HTTPStorages/{bundle_id}
~/Library/WebKit/{bundle_id}
~/Library/Application Scripts/{bundle_id}
~/Library/LaunchAgents/{label}.plist
```

**Windows**

```
%APPDATA%\{vendor}\{name}
%APPDATA%\{name}
%LOCALAPPDATA%\{vendor}\{name}
%LOCALAPPDATA%\{name}
%LOCALAPPDATA%\Packages\{package_family}
%PROGRAMDATA%\{vendor}\{name}
HKCU\Software\{vendor}\{name}
HKLM\Software\{vendor}\{name}
```

**Linux**

```
~/.config/{name}
~/.local/share/{name}
~/.local/state/{name}
~/.cache/{name}
~/.var/app/{flatpak_id}
~/snap/{snap_name}
~/.config/systemd/user/{name}*.service
~/.config/autostart/{name}.desktop
```

约定只提供**候选**，不直接授权删除；最终处置仍由证据和共享检测决定。

### 第 3 层：安装/使用观测（沉淀的核心）

见第 7、8 节。它把“约定猜出来的路径”变成“这个应用确实写过/创建过的路径”。

### 第 4 层：用户显式知识

- 用户在计划里勾选/取消；
- 用户拖入一个目录；
- 用户反馈“漏了某路径”；
- 用户确认“这个应用我卸载后不再需要这些数据”。

用户显式知识权重最高，但只作用于该应用/该路径，不自动泛化到其他应用。

## 7. 使用中沉淀：被动学习

被动学习不要求用户做额外操作，但也不假装能做到内核级进程归因。分三档：

### 7.1 安装窗口 diff（推荐首先实现）

触发：

- 监听安装目录：`/Applications`、`~/Applications`、`%LOCALAPPDATA%\Programs`、`/usr/share/applications`、Flatpak/Snap 目录；
- 发现新应用身份后，记录 `installed_at`；
- 在安装前（如果 Sift 正在运行）或首次发现时，对**限定根目录**做元数据快照；
- 24 小时后（或用户点击“完成学习”）再做一次 diff。

快照范围只包含候选根，不做全盘：

- macOS：`~/Library/{Application Support,Caches,Preferences,Containers,Group Containers,Saved Application State,Logs,HTTPStorages,WebKit,Application Scripts,LaunchAgents}`；
- Windows：`%APPDATA%`、`%LOCALAPPDATA%`、`%PROGRAMDATA%`、注册表卸载键；
- Linux：`~/.config`、`~/.local/share`、`~/.local/state`、`~/.cache`、`~/.var/app`、`~/snap`。

diff 得到的新路径，用身份 token 过滤后作为 `InstallDiff` 证据。注意：

- 只存路径 + 大小 + mtime，不读内容；
- 单次快照上限（如 20 万条），超出则退化为只扫名称匹配的候选；
- 安装器可能创建共享运行时，必须做共享检测。

### 7.2 使用窗口 diff

- 进程监控发现应用启动（`sift-monitor` 已有进程采样）；
- 对应用**已知候选根**做轻量快照；
- 应用退出后 diff；新增/修改的路径记为 `UsageDiff` 证据；
- 沙盒应用（macOS Container、MSIX）可以直接用容器路径，归因最强；
- 非沙盒应用只能做“时间窗口 + 路径约定”归因，权重低于安装 diff。

**能力边界（必须诚实）**：

- macOS 的 FSEvents 只给路径，不给 PID；`EndpointSecurity` 需要 Apple entitlement；`eslogger` 需要 root；
- Windows 的 ETW 文件事件需要管理员；`ReadDirectoryChangesW` 不给 PID；
- Linux 的 fanotify/eBPF 需要 root/CAP_SYS_ADMIN。

因此 v1 不做内核级逐进程归因，只做**限定根 + 时间窗口 + 约定**。后续如果需要“高级学习模式”，再提供一个显式授权的特权 helper，作为可选能力，不阻塞主流程。

### 7.3 卸载演练（最强沉淀信号）

当应用从安装位置消失，或用户点击“卸载”：

1. 记录卸载前已知的 artifact 集合；
2. 运行厂商卸载器（如果有）；
3. 对比卸载后哪些路径消失、哪些残留；
4. 询问用户：“这些残留要一起清理吗？哪些应该保留？”
5. 把结果写成 `UninstallRehearsal` 观测：
   - 厂商卸载器删掉的 → 确认归属；
   - 残留且用户清理的 → 高置信归属；
   - 残留且用户保留的 → 标记 `Keep` 或 `Review`，不要下次又推荐。

这是“越用越准”的主要来源，优先级高于安装/使用 diff。

### 7.4 周期性审计

- 每周对已知 `AppProfile` 做一次轻量 stat；
- 更新 `last_seen_ms`、大小、是否存在；
- 应用卸载 30 天后，profile 进入 `Archived`，但保留用于“重装检测”和“残留清理”；
- 路径消失则标记 `missing`，不删除证据（保留历史，便于解释）。

## 8. 使用中沉淀：主动学习

### 8.1 “教会 Sift 这个应用”

用户选择一个应用 → Sift 展示当前候选 → 用户启动应用、正常使用几分钟 → 退出 → Sift diff → 预览新增路径 → 用户确认。适用于没有配方、又不想等安装窗口的长尾应用。

### 8.2 “监视下一次安装”

用户点击“监视安装” → Sift 对候选根做快照 → 用户安装应用 → Sift 检测新应用并 diff → 用户确认。适合新装应用，一次学习即可长期复用。

### 8.3 “卸载后清理残留”

用户把应用拖进废纸篓/运行系统卸载器后，Sift 检测到应用消失，主动弹出“发现残留”，列出路径和大小。用户确认后清理并沉淀。

### 8.4 反馈闭环

计划执行前/后都允许：

- 取消勾选 → 该路径降权；
- “漏了某个目录” → 加入 profile，权重最高；
- “这个不该删” → 标记 false positive，降权并加入 `Keep`；
- 一键“这次计划很准” → 提升该应用 profile 的置信度。

反馈只做**逐应用、逐路径**的计数，不训练全局模型；全局权重由离线评估校准。

### 8.5 配方导出与社区贡献（可选）

用户可以把一个已确认的 profile 导出为归一化配方：

- 只保留 `{bundle_id}`、`{app_name}`、`$HOME` 等模式；
- 去掉用户名、真实路径、文件内容；
- 用户显式选择上传/提交 PR；
- 服务端/仓库审核后再进入公共配方库。

默认不上传，避免隐私和投毒风险。

## 9. 评分与处置

### 9.1 证据权重（初始值，可校准）

| 证据 | 权重 |
|---|---:|
| 配方精确匹配 | +50 |
| 配方 glob 匹配 | +40 |
| bundle id / package family 出现在路径中 | +35 |
| 应用名/vendor token 匹配 | +20 |
| 平台约定路径 + 正确 kind | +15 |
| 安装窗口内创建 | +30 |
| 使用窗口内写入 | +25 |
| 沙盒容器与 bundle id 完全一致 | +60 |
| launchd label / 服务名匹配 | +45 |
| 用户显式加入 | +80 |
| 与另一个已安装应用共享 | -50 |
| 用户数据标记（Documents/Photos/…） | -60 |
| 正在运行/被打开 | -30 |
| 用户反馈“不该删” | -80 |
| 用户反馈“漏了” | +60 |

> 这些权重是排序工具，不是删除授权。最终处置还要过“结构安全规则”。

### 9.2 处置阈值

- `score >= 70` 且 kind 在自动安全白名单 → `AutoSafe`；
- `40 <= score < 70` → `Review`；
- `score < 40` → 不展示，或仅作为“可能残留”折叠；
- 任何共享/用户数据/密钥/系统扩展 → 至少 `Review`，必要时 `Keep`/`Forbidden`。

自动安全白名单只包含：`Cache`、`Log`、`CrashReport`、`SavedState`，并且必须满足：

- 路径在用户域（`$HOME`/`%LOCALAPPDATA%`/`~/.cache`）；
- 不属于其他已安装应用；
- 不在应用 bundle 内；
- 不是挂载点/符号链接目标；
- 不是用户显式标记保留的路径。

### 9.3 共享检测

共享是卸载清理最大的误删来源。检测方法：

1. 其他已安装应用的 `AppProfile.artifacts` 是否包含同一路径/祖先/后代；
2. macOS `Group Containers` 的 group id 是否被多个 app 的 entitlement 引用；
3. vendor 目录（如 `~/Library/Application Support/Adobe`）是否被多个兄弟应用使用；
4. Windows `%PROGRAMDATA%`、共享运行库、注册表公共键；
5. Linux 共享 `~/.local/share` 下的公共目录。

命中共享 → `shared_with` 非空 → `Review`/`Keep`，并在 UI 明确写出“另一个应用也在用”。

## 10. 卸载执行与撤销

### 10.1 计划

`UninstallPlan` 包含：

- 应用身份、版本、安装路径、是否运行；
- 按 `ArtifactKind` 分组的路径、大小、处置、警告；
- 默认选中项（仅 `AutoSafe`）；
- 需要管理员权限的项；
- 共享/用户数据警告；
- 预计释放空间；
- 厂商卸载器信息（如有）。

UI 必须展示“会丢失什么”，而不是只展示总大小。

### 10.2 执行顺序

1. 请求退出应用（优雅退出，超时后再询问强制退出）；
2. 卸载 LaunchAgent/LoginItem/Service/计划任务（记录以便恢复）；
3. 运行厂商卸载器（如用户同意）；
4. 移动用户数据/缓存/日志到隔离区或回收站；
5. 最后移动应用本体；
6. 写 manifest，校验路径已消失；
7. 失败项逐条报告，不因单个失败中断全部。

### 10.3 隔离区与撤销

优先使用 **Sift 管理的隔离区**，而不是直接进系统废纸篓：

```
~/Library/Application Support/Sift/Quarantine/<manifest-id>/
  manifest.json
  payload/<原路径的镜像>
```

- 同卷用 `rename`，跨卷回退到 copy + 校验；
- 默认保留 7 天，之后提示用户“移入废纸篓/彻底删除”；
- “撤销卸载”按 manifest 逆序移回；
- 如果用户坚持直接进系统废纸篓，也写 manifest，撤销时按原路径找回。

这样既满足“深度清理”，又给出可验证的恢复路径。

### 10.4 权限

- 用户域路径：普通权限；
- `/Library`、`%PROGRAMDATA%`、系统服务：需要管理员；
- v1 可以只做用户域，系统域列在计划里但标记“需要管理员，暂不自动执行”；
- 未来用签名 helper + 一次性授权，不把管理员密码传给主进程。

## 11. 存储与 schema

独立 SQLite：`app_profiles.db`，与扫描 journal 分开，避免热路径耦合。

```sql
CREATE TABLE apps (
  app_key TEXT PRIMARY KEY,
  platform TEXT NOT NULL,
  bundle_id TEXT,
  package_family TEXT,
  name TEXT NOT NULL,
  vendor TEXT,
  version TEXT,
  team_id TEXT,
  install_path TEXT,
  status TEXT NOT NULL, -- active | missing | archived
  installed_at_ms INTEGER,
  updated_ms INTEGER NOT NULL
);

CREATE TABLE artifacts (
  id INTEGER PRIMARY KEY,
  app_key TEXT NOT NULL REFERENCES apps(app_key),
  path TEXT NOT NULL,
  kind TEXT NOT NULL,
  scope TEXT NOT NULL,
  disposition TEXT NOT NULL,
  confidence REAL NOT NULL,
  size_bytes INTEGER,
  first_seen_ms INTEGER NOT NULL,
  last_seen_ms INTEGER NOT NULL,
  missing INTEGER NOT NULL DEFAULT 0,
  requires_admin INTEGER NOT NULL DEFAULT 0,
  UNIQUE(app_key, path)
);

CREATE TABLE evidence (
  id INTEGER PRIMARY KEY,
  artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
  source TEXT NOT NULL,
  rule TEXT NOT NULL,
  weight REAL NOT NULL,
  detail TEXT,
  at_ms INTEGER NOT NULL
);

CREATE TABLE observations (
  id INTEGER PRIMARY KEY,
  app_key TEXT NOT NULL,
  kind TEXT NOT NULL, -- install_diff | usage_diff | rehearsal | audit | user
  path TEXT,
  at_ms INTEGER NOT NULL,
  payload TEXT
);

CREATE TABLE feedback (
  id INTEGER PRIMARY KEY,
  app_key TEXT NOT NULL,
  path TEXT NOT NULL,
  verdict TEXT NOT NULL, -- correct | false_positive | missed
  at_ms INTEGER NOT NULL
);

CREATE TABLE manifests (
  id TEXT PRIMARY KEY,
  app_key TEXT NOT NULL,
  created_at_ms INTEGER NOT NULL,
  restored_at_ms INTEGER,
  payload TEXT NOT NULL
);
```

约束：

- `SCHEMA_VERSION` + 迁移，风格与 `journal_sqlite.rs` 一致；
- 每个 app 的 observations 上限（如 5000 条），LRU/时间裁剪；
- artifacts 随 profile 归档而压缩；
- 所有路径以规范化的绝对路径存储，另存展示用脱敏路径。

## 12. API 与 UI

### Tauri commands（新增 `src-tauri/src/apps.rs`）

```rust
list_installed_apps() -> Vec<AppSummary>
get_app_profile(app_key) -> AppProfile
discover_app_artifacts(app_key) -> Vec<Artifact>
plan_uninstall(app_key, options) -> UninstallPlan
execute_uninstall(plan_id, selected_artifact_ids) -> UninstallReport
restore_uninstall(manifest_id) -> RestoreReport
start_learning(app_key, mode) -> LearningSession
finish_learning(session_id) -> LearningResult
record_app_feedback(app_key, path, verdict) -> ()
```

### UI（新增 Applications 视图）

- 应用列表：图标、名称、版本、安装位置、占用、最近使用、是否有 profile；
- 详情：按类别分组的路径、大小、置信度、证据来源、共享标记；
- 卸载计划：默认勾选自动安全项，`Review` 项默认不勾选但可见，`Keep/Forbidden` 置灰并解释；
- “教会 Sift”/“监视安装”/“清理残留”入口；
- 执行进度、失败项、撤销入口；
- 反馈按钮：准确 / 漏了 / 不该删。

## 13. 安全、隐私与权限

### 13.1 路径安全

复用/扩展现有 `sift-core::deletable`：

- 拒绝 `/`、home 根、系统目录、挂载点、bundle 内部；
- `canonicalize` + 符号链接解析 + 挂载边界检查；
- 删除前重新 stat，fingerprint 变化则中止（TOCTOU）；
- 拒绝 `..`、相对路径、非法配方路径；
- 系统域路径必须显式授权。

### 13.2 身份安全

- macOS 优先使用代码签名身份；签名与 bundle id 不一致时降级；
- Windows 校验注册表 publisher 与安装路径的一致性；
- 防止“同名目录”冒充：只有 bundle id/package family 或配方明确匹配才给高权重。

### 13.3 隐私

- 默认只存本地；
- 不读取用户文件内容；
- 社区配方只包含归一化路径模式，不含用户名/真实路径；
- 模型只接收脱敏路径/类别/大小/年龄，沿用现有 `sift-analyze` 的隐私边界；
- 遥测默认关闭，即使开启也只上报计数和脱敏规则 id。

### 13.4 模型边界

- 模型可以：解释某个残留是什么、建议 `review/keep`、指出风险；
- 模型不可以：授权 `AutoSafe`、生成 shell 命令、决定共享路径；
- 沿用 `sanitize_model_verdict` 的降级逻辑。

## 14. 分阶段落地

### Phase 0：语料与基线（1 周）

- 抓取 Homebrew Cask `zap` 语料，统计覆盖率；
- 建立 20 个常用应用的 fixture home 目录；
- 定义 recipe schema、路径安全测试。

### Phase 1：只读计划（2 周）

- `sift-apps`：清单、身份、配方加载、平台约定；
- `plan_uninstall` + UI 预览；
- 不执行删除，先验证“找得全、标得准”。

### Phase 2：执行与撤销（2 周）

- 隔离区 + manifest + 恢复；
- 只自动勾选 `Cache/Log/CrashReport/SavedState`；
- 厂商卸载器调用（用户确认）；
- 失败项报告、权限提示。

### Phase 3：被动沉淀（3 周）

- 安装窗口 diff、使用窗口 diff、卸载演练；
- `app_profiles.db` 持久化；
- 周期性审计；
- 共享检测。

### Phase 4：主动学习与反馈（2 周）

- “教会 Sift”“监视安装”“清理残留”；
- 反馈闭环与置信度调整；
- 配方导出。

### Phase 5：社区与跨平台（持续）

- 配方审核/同步；
- Windows 注册表/服务、Linux flatpak/snap 的深度覆盖；
- 可选特权 helper（高级学习模式）。

## 15. 评估与测试

### 15.1 指标

- **自动安全精确率**：目标 100%（以撤销/投诉计），宁可少删；
- **配方覆盖率**：Top 200 应用中带 recipe/convention 的比例；
- **残留召回率**：在 VM 中安装/使用/卸载后，人工标注的残留被计划覆盖的比例；
- **撤销率**、**误报率**、**单应用计划耗时**、**平均释放空间**。

### 15.2 测试

- fixture home + 合成 `.app` / 注册表导出 / `.desktop`；
- 配方路径安全属性测试（拒绝系统路径、路径穿越、home 根）；
- 共享检测：两个应用引用同一路径；
- 撤销：移动后恢复，校验内容/权限；
- 权限失败、路径消失、符号链接替换（TOCTOU）；
- 跨平台路径归一化（`/` vs `\`、大小写、Unicode 规范化）；
- 恶意配方：`../../`、绝对路径、Keychain、`/System`。

### 15.3 红队

- 伪造 bundle id 的应用；
- 配方投毒（把用户目录写成可删）；
- 共享容器诱导删除；
- 卸载器要求管理员权限时的提权滥用；
- 路径竞态替换。

## 16. 与现有 Sift 的集成点

| 现有模块 | 复用方式 |
|---|---|
| `sift-core::deletable` | 路径分类、bundle 内部判断、权限检查 |
| `sift-platform::trash` | 删除执行；新增 quarantine 模块或复用 |
| `sift-monitor` | 应用运行状态、进程采样 |
| `sift-store` | SQLite/WAL/迁移风格；`app_profiles.db` |
| `sift-analyze` | `Safety`/`Reason`/`Verdict`、模型护栏；可选让模型解释残留 |
| `sift-scan` | 不需要全盘扫描；应用清单按需枚举 |
| `src-tauri` | 新 `apps.rs` commands、事件、权限 |
| `src/lib` | 新 Applications 视图、i18n、计划弹窗 |

建议新 crate 命名 `sift-apps`，职责单一：

```
sift-apps/
  src/
    identity.rs      // AppIdentity, inventory
    recipe.rs        // recipe schema, load/validate
    discovery.rs     // convention + recipe matching
    profile.rs       // AppProfile, scoring
    planner.rs       // UninstallPlan
    executor.rs      // quarantine/trash, manifest, undo
    learning.rs      // install/usage diff, rehearsal
    shared.rs        // shared artifact detection
    db.rs            // SQLite
```

## 17. 风险与开放问题

| 风险 | 缓解 |
|---|---|
| 配方质量参差/过期 | 版本范围、CI 路径校验、用户反馈、按应用灰度 |
| 配方许可证 | 只选可再分发来源（Homebrew BSD-2），保留 NOTICE；社区提交需 CLA/许可 |
| 共享路径误删 | 共享检测 + 默认 `Review` + 明确警告 |
| 权限/系统域 | v1 只做用户域；系统域显式授权 + helper |
| 进程归因不准 | 限定根 + 时间窗口；不承诺内核级；高级模式需特权 |
| 应用更新改变路径 | profile 记录版本范围；更新后重新审计 |
| 恶意配方/路径穿越 | 严格 schema 校验、canonicalize、拒绝系统路径、代码评审 |
| 撤销失效（用户清空隔离区） | UI 明确保留期；manifest 记录；支持系统废纸篓模式 |
| Windows 注册表/服务复杂 | 先做用户域 + 卸载键；服务/驱动交给厂商卸载器 |

**开放问题**

1. 隔离区放在 `~/Library/Application Support/Sift/Quarantine` 是否会被用户/清理工具误删？需要加入 Sift 自身保护名单。
2. 是否默认把隔离区显示在系统废纸篓，还是只在 Sift 内可见？建议默认 Sift 内可见，7 天后提示移入废纸篓。
3. 社区配方是否需要服务端？v1 可以先做“导出 + 手动 PR”，不做自动同步。
4. 多用户/多卷场景：profile 是否需要按用户隔离？建议按 `$HOME` 维度隔离。

## 18. 结论

这套方案的关键不是“更长的路径表”，而是把卸载知识做成**有身份、有证据、有反馈、可撤销的 AppProfile**：

- **冷启动**靠 Homebrew zap + 平台约定，快速覆盖常见应用；
- **沉淀**靠安装/使用 diff、卸载演练和用户反馈，逐步覆盖长尾；
- **安全**靠结构白名单、共享检测、代码签名身份和隔离区撤销；
- **扩展**靠配方 schema 和跨平台 inventory，而不是每个应用写死代码。

按 Phase 1 → Phase 2 的顺序落地，可以先做到“找得全、看得懂、删得可恢复”，再逐步引入被动/主动学习。
