## 亮点

0.4.0 把 0.3.0 的环境感知补成了完整闭环：卸载计划可以直接执行和恢复，包管理器维护可以逐项运行，Homebrew 卸载配方和模型维护建议会进入同一套知识库。

- 应用卸载：计划 → 勾选 → 移入 Sift 隔离区 → 随时恢复。
- 共享路径检测：另一个应用也在用的路径会自动降级为「需确认」。
- Homebrew Cask `zap` 配方：按需读取已安装 cask，把 trash/rmdir/delete 路径并入计划。
- 包管理器维护：`brew cleanup`、`pnpm store prune`、`npm cache verify`、`pip cache purge` 等，逐项确认、无 shell、带超时。
- 模型维护建议：AI 可以返回封闭动作枚举，解析后进入 `Proposed` 状态，不会直接执行。
- Windows/Linux 应用清单：Linux `.desktop`/flatpak，Windows 注册表卸载键。
- 应用目录变化会立即让环境缓存失效，不再等 5 分钟。

## 更新内容

### 卸载流程

- Dashboard 的 Environment 面板每个应用都有「管理」入口。
- 计划按缓存、保存状态、支持目录、容器、偏好、日志、LaunchAgent 分类；缓存/保存状态默认勾选，其余需确认。
- 执行前重新生成计划，前端传入的路径必须在计划内；选中项移动到 Sift 隔离区并写 manifest。
- 执行后可直接恢复；已存在的原路径不会被覆盖。

### 包管理器维护

- 新命令目录：Homebrew、npm、pnpm、yarn、pip、conda、go、gem、composer、scoop、flatpak。
- 只接受 `(pm_id, action)`，程序、参数、作用域全部来自本地目录；拒绝 `Danger` 动作。
- 超时 5 分钟；成功把建议标为 `Validated`，失败标为 `Stale`。
- UI 每个建议带「运行」按钮，执行前二次确认。

### 配方与共享

- `brew info --cask --json=v2 <slug>` 按需读取；解析 `zap` 的 trash/rmdir/delete，过滤 home 之外的路径。
- 配方路径与约定路径合并去重；配方来源优先展示。
- 其他应用的名称或 bundle id 出现在路径中时，标记 `sharedWith` 并降级为 `review`。

### 模型维护建议

- Prompt 可选要求 `maintenance` 数组，动作限定为 cleanCache / pruneStore / dedupe / removeOrphan / removeOldVersion / verifyIntegrity。
- 未知动作、非法 subject、空理由会被丢弃；提示始终以 `Proposed` 进入知识库。

### 跨平台与刷新

- Linux：`/usr/share/applications`、`~/.local/share/applications`、flatpak 导出目录。
- Windows：HKCU/HKLM 注册表 Uninstall 键（best-effort）。
- 监听 `/Applications`、`~/Applications`、Linux 应用目录、Windows Programs/Program Files；变化即标记缓存失效。

## 安装说明

- **macOS**：打开 dmg，把 Sift 拖进「应用程序」。App 已签名并公证（可离线验证）；dmg 本身未单独公证，属正常流程。
- **Windows**：安装包暂未签名，SmartScreen 可能提示「未知发布者」，选择「更多信息 → 仍要运行」。
- **Linux**：AppImage 添加可执行权限后运行；deb 用系统包管理器安装。

## 已知限制

- Windows 注册表和 Linux `.desktop` 清单为 best-effort，CI 只验证编译/测试，未在真实系统上跑 inventory。
- 包管理器 runner 覆盖保守子集；项目级命令和系统包管理器不会从 UI 执行。
- Homebrew zap 仅覆盖已安装 cask，且用应用名猜 cask slug。
- 系统级/管理员卸载项会展示但不会执行。
- 环境监听是非递归目录监听，用 dirty flag 触发重新 inventory，不是完整事件管线。
