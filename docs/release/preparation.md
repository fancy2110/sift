# 发版准备清单（Release Readiness）

面向「要对外发一个版本」时需要提前准备和逐项核对的事项。

- **怎么构建/签名/出安装包**：见 [packaging.md](./packaging.md)
- **自动化流水线**：见 [`.github/workflows/release.yml`](../../.github/workflows/release.yml)
- 本文聚焦：**版本与更新日志、签名账号与密钥、质量门禁、分发渠道、回滚预案**，
  以及首次发版（v0.1）特有的一次性准备。

约定状态标记：
- ✅ 已具备 / 已完成
- ⚠️ 已具备但需发版前确认
- 🔴 缺失，发版前必须补

---

## 1. 版本与命名

- 🔴 **统一版本号**（三处必须一致，目前均为 `0.1.0`）：
  - `Cargo.toml` workspace `version`
  - `package.json` `version`
  - `src-tauri/tauri.conf.json` `version`
  - Git tag：`vX.Y.Z`（CI 以此触发）
- 版本策略建议（语义化版本，0.x 阶段）：
  - `0.1.0`：首个内测（alpha/beta），可标 `0.1.0-beta.1`；
  - 破坏性数据结构变化（如扫描日志 schema）需 bump minor；
  - 纯修复 patch。
- 🔴 **更新日志**：仓库当前无 `CHANGELOG.md`，首发应建立，按版本记录
  「新增 / 修复 / 已知问题」。Release notes 中英文各一份。
- 产品命名一致性：窗口标题、productName、安装包名、README 中的名称统一为 **Sift**。

## 2. 应用元数据（安装包信息）

`src-tauri/tauri.conf.json` 当前缺失以下分发常用字段，正式渠道（尤其
Windows SmartScreen、软件中心、公证）建议补齐：

- 🔴 `publisher` / 版权人（Windows 安装器与签名信息需要一致的组织名）
- 🔴 `shortDescription` / `longDescription`（应用商店与安装器描述）
- 🔴 `copyright`
- ⚠️ Windows 段（`bundle.windows`）：`webviewInstallMode`（默认 NSIS 带
  WebView2 bootstrapper，需确认目标用户网络可下载）、`nsis.installMode`、
  是否要 `wix` MSI。
- ⚠️ macOS 最低系统版本：当前 `Info.plist` 未设 `LSMinimumSystemVersion`，
  需确认支持下限（getattrlistbulk 快速路径与 WebKit 版本相关）。
- ✅ `identifier = com.sift.app`（建议确认为团队实际拥有的反向域名；
  更换 identifier 会改变 TCC 授权、自启动项与数据目录归属，首发后不宜再改）。
- ✅ 图标三格式齐全（icns/ico/各尺寸 PNG，Reclaim Wedge 品牌图标已替换）。

## 3. 签名、公证与账号（首次发版最耗时，提前办理）

### macOS（不签则无法对外分发）
- 🔴 Apple Developer Program（99 USD/年）
- 🔴 `Developer ID Application` 证书（分发用，非 Mac App Store 证书）
- 🔴 App 专用密码（notarytool）+ Team ID
- GitHub secrets：`APPLE_CERTIFICATE`、`APPLE_CERTIFICATE_PASSWORD`、
  `APPLE_ID`、`APPLE_PASSWORD`、`APPLE_TEAM_ID`
- 验收：`spctl -a -vv -t install Sift.app` 通过；干净机器双击直接打开、
  首次扫描的文件夹授权引导正常。

### Windows（不签会有 SmartScreen 蓝框）
- 🔴 OV/EV 代码签名证书；EV 多在硬件令牌/HSM，CI 需密钥云方案
  （Azure Key Vault / DigiCert ONE）。
- GitHub secrets：`WINDOWS_CERTIFICATE`、`WINDOWS_CERTIFICATE_PASSWORD`
- 心理预期：OV 证书即便有效，新证书仍需积累 SmartScreen 信誉，
  首发蓝框可能持续一段时间。

### Linux
- ✅ 无强制签名门禁；以 SHA256 校验和为准（CI 已生成）。
- ⚠️ 若进软件中心/企业 apt 源，再评估 GPG 签名与仓库维护。

## 4. 质量门禁（发版前必须全绿）

命令与发布 checklist 与 packaging.md §5.3 对齐：

- [ ] `cargo test --workspace` 全绿（含 `--features remote-ai`）
- [ ] `cargo clippy --workspace --all-targets` 无新增警告
- [ ] `pnpm check` 0 错误；`pnpm build` 成功
- [ ] 三平台至少各自真实出包并安装/启动一次（CI 出包≠验证过安装体验）
- [ ] 冷启动达标：`SIFT_STARTUP_TRACE=1` 生产包首帧 ≤ 500ms
      （基线与测量见 [docs/perf/startup.md](../perf/startup.md)、
      `scripts/measure-startup.sh`）

### 手工核心流程冒烟（每平台一遍）
- [ ] 选盘 → 扫描，进度 0→100 平滑、与扫描完成对齐、可随时中断
- [ ] 扫描取消后能立即重新扫描；中途切盘/切焦点正常
- [ ] 分析出候选；勾选 → 清理，清理中可「停止」，未处理项保持勾选可续做
- [ ] 删除项确实进入回收站且可从回收站还原（非永久删除）
- [ ] 权限拒绝目录：出现授权引导；授权后刷新计数；拒绝不阻塞全盘
- [ ] 计划任务/自启动项：开关生效；自动清理只动已批准的 Safe 项
- [ ] 深浅色模式：标题栏与内容一致；系统切换外观实时跟随
- [ ] 关闭窗口驻留托盘；Dock 重开/托盘菜单恢复窗口；托盘退出才真正退出
- [ ] 浏览器 mock 模式（`pnpm dev` 非 Tauri 壳）不报错

### 数据与隐私
- [ ] API Key 不落盘明文（Keychain / 0600 文件）确认
- [ ] 远程判定 opt-in 流程：未同意不发起任何模型请求
- [ ] 不上传文件路径/内容；CSP 与外链请求核对
- [ ] 卸载/重装后本地数据（`~/.sift` 等）行为符合预期

## 5. 分发与渠道

- 🔴 发布渠道确认：仅 GitHub Releases？是否同时提供 Homebrew cask、
  官网下载、应用内更新？
- ⚠️ 应用内自动更新：当前**未接入** Tauri updater。若首发就要自动更新，需：
  - 配置 `bundle.createUpdaterArtifacts` 与 updater endpoint / 公钥；
  - 维护 `latest.json` 与签名（macOS 需更新包签名）。
  - 否则在 README/Release notes 明确「首版本需手动下载更新」。
- ⚠️ Release 产物命名与说明：dmg/exe/AppImage/deb、架构后缀
  （aarch64/x86_64/universal）、SHA256SUMS。
- ⚠️ 首发是否发 **草稿/预发布（pre-release）**：建议先发 `v0.1.0-beta`
  作为 pre-release 小范围内测，收集真机问题后再发正式 tag。

## 6. 文档与支持

- 🔴 README 面向用户的下载/安装说明（含 macOS 右键打开、Windows
  SmartScreen 的过渡提示，未签名/新签名阶段需要）
- ⚠️ 隐私政策/数据说明（涉及文件扫描、可选 AI 远程判定，分发平台常要求）
- ⚠️ 反馈渠道：issue 模板 / 邮箱 / 应用内「关于」里的入口
- ⚠️ 系统要求写明：macOS 版本下限、Windows 10/11 + WebView2、
  Linux WebKitGTK 依赖

## 7. 回滚与应急预案

- [ ] 每个 release tag 对应产物与 SHA256 长期保留，可重新下发旧版
- [ ] 数据结构（扫描日志 SQLite）有 user_version 迁移；新版本读取失败时
      不会损坏用户数据，旧版遇到高版本库有明确处理
- [ ] 自动清理（计划任务）有安全护栏：仅回收站、仅已批准 Safe 项、
      home_only；出问题时用户可从回收站恢复
- [ ] 发布后观察期：崩溃日志/反馈渠道有人值守；准备一个 patch 版本的
      快速通道（分支 + tag + CI）

---

## 首次发版（v0.1）一次性准备顺序建议

1. **尽早办理**证书与开发者账号（Apple 审核/制证、Windows EV 出证可能数天）。
2. 确认 bundle id、publisher、应用描述、最低系统版本等元数据。
3. 建 `CHANGELOG.md`，确定版本号并改三处。
4. 三平台 CI 在**未配置签名 secrets** 下先跑通，产出内测包。
5. 小范围 beta（pre-release）真机冒烟，重点验证回收站删除、权限引导、
   扫描/取消/清理、深浅色、托盘驻留。
6. 配好签名 secrets 后重跑，产出签名/公证包，干净机器验收 Gatekeeper。
7. 补 README 下载安装说明与隐私说明，写中英文 release notes。
8. 核对 SHA256，发布；安排观察与 patch 通道。
