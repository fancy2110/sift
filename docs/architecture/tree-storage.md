# 文件树存储：数据结构、快照与查询预算

## 1. 约束

1. 扫描完成后，每个目录的文件列表 + 大小可查询。
2. 子目录变化必须沿父链实时更新。
3. 支持磁盘快照/恢复，磁盘占用受控。
4. 未访问目录首次打开 < 300 ms；已访问目录列表/大小 < 100 ms。

## 2. 现状 review

旧结构有三套事实：

- `ScanManager::last_tree`：扫描期 arena 树；
- `size_tree.json`：跨会话的尺寸 JSON，测试机上达到 **67 MB**，启动/首次列目录会阻塞；
- 前端 `nodeRecords` + `dirCache`：列表和尺寸的第二份投影。

问题不是查询慢，而是**事实分散 + 全量 JSON 重写**：目录列表在 `size-tree` 和前端各存一份；每次保存都可能重写整棵尺寸树；启动要解析几十 MB JSON。

## 3. 选型

采用 **内存热索引 + 压缩二进制快照**，而不是每次操作写 SQLite：

- 查询是 UI 热路径，目录列表/大小直接从内存 `HashMap` 读取；
- 扫描/浏览产生的变更先更新内存，父链同步更新；
- 快照按需（扫描完、30 秒节流、退出）写成单个压缩文件；
- 恢复时一次性解码进内存，之后所有查询仍是内存级。

没有选择“每个节点一行”的 SQLite 作为在线读模型，是因为：
- Sift 的读模式是“打开一个目录取整页列表”，不是随机点查；
- 内存索引可以做到微秒级，SQLite 事务/页缓存对 100 ms 预算没有收益；
- 快照只需要恢复，不需要在线 WAL 并发。

## 4. 内存结构

`crates/sift-store/src/folders.rs`：

| 结构 | 键/值 | 用途 |
|---|---|---|
| `listings` | `path -> FolderListing {resolved, entries, at_ms}` | 每个目录的列表；目录/文件都包含 |
| `sizes` | `path -> SizeRecord {size, exact, verified, at_ms}` | 每个节点大小及可信状态 |
| `nodes` | `path -> NodeRecord {parent, is_dir, purpose}` | 父子边与“作用” |
| `children` | `path -> HashSet<path>` | 父目录求和、子树删除 |
| `revision` / `size_revision` | `AtomicU64` | 列表/尺寸变更订阅 |

复杂度：

- 列目录：`HashMap` 查找 + 复制条目，O(条目数)。
- 取目录大小：`HashMap` 查找，O(1)。
- 子项更新父链：从子路径向上，父目录大小 = 已知直接子项之和；O(深度 × 直接子项数)。
- 精确父目录不会被 derived 子项和覆盖，避免丢掉父目录直接文件的字节。

## 5. 快照格式

`FolderIndex::snapshot_bytes`：

```
magic "SFTREE01"
u32 listing_count
  str requested; str resolved; u32 entry_count
    str name; u8 is_dir; u64 size; u8 has_mtime; i64 mtime
u32 size_count
  str path; u64 size; u8 exact; u8 verified; i64 at_ms
u32 purpose_count
  str path; str purpose
```

- 列表条目只存 **name**，不重复存完整子路径；路径在恢复时由 parent + name 重建。
- 整个字节流用 gzip 压缩（`flate2`，fast level）。
- 写入 `tree.snapshot.gz.tmp` 后 rename，避免半写文件。
- 恢复在 `Store::open` 时执行；失败只记录错误，回退为空索引。

## 6. 实测预算

命令：

```bash
cargo run -p sift-store --release --example tree_snapshot_bench
```

在本机 macOS / release 构建，20,000 个目录、200,000 个文件条目：

| 指标 | 结果 |
|---|---|
| 构建 20k 目录索引 | 194.8 ms |
| 快照编码 + gzip | 71.4 ms |
| 快照文件 | **656,848 bytes（0.6 MiB）** |
| 冷恢复（解码 + 重建索引） | **27.8 ms** |
| 200 次 warm list+size | **42.4 µs**（约 0.21 µs/次） |

外推 1M 条目约 3–4 MiB 快照，仍远低于旧的 67 MB JSON；恢复远低于 300 ms。实际文件数/名字长度会改变压缩率，因此应以真实目录重跑 benchmark。

## 6.1 浏览页加载顺序

1. `openExplorer` 只等待一次 Store 列表查询；列表命令不再解析旧 `size-tree`，也不做任何尺寸 walk。
2. `explorerRight`/`explorerRightPath` 一次赋值，列表开始渲染。
3. 渲染后，`fillExplorerSizes` 把所有缺失目录作为 **一个批次** 交给 `sizesFor`：
   - 先 `dirSizePeek` 从 Store 批量取已知值；
   - 仍缺失的路径一次 `dirSizesTree`，后端用最多 4 个 worker 并行 walk。
4. 每批尺寸返回后直接更新行，最后用完整行和替换头部总数；不会出现“部分子项已更新但总数还是旧值”的跳变。
5. 尺寸加载失败不阻塞列表；列表始终先可用。

这满足“先展示列表 + 并行加载尺寸”，而不是等所有目录 walk 完才显示。

## 7. 写入时机

- `scan_index_ingest` 完成后：`Store::snapshot_tree(true)`；
- `Store::save`：树脏且距上次 ≥30 s 时快照；
- `Store` 退出：强制快照；
- 浏览产生的列表/尺寸变更标记 dirty，不立即写盘。

这保证扫描结果落盘，同时避免每次浏览都重写整个快照。崩溃最多丢失最近 30 秒的树变更；对“可重新扫描/可恢复”的磁盘事实可接受。

## 8. 集成

- `scanner.rs`：`DirectoryListed` 直接把目录的完整列表（含文件）写入 Store；`DirectorySized` 写大小。
- `explorer.rs`：`list_dir_entries` 只查 Store；覆盖 Store 大小。
- `Store::open`：恢复 `tree.snapshot.gz`；`Store::save/Drop`：快照。
- `docs/architecture/store-centric.md`：Store 作为读模型的总图。

## 9. 已知边界

1. 快照是全量重写，不是增量 WAL；更适合“扫描后恢复”，不适合每秒持久化。
2. 快照不保存订阅者/前端状态；恢复后 revision 从 0 开始。
3. `size-tree.json` 已停止写入；启动时删除旧文件，模块只保留为进程内兼容索引。Store 快照是唯一持久事实。
4. 文件名/列表都在内存中；超大磁盘（千万级文件）仍需分片/按需淘汰策略。
5. 尺寸 walk 不跨挂载点（`is_same_volume`），避免 `/System/Volumes/Data` 被算进系统卷子目录后与父级/行显示不一致。
