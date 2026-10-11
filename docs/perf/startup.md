# 启动性能：500 ms 到主界面

## 目标

冷启动（用户日常再次打开 App，系统已缓存二进制与框架）从进程起到**看到完整主界面** ≤ 500 ms。

## 测量方法

- 原生侧：`src-tauri/src/startup_trace.rs`，`main()` 第一行锚定单调时钟，记录
  `services → setup → Ready → tray` 各阶段；`SIFT_STARTUP_TRACE=1` 时输出到 stderr。
- WebView 侧：`src/lib/startup.ts`，用 `Date.now()` 对齐原生锚点，记录
  `main-module → app-mounted → FCP → first-frame → init-done`。
- 自动化：`scripts/measure-startup.sh [runs]`，输出 pre-main（dyld）与各阶段。
- 生产构建必须用 `pnpm tauri build --no-bundle`。直接 `cargo build --release`
  不带 Tauri 的 `custom-protocol` feature，WebView 会去加载 `localhost:5173`
  的开发页，测到的不是生产启动（这次排查中踩过）。

## 优化前基线（生产包，热缓存，多次中位数）

| 节点 | 相对进程起 |
|---|---|
| dyld / pre-main（第二次启动） | ~10 ms |
| setup 结束（含托盘） | ~150 ms |
| WebView 主模块求值 | ~270 ms |
| 首帧内容（FCP） | ~335 ms |
| 数据就绪（init-done） | ~430 ms |
| **入场动画全部结束、界面真正完整** | **~950–1140 ms** |

瓶颈分布：

1. **首屏入场动画的固定 delay（最大问题）**：四张入口卡片 `in:fly` 带
   420/480/540/620 ms delay + 520 ms duration，CTA 380 ms delay。FCP 之后
   界面仍大面积透明/位移，"看到主界面"实际要等到约 1 s 后。
2. **初始 JS 包含全部视图**：`SubPageView`（1497 行）、`DashboardView`、
   `SettingsDialog`、`PermissionDrawer` 以及 860 行的浏览器 mock backend 全在
   185 KB 的首包里，冷启动必须解析编译用不到的代码。
3. **启动 IPC 串行**：前端 `init()` 顺序 await
   listeners → scanRunning → warnings → listVolumes → history → routines，
   6 个往返累加；其中 `list_volumes`（sysinfo 刷全盘统计，~44 ms）跑在平台主线程。
4. **i18n 阻塞 init**：`initI18n().then(() => store.init())`，一次额外串行往返，
   而内置默认字典已是 zh，首帧并不需要等它。
5. **托盘构建在 setup 关键路径**：NSStatusBar/菜单构建 ~8–10 ms，发生在窗口创建前。
6. **窗口默认白底**：CSS 加载前是白屏闪烁。

## 已实施的优化（分支 perf/startup-under-500ms）

前端：

- 冷启动首次挂载时，主界面所有入场动画 `duration:0`，内容在 FCP 即完整可见；
  之后从其他视图返回 Home 仍保留原动画（`firstBoot` 一次性标志，`onMount` 复位）。
- 视图级代码分割：`DashboardView` / `SubPageView` 动态 import；
  `SettingsDialog`、`PermissionDrawer`、mock backend/events 全部移出首包。
  首包 JS 185 KB → **129 KB**（gzip 61 → 46 KB），CSS 68 → **35 KB**。
- `init()` 中监听器注册后，scanRunning/warnings/volumes/history/routines/
  monitorStatus 一次性并发；i18n 与 store init 并发。
- 去掉 Home 外层冷启动 fade 包裹。

后端：

- `list_volumes` 改 `async + spawn_blocking`，不再占用平台主线程（网络盘挂起时
  尤其关键）。
- 托盘从 `setup` 推迟到首个 `RunEvent::Ready`，移出窗口创建关键路径。
- 窗口 `backgroundColor: #1b1e24`（深色 bg），消除 CSS 前的白闪。
- 启动埋点成为常驻 opt-in 工具（`SIFT_STARTUP_TRACE=1` + `startup_trace` 命令）。

## 优化后结果（生产 .app，热缓存，5 次）

| 节点 | 优化前 | 优化后 |
|---|---|---|
| FCP | ~335 ms | **300–320 ms** |
| 第一帧 (rAF) | ~370 ms | **320–340 ms** |
| 数据就绪 | ~430 ms | **343–363 ms** |
| **完整主界面可见** | **~950–1140 ms** | **~330 ms（FCP 即完整）** |
| 首包 JS / gzip | 185 / 61 KB | **129 / 46 KB** |
| 首包 CSS / gzip | 68 / 12 KB | **35 / 7.5 KB** |

结论：热启动（用户真实日常场景）进程起到完整主界面稳定在 **~340 ms**，目标达成。

## 冷启动说明

系统缓存全冷（重启后首次、换路径的新二进制）实测 dyld pre-main 约
**630–730 ms**，几乎全部在 dyld 加载/校验 WebKit、AppKit、JavaScriptCore 等
Apple 框架（仅 17 个 dylib，但 WebKit 巨大），发生在任何应用代码之前，Rust/JS
侧无法消除；它与首次启动时 Finder/Dock 的"App 首次出现"延迟同源。

缓解手段（未在本期实施，属于发布/系统集成层）：

- 用正式 Developer ID 签名 + notarization 的 .app 分发（当前本地构建是 adhoc，
  首次路径会触发额外校验）。
- PACT 预热：登录项以 KeepAlive 驻留会改变产品形态，不建议仅为指标采用。
- 持续监控框架数量，不引入新的重框架（现 17 个 dylib 已很少）。

## 验证

- `pnpm check`：0 error / 0 warning。
- `cargo test`（全 workspace）：316 项全过。
- 生产包手动验证：FCP 时四张卡片与 CTA `opacity=1`；点击智能清理触发
  SubPage 懒加载 chunk 并正常渲染；无 panic/JS 报错。
