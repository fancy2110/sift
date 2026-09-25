# Sift — AI 原生的磁盘空间管理器

跨平台（macOS / Windows / Linux）磁盘空间管理与清理工具。**一套 Rust 核心，两个前端**：

| 前端 | 技术 | 状态 |
|---|---|---|
| `src-tauri/` + `src/` | Tauri v2 + Svelte 5 | 原有方案，完整保留（`pnpm check` 0 错误、`pnpm build` 通过） |
| `crates/sift-gpui-app/` | [GPUI Kit](https://gpui-kit.com) 原生桌面 | 新增方案 |

两个前端共享扫描引擎、分析判定、本地持久化与后台监控——同一个结论在两边显示的数字与理由必然一致。

## 现在能做什么

- **快**：macOS 上走 `getattrlistbulk(2)` 批量枚举。实测 61 万文件 / 57 GB 用 **2.2 s**；整卷 1388 万文件 + 400 万目录用 **202 s**（约 68.6k 文件/s，外推到 2TB/2M 文件约 **29 s**，预算 60 s）。
- **省内存**：目录节点是 72 字节定长记录，文件不建节点、名称去重、硬链接表只收多链接文件。61 万文件的树常驻 **11 MB**；打满 400 万节点预算时 **326 MB**。
- **焦点永远先到**：打开某个目录后立刻扫描它，几百毫秒内可用；焦点目录一定早于任何兄弟或无关子树出现。
- **AI 判定，但不能单独决定删除**：本地规则先提名，再给「可删 / 安全等级 / 原因」；接远程 LLM 时它只能**确认或削弱**，永远无法凭一句话把东西标成 `Safe`。
- **结论记得住**：明确可清理的清单按路径指纹落盘（原子写、版本化、有上限），重启后仍在，并作为习惯挖掘与自动清理的依据。
- **后台值守**：长期监听剩余空间，低于阈值时提示；可按「已批准的安全清单」自动清理，全部进回收站。

## 快速开始

```bash
# 1) 原生 GPUI 应用
cargo run --release -p sift-gpui-app

# 2) 原有 Tauri + Svelte 应用
pnpm install
pnpm tauri dev            # 或 pnpm dev 只跑前端

# 3) 扫描性能基准（直接验证 <60s 目标）
cargo run --release -p sift-scan --example scan_bench -- /Applications
cargo run --release -p sift-scan --example scan_bench -- / --workers 8
```

> 本机 npm 在该带空格路径下会异常短路，请使用 pnpm。

### 常用校验

```bash
cargo test                      # 278 项：单元 + 集成 + GPUI 交互级 UI 测试
cargo clippy --all-targets
pnpm check                      # Svelte 类型检查
cargo run --release -p sift-scan --example scan_bench -- <path>
```

需要系统授权的用例（真实回收站、FSEvents）标记为 `#[ignore]` 并写明原因，例如：

```bash
cargo test -p sift-platform -- --ignored
```

## 仓库结构

```
crates/
├── sift-core/       领域模型：arena 目录树、名称 interner、字节记账、删除策略
├── sift-platform/   系统能力：卷枚举、批量读目录、回收站、文件系统监听
├── sift-scan/       扫描引擎：优先级调度、内存预算、事件流
├── sift-analyze/    智能分析：候选规则、判定器契约、隐私路由、习惯挖掘
├── sift-store/      本地持久化：结论缓存、可清理清单、决策日志、设置
├── sift-monitor/    后台监控：阈值策略、安全自动清理
└── sift-gpui-app/   GPUI 原生前端
src/                 Svelte 前端（配套 src-tauri）
src-tauri/           Tauri 适配层（命令 + 事件翻译）
docs/
├── architecture/overview.md        分层、关键决策、实测数据
├── architecture/gpui-kit-api-cheatsheet.md  gpui-kit 0.6.6 精确 API（含陷阱）
└── research/fast-disk-scan.md      2TB/60s 快速扫描调研（三平台 API、内存架构、引用）
```

架构与设计决策见 **[docs/architecture/overview.md](docs/architecture/overview.md)**；扫描算法调研见 **[docs/research/fast-disk-scan.md](docs/research/fast-disk-scan.md)**。

## 安全边界（重要）

这些不是「待办」，是当前实现里已经生效的约束：

1. **删除永远进回收站**，从不永久擦除；失败按条目回报，一个文件失败不影响整批。
2. **自动清理只动 `Safe` 且已逐条批准的条目**；`Review` 是「需要人来判断」，不是长期许可。
3. **删除前重新校验指纹**：条目在批准之后发生变化（大小/mtime/类型）就不再匹配，会被跳过。
4. **`home_only` 默认开启**：主目录之外不自动删；无法证明在主目录内时拒绝。
5. **超大批次强制确认**：超过上限时整批转为「需要确认」，绝不静默截断。
6. **API Key 不落盘**：设置里只存环境变量名。
7. **远程判定是 opt-in**：编译期 feature 门控 + 运行期显式同意；结构性家族（缓存、构建产物、回收站）永不上传；只发脱敏路径与元数据，不发文件内容。

## 跨平台状态

| 能力 | macOS | Windows | Linux |
|---|---|---|---|
| 批量读目录 | `getattrlistbulk` ✅ 已实现并实测 | `read_dir`+`metadata`（可移植回退） | `read_dir`+`metadata`（可移植回退） |
| 硬链接去重 | ✅（含链接数） | ✅（`number_of_links`） | ✅ |
| 卷枚举 | ✅ | ✅ | ✅ |
| 回收站 | ✅（需 Finder 自动化授权） | ✅ | ✅ |
| 删除监听 | ✅ FSEvents | ✅ ReadDirectoryChangesW | ✅ inotify |
| 整卷快路径 | 已实现 | 已调研（直读 `$MFT` / `FileIdBothDirectoryInfo`），未实现 | 已调研（`getdents64`+`statx`），未实现 |

Windows / Linux 目前走 Tier 2 可移植回退：**每目录一次批量枚举、绝不 per-file stat**，按调研结论冷缓存 2M 文件约 20–35 s，仍在预算内。
