# Sift 需求清单与依赖关系

AI 原生的跨平台磁盘空间管理器。Tauri v2（Rust 后端 + Svelte 5 前端），支持 macOS / Windows / Linux。

## 需求拆分（Epic → Task）

### E1 工程骨架
- **R1.1** Tauri v2 接入：`src-tauri/`（Cargo.toml、tauri.conf.json、capabilities）、Vite `devUrl`/`beforeDevCommand`、三端构建配置
- **R1.2** 应用图标与窗口基线（最小尺寸、记住窗口位置）

### E2 磁盘与扫描（数据底座）
- **R2.1** 磁盘卷宗枚举：列出系统盘/外置盘，名称、总容量、可用空间、挂载点、可移除标记
- **R2.2** 增量优先级扫描引擎：后台线程 + 有界优先队列，事件流式推送。优先级：**当前目录 → 父目录链 → 兄弟目录 → 其余**；随时可改"焦点路径"并重排队
- **R2.3** 扫描事件协议：`discovered(节点)` / `size_updated(节点,累计大小)` / `scan_progress` / `scan_done`；节点带稳定 id、路径、是否目录、mtime、可删除权限标记
- **R2.4** 扫描取消 / 重扫 / 内存预算（树深度与单目录条目上限、惰性聚合）

### E3 前端实时数据层
- **R3.1** IPC 封装（invoke + 事件监听，TypeScript 类型与后端对齐）
- **R3.2** 实时增量树 store：节点按事件 upsert、大小滚动更新；当前目录**立即可见**（未扫完显示扫描中）
- **R3.3** Treemap / 目录列表 / 面包屑绑定实时树；焦点切换即重排扫描优先级
- **R3.4** 容量与磁盘选择器接真实卷宗

### E4 删除与状态感知
- **R4.1** 跨平台移入回收站（macOS Trash / Windows Recycle Bin / Linux trash，遵循 FreeDesktop 规范），失败回退错误而非永久删除
- **R4.2** 权限判定：按路径/属主/ACL 标记 deletable；无权限项禁用删除并说明
- **R4.3** 删除状态监听：删除命令结果 + **文件系统 watcher** 感知外部删除（用户在 Finder/资源管理器里删），推送 `node_deleted`
- **R4.4** 前端删除动作、队列移除、节点剪枝与大小实时重算

### E5 AI 分析（本地启发式，离线）
- **R5.1** 规则引擎：可重建缓存（DerivedData/node_modules/build/.gradle 等）、安装包与归档、久未访问大文件、疑似重复（同名+同大小）、回收站/临时文件；每条给理由/风险分级(safe/review/keep)/置信度
- **R5.2** 分析随扫描增量产出，支持对任意子树重算
- **R5.3** 前端结论渲染与勾选联动（保留现有 UI）

### E6 习惯沉淀与例行任务
- **R6.1** 决策记录：持久化每次用户勾选/删除/保留的路径指纹
- **R6.2** 习惯挖掘：重复决策（≥3 次周期出现）建议沉淀为例行任务
- **R6.3** 例行任务 CRUD + 调度器（cron 式周期）+ 自动模式（仅提醒/执行前确认/自动执行）
- **R6.4** 安全边界：自动仅限 safe 级、超大体积强制确认、个人目录白名单外不自动删
- **R6.5** 前端例行任务页接真实数据（启动/删除/模式切换）

### E7 系统集成
- **R7.1** 系统托盘：图标 + 菜单（打开、立即扫描、自动整理开关、退出），长时任务进度在菜单/tooltip 体现
- **R7.2** 内存优化：扫描树超预算时折叠、扫描线程限速、托盘菜单不堆积
- **R7.3** 后台运行：关闭窗口驻留托盘；开机自启（登录项/注册表/autostart）
- **R7.4** 通知与授权：需真实动作（删除/全盘访问/自启动）时系统通知，点击授权；macOS 全盘访问引导、Windows 提示、Linux PolicyKit 场景说明
- **R7.5** 多语言：i18n 中英文，跟随系统语言可手动切换；后端用户可见文案经前端 i18n key 输出

## 依赖关系（开发顺序）

```
R1.1 ── R1.2
 │
 ├─ R2.1 ── R2.2 ── R2.3 ── R2.4
 │                         │
 │                         └─ R3.1 ── R3.2 ── R3.3 ── R3.4
 │
 ├─ R4.1 ── R4.2 ── R4.3 ── R4.4   (依赖 R3.x 的实时树)
 │
 ├─ R5.1 ── R5.2 ── R5.3           (依赖 R2 扫描数据)
 │
 ├─ R6.1 ── R6.2 ── R6.3 ── R6.4 ── R6.5  (依赖 R4/R5)
 │
 ├─ R7.5 (i18n，先行铺 key，后续 feature 复用)
 ├─ R7.1 ── R7.2
 └─ R7.3 ── R7.4
```

关键路径：骨架 → 扫描引擎 → 实时树 → 删除/感知 → AI → 例行任务 → 系统集成。

## 提交策略

每个 R 任务完成后一次提交（`feat:` / `chore:` / `docs:`），提交必须通过 `svelte-check` 与 `cargo check`。

## 跨平台策略

| 能力 | macOS | Windows | Linux |
|---|---|---|---|
| 回收站 | NSFileManager (trash) | IFileOperation SHFileOperation | trash crate (XDG trash) |
| 容量 | statfs | GetDiskFreeSpaceEx | statfs |
| 删除监听 | FSEvents (notify) | ReadDirectoryChangesW | inotify |
| 自启动 | Login Items (LaunchAgent) | Registry Run | .desktop autostart |
| 提权/全盘访问 | 全盘访问授权引导 | 标准用户提示 | PolicyKit 提示 |
| 托盘 | Tauri tray | Tauri tray | AppIndicator |
