# Sift 发布打包指南（Linux / Windows / macOS）

本文档说明从干净仓库产出三个平台正式安装包需要的条件、每平台的产出物、
签名/公证要求，以及如何配合 GitHub Release 与 GitHub Actions 发布。

配套 CI：[`.github/workflows/release.yml`](../../.github/workflows/release.yml)，
推送 `v*` tag 后自动三平台并行构建并挂到 GitHub Release（草稿）。

---

## 0. 版本号来源（单一事实源）

三处版本号必须一致，tag 也按同一版本命名：

1. `Cargo.toml` workspace 的 `version`（所有 crate 与 `src-tauri` 共用）；
2. `package.json` 的 `version`；
3. `src-tauri/tauri.conf.json` 的 `version`。

发布流程：

```bash
# 例：0.1.0
VER=0.1.0
# 1) 改三处版本号并提交
# 2) 打 tag（带 v 前缀）
git tag -a "v$VER" -m "Release $VER"
git push origin "v$VER"
# 3) CI 构建三平台产物，创建/更新一个 *草稿* Release
# 4) 在 GitHub 上核对产物后手动点 Publish
```

Tauri 会从 `tauri.conf.json` 读取版本写进安装包元数据；tag 名决定
GitHub Release 名与产物的归类。

---

## 1. 通用构建前提

| 依赖 | 版本/说明 |
|---|---|
| Rust toolchain | stable，`rustup default stable`；CI 用 `dtolnay/rust-toolchain@stable` |
| Node.js | ≥ 20（CI 用 22）；包管理器固定 **pnpm**（本机带空格路径下 npm 会短路） |
| pnpm | 随 `pnpm-workspace.yaml` / `packageManager`；`corepack enable && corepack prepare pnpm@latest --activate` |
| Tauri CLI | 已在 devDependencies（`@tauri-apps/cli@3.0.0-alpha.3`），用 `pnpm tauri ...` 调用，无需全局安装 |
| 前端构建 | `pnpm build`（Vite → `dist/`），Tauri 的 `beforeBuildCommand` 会自动执行 |
| 图标 | `src-tauri/icons/` 已含 `icon.icns` / `icon.ico` / 各尺寸 PNG；换图标后需重新生成三种格式 |

本地任意平台出包的统一命令：

```bash
pnpm install
pnpm tauri build            # 出当前平台所有默认目标（macOS: app+dmg; win: nsis/msi; linux: deb/AppImage/rpm）
```

产物在 `src-tauri/target/release/bundle/`。

特性开关：`src-tauri` 默认开启 `remote-ai`（本地三规则之外的可选远程判定，
仍受编译期 feature + 运行期显式同意双重门控）。如要出**纯离线**包：

```bash
pnpm tauri build -- --no-default-features
```

---

## 2. macOS

### 2.1 构建机条件

- macOS 主机（**只能在 macOS 上交叉/原生构建 macOS 包**，无法在 Linux 出 .app/.dmg）；
- Xcode Command Line Tools：`xcode-select --install`；
- 目标架构：
  - Apple Silicon：`aarch64-apple-darwin`（默认）；
  - Intel：`x86_64-apple-darwin`，`rustup target add x86_64-apple-darwin`。
  - 出 Universal Binary：`pnpm tauri build --target universal-apple-darwin`
    （需要两个 target 都装好，产物是一个同时含两架构的 .app/.dmg）。

### 2.2 产出物

- `src-tauri/target/re/bundle/macos/Sift.app`
- `src-tauri/target/release/bundle/dmg/Sift_<ver>_<arch>.dmg`

### 2.3 签名与公证（正式分发**必需**）

未签名的 app 用户首次打开会被 Gatekeeper 拦截（需右键→打开，且公司外分发体验很差）。
正式 release 需要：

1. **Apple Developer 账号**（付费，99 USD/年）；
2. 在 [developer.apple.com](https://developer.apple.com) 创建：
   - `Developer ID Application` 证书（用于 app 签名，区别于 Mac App Store 的证书）；
   - 一个 **App 专用密码**（App-specific password，用于 notarytool 公证）；
   - 记录 Team ID。
3. 把证书与私钥导出为 `.p12`（Keychain 里右键导出，设导出密码）。

CI 所需的 GitHub Actions secrets：

| Secret | 内容 |
|---|---|
| `APPLE_CERTIFICATE` | `.p12` 的 **base64**：`base64 -i cert.p12 \| pbcopy`，剪贴板内容贴进 secret |
| `APPLE_CERTIFICATE_PASSWORD` | 导出 .p12 时设的密码 |
| `APPLE_ID` | 公证用的 Apple ID 邮箱 |
| `APPLE_PASSWORD` | 该 Apple ID 的 App 专用密码 |
| `APPLE_TEAM_ID` | 开发者 Team ID（10 位） |

本地手动签名+公证（排查 CI 时用）：

```bash
# 签名（Tauri 也可直接做，见下）
codesign --force --options runtime \
  --sign "Developer ID Application: Your Name (TEAMID)" \
  src-tauri/target/release/bundle/macos/Sift.app

# 公证（新版命令行工具）
xcrun notarytool submit Sift_0.1.0_aarch64.dmg \
  --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID" --wait
xcrun stapler staple Sift_0.1.0_aarch64.dmg
```

Tauri 3 推荐把签名/公证交给配置（`tauri.conf.json` 的 `bundle.macOS`）或环境变量，
CI workflow 已通过环境变量接入，无需把账号写进配置文件。

> 首次签名需在真实 macOS 上跑通一次（Keychain 授权、证书信任）。
> 仓库的 `Info.plist` 已声明 Desktop/Documents/Downloads/外接盘用途描述，
> 全盘访问（Full Disk Access）仍由首次扫描时系统引导，不进包签名。

---

## 3. Windows

### 3.1 构建机条件

推荐在 **Windows + MSVC** 原生构建（CI 用 `windows-latest` runner）：

- Visual Studio 2022 Build Tools，勾选 **“使用 C++ 的桌面开发”**（MSVC + Windows SDK）；
- Rust 目标 `x86_64-pc-windows-msvc`（`rustup default stable-x86_64-pc-windows-msvc`）；
- WebView2 Runtime：Win11 自带；Win10 较旧版本需随安装包引导
  （Tauri 的 NSIS 安装器默认带 WebView2 bootstrapper）；
- NSIS：Tauri 自动下载其内置 NSIS 工具链，无需手工安装。

也可在 Linux/macOS 用 `x86_64-pc-windows-gnu` + mingw-w64 做**可移植回退**构建，
但本项目面向正式分发只以 **MSVC + NSIS** 为准。

### 3.2 产出物

- NSIS：`src-tauri/target/release/bundle/nsis/Sift_<ver>_x64-setup.exe`
- （如启用 MSI）`bundle/msi/Sift_<ver>_x64_en-US.msi`

默认 `bundle.targets = "all"` 会同时尝试 NSIS/MSI；若只想要安装 exe，
可在 CI 用 `--bundles nsis` 限定。

### 3.3 代码签名（建议，非强制）

未签名的 exe 会触发 SmartScreen 蓝色警告（签名后需要积累一段信誉度才会完全消失）。
正式分发建议签名：

- 需要一张 **OV（组织验证）或 EV（扩展验证）代码签名证书**（DigiCert/Sectigo/GlobalSign 等）；
- EV 证书通常在 USB 令牌/HSM 里，GitHub 托管 runner 上自动化较麻烦，
  常用方案是托管在支持的密钥云（如 Azure Key Vault / DigiCert ONE）并通过其 signtool 桥接。

CI secrets（OV 证书文件方式）：

| Secret | 内容 |
|---|---|
| `WINDOWS_CERTIFICATE` | 代码签名证书（含私钥的 .pfx/**.p12**）base64 |
| `WINDOWS_CERTIFICATE_PASSWORD` | .pfx 密码 |

Tauri 3 识别 `TAURI_SIGNTOOL_*` 环境变量；workflow 已预留，未配置 secrets 时
**跳过签名但照常出包**（草稿 Release 里即为未签名安装器，供内部测试）。

---

## 4. Linux

### 4.1 构建机条件

在 Linux（CI 用 `ubuntu-22.04/24.04`）上需要 WebKitGTK 与打包器系统依赖：

```bash
# Debian/Ubuntu
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev \
  patchelf gnome-desktop-testing   # patchelf 供 AppImage 使用
```

- glibc 兼容性：在**较旧**的 Ubuntu（如 22.04）上构建，能在更广泛的发行版运行；
  在 24.04 上构建的包可能无法在 22.04 上运行（glibc 较新）。CI 默认 22.04。
- AppImage：Tauri 自动下载 `appimagetool`；
- deb：需要 `dpkg`（镜像自带）；
- rpm：需要 `rpm`/`rpmbuild`（如出 rpm，apt install rpm）。

### 4.2 产出物

- Debian/Ubuntu：`bundle/deb/sift_<ver>_amd64.deb`
- 通用：`bundle/appimage/sift_<ver>_amd64.AppImage`
- （可选）RPM：`bundle/rpm/sift-<ver>.x86_64.rpm`

### 4.3 签名说明

- Linux 桌面没有统一的“必须签名”门禁；AppImage/deb 可直接运行/安装。
- 若要进软件中心或企业分发，可对 deb 做 **GPG 签名**（dpkg-sig / debsign）
  并维护 apt 仓库；本项目当前 release 不强制，产物以 SHA256 校验和为准
  （CI 会为所有产物生成 `.sha256.txt`）。

---

## 5. GitHub Release 发布流程

### 5.1 自动（CI，推荐）

1. 确认三处版本号 → 合并到 `main`；
2. 打 tag `vX.Y.Z` 并 push；
3. workflow `Release (3 platforms)`：
   - `tauri-apps/tauri-action` 三平台矩阵构建；
   - macOS 有签名 secrets 就签名+公证，否则出未签名包；
   - Windows 有证书就签名；
   - 上传 `.dmg/.app`、`-setup.exe`、`.deb/.AppImage` 及 `SHA256SUMS`；
   - 创建/更新一个 **draft** Release，名 `Sift vX.Y.Z`，tag `vX.Y.Z`；
4. 在 GitHub Releases 页打开草稿：
   - 逐产物核对（名称、体积、SHA256）；
   - macOS 产物至少在一台干净机器验证 Gatekeeper（已公证应直接打开）；
   - 写 release notes（中英文）；
   - 点 **Publish release**。

未配置任何签名 secret 时 CI 也能跑通，产物可用但 macOS 需右键打开、Windows 有
SmartScreen 提示——只适合内部测试，不要对公众 Publish。

### 5.2 手动出单个平台（CI 不可用或热修）

```bash
# macOS（本机 Apple Silicon）
pnpm install
pnpm tauri build

# Windows（PowerShell，MSVC 环境）
pnpm install
pnpm tauri build --bundles nsis

# Linux
sudo apt install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf
pnpm install
pnpm tauri build --bundles deb,appimage
```

然后到 GitHub Releases 手动建 release，把上述文件拖入附件区并补 SHA256：

```bash
shasum -a 256 * | tee SHA256SUMS   # macOS/Linux
certutil -hashfile setup.exe SHA256  # Windows
```

### 5.3 release checklist（发布前人工核对）

- [ ] 三处版本号一致且 tag 为同一 `vX.Y.Z`；
- [ ] `cargo test --workspace` 全绿；`cargo clippy --workspace --all-targets` 无新增警告；
- [ ] `pnpm check` 0 错误；`pnpm build` 成功；
- [ ] 至少在当前平台真实 `pnpm tauri build` 出包并安装/启动一次；
- [ ] 核心流程冒烟：选盘 → 扫描 → 分析 → 勾选 → 删除进回收站 → 撤销/恢复；
- [ ] macOS 包已公证（`spctl -a -vv -t install Sift.app` 通过）；
- [ ] Windows 安装器在干净 VM/机器安装并启动（含 WebView2 引导）；
- [ ] 所有附件 SHA256 已记录；
- [ ] Release notes 写明破坏性变更/安全边界（删除只进回收站等）。
