# 快速磁盘扫描调研

> 目标平台：macOS(APFS) / Windows(NTFS) / Linux(ext4·btrfs·XFS)，Rust 实现
> 硬指标：2 TB / 1–4 M 文件端到端 **< 60 s**（笔记本级 SSD），常驻内存 **远低于 500 MB，最好 ~100 MB**；焦点目录 **几百毫秒**内可用，大小实时细化。
>
> 本文所有数字都标注了来源类型：**[实测]**（有 benchmark 命令与硬件描述）、**[真机实测]**（在 macOS 27.0 / Apple Silicon / APFS 上用只读 `ctypes`→libc 探针当场量出来的，不是从文档推断的）、**[厂商声称]**、**[估算]**。凡是来源互相矛盾或证据薄弱的，会显式说明。
>
> 其中 **macOS/APFS 一节的部分结论直接推翻了文档与广为引用的参考实现**（例如 `getattrlist(2)` man page 关于截断字段的说法、以及 dumac 解析器的字段顺序），这些都在文中标了 `[真机实测]` 并附原始字节/数值。

---

## 结论摘要

### 一句话结论

**不要用「`read_dir` + 每条目 `metadata()`」硬扫全盘。** 真正决定 60 s 目标能不能达成的是「每个文件需要几次系统调用」这个乘数：

| 路径 | 每文件系统调用 | 2 M 文件 + ~20 万目录的总调用量 |
|---|---|---|
| 朴素 `read_dir` + 逐条 `lstat` | ≈ 1（`fstatat`）+ **≈0.2**（`getdents64`，见下） | **≈ 2.1–2.4 M** |
| macOS `getattrlistbulk` | ≈ 0.01–0.02（每次调用回几十~几百条） | **≈ 2–5 万** |
| Windows `FileIdBothDirectoryInfo` 批量 | ≈ 0.01–0.03（每目录 1–3 次） | **≈ 2–5 万** |
| Windows 直读 `$MFT` | ≈ 0（纯顺序大块读，无「每文件」调用） | **≈ 数百次 `ReadFile`** |
| Linux `getdents64` + `statx` | ≈ 1（`statx`，无法批量）+ ≈0.2（`getdents64`） | **≈ 2.4 M** |

> **为什么 `getdents64` 是 0.2 次/文件而不是 0.005？** 因为**每个目录至少要多一次返回 0（EOF）的调用**。实测（见「扫描耗时的真正来源」第 3 节）：42,046 个目录 + 439,333 个条目 ⇒ **84,193 次 `getdents64`**，平均 **5.2 条/调用**。32 KiB 缓冲在条目多的目录里确实能一次回上千条，但真实目录树里**小目录占多数**，EOF 调用的开销无法摊薄。这也是 `getdents64` 占到系统调用时间 17% 的原因。

相差 2–4 个数量级。这就是为什么 WizTree/Everything 在 NTFS 上「几秒扫完整卷」，而 `du -sh` 要几十秒。

### 推荐的分层（Tier Ladder）

| 层级 | 名称 | 触发条件 | 机制 | 预期 2 M 文件耗时 |
|---|---|---|---|---|
| **Tier 0** | 目录册缓存 | 上次扫描结果 + 变更日志可用 | 读自己的持久化快照（SQLite / mmap blob），再用 FSEvents / USN Journal / inotify 增量补齐 | **0.2–3 s**（增量重扫） |
| **Tier 1a** | Windows MFT 直读 | NTFS + 已提权（`\\.\C:` 可读） | `FSCTL_GET_NTFS_VOLUME_DATA` 取 `MftStartLcn`/`BytesPerFileRecordSegment`，顺序读 `$MFT`，本地解析 `FILE` 记录 | **3–8 s** |
| **Tier 1b** | macOS 批量属性 | 任意本地卷（APFS/HFS+），非网络卷 | `getattrlistbulk(2)`（VFS 层支持，回退链见下） | **8–20 s**（冷缓存） |
| **Tier 1c** | Linux 整卷枚举 | XFS / btrfs（**建议 v2，不进第一条主线**） | `XFS_IOC_FSBULKSTAT` 给 size/nlink/ino 但**不给名字**，仍要 `getdents64` 补名字；`BTRFS_IOC_TREE_SEARCH_V2` 可一次拿到 inode item + dir item | 8–20 s（**收益远小于 NTFS，因为 XFS bulkstat 没有名字**） |
| **Tier 2** | 可移植批量回退 | 未提权 / 网络卷 / FUSE / 未知 FS | 平台批量枚举 API + **work-stealing 线程池**（Windows `FileIdBothDirectoryInfo`；macOS 若 `getattrlistbulk` 返回 `ENOTSUP` 退 `getattrlist` 单条；Linux `getdents64`+`statx`） | **15–35 s**（冷缓存，可接受） |
| **Tier 3** | 纯 POSIX 兜底 | 上面全失败 | `std::fs::read_dir` + `symlink_metadata()`，单线程或小线程池 | 40–90 s（可能超预算，必须给进度条与「慢速模式」提示） |

**关键工程结论**：Tier 1 是「锦上添花」，Tier 2 是「必须做对」的那一层。因为 Windows 上大多数用户不会提权（UAC），macOS 上 `/Volumes` 下可能是 exFAT/网络卷，Linux 上最常见的是 ext4（没有任何整卷批量接口）。**只要 Tier 2 做到「每目录一次批量枚举、绝不 per-file `stat`」，Linux 上 2 M 文件冷缓存 20–30 s、热缓存 2–5 s，就已经在 60 s 预算内。**

### 时间 / 内存预算表（2 TB，≈2 M 文件 + ≈20 万目录，笔记本 NVMe/SSD，冷缓存）

| 项目 | macOS/APFS | Windows/NTFS | Linux/ext4 |
|---|---|---|---|
| 目录枚举（Tier 1/2） | `getattrlistbulk` 8–20 s | MFT 3–8 s ／ 批量 15–30 s | `getdents64`+`statx` 20–30 s |
| 元数据解析 + 建树（Rust） | 1–3 s | 2–5 s（MFT 解析，可 4–8 线程并行） | 1–3 s |
| 聚合 / 发布事件 | 0.5–2 s | 0.5–2 s | 0.5–2 s |
| **合计** | **10–25 s** | **6–15 s（提权）/ 18–35 s（不提权）** | **22–35 s** |
| 焦点目录首屏 | < 100 ms（只扫焦点目录本身） | < 100 ms | < 100 ms |
| 单条事件延迟 | 16–100 ms（批量 flush） | 16–100 ms | 16–100 ms |
| 峰值内存（全部文件节点） | 60–120 MB | 60–120 MB | 60–120 MB |
| 峰值内存（仅目录节点，见「内存架构」） | 10–25 MB | 10–25 MB | 10–25 MB |
| 硬上限（可配置） | 400 MB / 10 M 节点后降级为「仅目录」 | 同 | 同 |

预算余量约 2×，说明这个目标是可达的，但**没有余量去容忍任何 per-file `stat`**。

---

## 扫描耗时的真正来源

### 1. 一条朴素路径到底发生了什么

以 Linux 上 Rust 的 `std::fs::read_dir` + `DirEntry::metadata()` 为例，一个包含 `n` 个条目的目录：

| 动作 | 系统调用 | 次数（该目录） |
|---|---|---|
| `read_dir()` | `openat(AT_FDCWD, path, O_RDONLY\|O_DIRECTORY\|O_CLOEXEC)` | 1 |
| 迭代 | `getdents64(fd, buf, 32768)` | `ceil(n / 每缓冲区条目数)` |
| 每条目 `metadata()`（不跟随符号链接） | `fstatat(dirfd, name, &st, AT_SYMLINK_NOFOLLOW)` | **n** |
| 迭代器析构 | `close(fd)` | 1 |

关键点：

- **`getdents64` 是摊薄的，但没有想象中那么摊薄**。缓冲区一般 32 KiB，`dirent` 头 19 B + 名字 + 对齐；平均 20–30 B/条目时，**单个大目录每次 `getdents64` 能回 1000–1500 条**。但真实目录树里小目录占多数，而且**每个目录至少要一次返回 0（EOF）的调用**，所以**树平均只有 ~5 条/调用**（实测 439,333 条目 / 84,193 次调用 = 5.2）。2 M 文件 / 20 万目录 ⇒ **约 38 万次 `getdents64`**，占系统调用时间约 17%。（[Stack Overflow: Buffer size for getdents64 to finish in one go](https://stackoverflow.com/questions/54047840/buffer-size-for-getdents64-to-finish-in-one-go)）
- **`fstatat` 是真正的瓶颈**：**每个文件一次**，不可摊薄。2 M 文件 = 2 M 次系统调用 + 2 M 次 dentry/inode 查找。
- **`getdents64` 顺带给出 `d_type`**（`DT_DIR`/`DT_REG`/`DT_LNK`），所以「判断是不是目录」**不需要** `stat`。这是免费信息，务必利用：只有需要 size/mtime/inode 时才去 `statx`。很多自己写的 walker 白白多 stat 一次，白送 30–50% 的时间。（[getdents(2)](https://man7.org/linux/man-pages/man2/getdents.2.html)）
- **macOS 上 `read_dir` 走 `getdirentries64`**：同样只有名字 + inode，**size 必须另外 `lstat64`**。healeycodes 的 dumac 文章实测，Go 版 `Readdir` 在他 409 500 文件的 benchmark 上 `getdirentries64` 调用次数是目录数的 **2 倍**，`lstat64` 约等于文件数——并在 Instruments 里看到这两项就是全部开销。（[Maybe the Fastest Disk Usage Program on macOS](https://healeycodes.com/maybe-the-fastest-disk-usage-program-on-macos)）
- **Windows 上 `FindFirstFile`/`FindNextFile`** 每次调用只回**一条** `WIN32_FIND_DATA`，所以「枚举 + 元数据」在 API 层面就是 per-entry 的；这正是它慢的根本原因，也是 `NtQueryDirectoryFile` / `FileIdBothDirectoryInfo` 存在的意义（一次调用回一缓冲区条目）。

### 2. 三种「批量」的档次差别

| 档次 | 代表 | 每文件系统调用 | 返回内容 |
|---|---|---|---|
| (a) 朴素逐条 | `read_dir` + `metadata()` | ≈ 1 + ε | 全部（但一条一次） |
| (b) 每目录批量 | `getattrlistbulk`、`NtQueryDirectoryFile(FileIdBothDirectoryInfo)`、`getdents64`+`statx` | 0.01–1 | 单目录内所有条目的 name+size+type+id |
| (c) **整卷目录册** | NTFS `$MFT` 直读、`FSCTL_ENUM_USN_DATA`、XFS bulkstat、btrfs tree search | **≈ 0**（与文件数无关的几百次 I/O） | 全卷所有 inode/MFT 记录，但**不含完整路径**，需要自己拼 |

档次 (c) 的代价从「每文件一次系统调用」变成「每卷一次顺序读 + CPU 解析」。NTFS 上 2 M 文件的 `$MFT` 约 **1.5–2.5 GB**（默认 1 KiB/记录），顺序读 2–3 GB/s ≈ 1 s，剩下全是解析 CPU。这与 Everything 作者给出的「1 000 000 文件约 1 分钟」并不矛盾——Everything 默认还建全文索引并把名字压缩进数据库，而 WizTree 只做一次流式聚合。（[voidtools FAQ](https://www.voidtools.com/faq/)）

### 3. 系统调用单价的量级 —— **有一份真实的 `strace -c` 表可以引用**

在「扫描一个目录树」这件事上，我找到的最好的一份逐系统调用实测来自 Rust 用户论坛的一篇对比帖（作者在同一棵树上跑 `statx` 版与 io_uring 版，NVMe `/mnt/sn850x`，**42,046 个目录 / 439,333 个条目**；[来源](https://users.rust-lang.org/t/batching-statx-syscall-using-io-uring/110745)）：

**单线程 `statx` 版本（单位 = 系统调用时间，秒）**：

| 系统调用 | 时间 (s) | 调用次数 | µs/次 | 占系统调用时间 |
|---|---|---|---|---|
| **`statx`** | 1.484560 | **439,333** | **3.38** | **63.93%** |
| `getdents64` | 0.399851 | 84,193 | 4.75 | 17.22% |
| `openat` | 0.195957 | 42,046 | 4.66 | 8.44% |
| `close` | 0.133081 | 42,047 | 3.16 | 5.73% |
| `fstat` | 0.107838 | 42,046 | 2.56 | 4.64% |
| **合计** | **2.322118** | **649,733** | 3.57 | 100% |

**io_uring 批量 `IORING_OP_STATX` 版本（ring size 1024）**：

| 系统调用 | 时间 (s) | 调用次数 | µs/次 | 占比 |
|---|---|---|---|---|
| `io_uring_enter` | 1.606813 | 42,052 | **38.2** | 69.26% |
| `getdents64` | 0.341071 | 84,193 | 4.05 | 14.70% |
| `openat` | 0.168630 | 42,046 | 4.01 | 7.27% |
| `close` | 0.110235 | 42,048 | 2.62 | 4.75% |
| `fstat` | 0.092055 | 42,046 | 2.19 | 3.97% |
| **合计** | **2.319863** | 252,478 | 9.19 | 100% |

**这张表说明了三件事**：

1. **`statx` 占系统调用时间的 ~64%，`getdents64` 只占 ~17%**。⇒ 优化重点在「减少每文件的元数据调用」，不在「加快目录枚举」。
2. **`getdents64` 的每次调用成本（4.75 µs）比 `statx`（3.38 µs）还高**，但它一次回 439333/84193 ≈ **5.2 个条目**（这是平均值，包含大量小目录；32 KiB 缓冲在条目多的目录里能回上千条）。**关键洞察：小目录拉高了 `getdents64` 的摊薄成本**——42k 目录里很多只有几个条目。
3. **io_uring 批量把 439,333 次 `statx` 合并成 42,052 次提交，总系统调用时间几乎没变（2.3219 s → 2.3199 s）**，因为 `io_uring_enter` 每次 38.2 µs 把省下的全吃掉。**这是「不要用 io_uring」的直接实测证据。**

⚠️ **注意**：`strace` 会放大系统调用成本（ptrace 开销），所以这些绝对值偏保守；但「谁占大头」的相对比例是可信的。另一个致命细节：`usecs/call` 在 `strace -c` 里是「只统计进行了系统调用（进入内核）的次数」，对 `statx` 这种慢路径仍具参考性。

**据此算 2 M 文件的预算**（**估算**）：2 M × 3.4 µs ≈ **6.8 s 的纯 `statx` 时间**（单线程）；200 k 目录 × (4.7 + 3.2 + 2.6) µs ≈ **2.1 s**；`getdents64` ≈ **1.9 s**。**合计约 11 s 单线程系统调用时间**，摊到 8 线程后的关键路径不到 1.5 s。**其余时间全在文件系统本身与你的 per-entry CPU 上。**

### 3b. 经验数值与顺序（**估算**）

| 操作 | 量级 | 说明 |
|---|---|---|
| 空系统调用（`getpid` 类） | 50–150 ns | 无 mitigations 的现代 x86；有 KPTI/retpoline 时退化明显 |
| `getdents64`（命中 dcache/page cache） | 2–5 µs **每次调用**，摊到每条约 0.3–1 ns（条目多时） | 见上表：实测 4.75 µs/调用，但每次回多条 |
| `fstatat`/`statx`（命中 icache） | **2–4 µs**，实测 3.38 µs | 含路径/dentry 查找、attr 拷贝；条目在 dcache 中时不触发磁盘 I/O |
| `openat` / `close`（每目录一次） | 4.66 µs / 3.16 µs | 每目录合计 ~10.4 µs（含 `fstat`） |
| 单线程 `lstat`/`statx` 循环上限 | **0.3–1 M ops/s** | CPU 上限，热缓存 |
| 冷缓存随机 4 KiB 元数据读 | SATA SSD 50–100 k IOPS；NVMe 300–600 k IOPS | **这才是冷缓存下 2 M 文件 20–30 s 的真正原因** |

**结论**：热缓存下瓶颈是 CPU（系统调用 + 建树）；冷缓存下瓶颈是**元数据块的随机 I/O**。这解释了两个经验现象：

1. `du -hs` 冷缓存 30.6 s → 热缓存 1.26 s（**24×**），而线程化工具冷缓存只快 6.8×。（[gdu benchmark](https://github.com/dundee/gdu#benchmarks)）
2. 并行度提高在 SSD 上收益明显、在 HDD 上会因寻道而恶化——gdu README 原文：「Gdu is intended primarily for SSD disks where it can fully utilize parallel processing. However HDDs work as well, but the performance gain is not so huge.」。

### 4. 实测端到端锚点（**这些是硬数据，用它们校准预算**）

**gdu benchmark**（90 GB 目录，100 k 目录 / 400 k 文件，500 GB SSD，hyperfine，Linux；[来源](https://github.com/dundee/gdu#benchmarks)）：

| 命令 | 冷缓存 Mean | 热缓存 Mean |
|---|---|---|
| `diskus ~` | 4.489 s（1.00×） | 270.8 ms（1.00×） |
| `gdu -npc ~` | 4.716 s | 466.1 ms |
| `pdu ~` | 5.969 s | 299.1 ms |
| `dua ~` | 6.030 s | 590.6 ms |
| `dust -d0 ~` | 6.181 s | 578.7 ms |
| `du -hs ~` | **30.608 s（6.82×）** | 1255.2 ms（4.63×） |
| `ncdu -0 -o /dev/null ~` | 33.163 s | 2222.4 ms |
| `gdu --db=tmp.db ~`（SQLite） | 44.989 s | 8246.7 ms |
| `gdu --db=tmp.badger ~` | 27.479 s | 15608.0 ms |

推出的吞吐：

- 400 k 文件冷缓存 **4.5–6.2 s ⇒ 65–89 k 文件/s**；热缓存 **0.27–0.59 s ⇒ 680 k–1.5 M 文件/s**。
- 换算到 2 M 文件：**冷 23–31 s，热 1.3–3 s**。→ Tier 2 在 ext4 上达标。
- `du -hs` 冷缓存 **13 k 文件/s**，2 M 文件要 **~153 s**，明确不达标。

**jwalk benchmark**（Linux 源码树，iMac Late 2015；[来源](https://github.com/Byron/jwalk/blob/main/benches/benchmarks.md)）——并行度收益的直接证据：

| 变体 | 1 线程 | 2 线程 | 8 线程 | 8 线程加速比 |
|---|---|---|---|---|
| unsorted | 141.66 ms | 88.416 ms | 54.631 ms | **2.59×** |
| sorted | 150.89 ms | — | 56.133 ms | 2.69× |
| sorted + metadata | 313.91 ms | — | 86.985 ms | **3.61×** |
| walkdir（单线程参照） | 134.28 / 170.24 / 310.26 ms | — | — | — |

注意 **带 metadata 的那一行加速比最高（3.61×）**：metadata 越多，线程化收益越大（掩盖了每个 `stat` 的等待）。

**macOS 锚点**（M1 Pro，4095 目录 / 409 500 文件，12 层，热缓存；[来源](https://healeycodes.com/maybe-the-fastest-disk-usage-program-on-macos)）：

| 实现 | 时间 | CPU |
|---|---|---|
| `du -sh`（BSD） | 2.570 s | 43% |
| Go + goroutine（`Readdir`+`lstat`） | 4.987 s（**比 du 还慢**） | 68% |
| Go + CGO + `getattrlistbulk` | 0.850 s | 443% |
| Rust + tokio + `libc::getattrlistbulk`（64 并发） | **0.52 s** | — |
| `diskus`（POSIX） | 0.52 × 2.58 ≈ 1.34 s | — |

⇒ 409 k 文件 0.52 s = **788 k 文件/s（热缓存）**。作者还明确报告：**系统调用占 91% 的时间**，tokio 调度 + inode 锁只占 1.5%。这是「瓶颈在系统调用而非 CPU 调度」的最强单点证据。

**Windows 锚点**（WizTree 官方对比页；[来源](https://diskanalyzer.com/wiztree-vs-windirstat)，**[厂商声称]**，硬件与文件数未完全披露，仅作量级参考）：

| 场景 | WizTree 3.40 | WinDirStat 1.1.2.80 | 倍数 |
|---|---|---|---|
| 25 GB HDD，Windows XP（Acer 笔记本） | 4.34 s | 3 min 20 s | 46× |
| 460 GB SSD，Windows 10（ASUS 笔记本） | 5.23 s | 1 min 55 s | 22× |

**Everything 官方 FAQ**（[来源](https://www.voidtools.com/faq/)，**[厂商声称]**）：

- 全新安装的 Windows 11（约 250 000 文件）索引约 **5 s**；**1 000 000 文件约 1 分钟** ⇒ ~17 k 文件/s。这个数字比 WizTree 的「秒级」保守得多，**两者不是同一件事**：Everything 要写持久化数据库、存名字/大小/日期并支持实时排序，WizTree 只做一次性聚合。此处**明确标注为冲突来源**，不要用「1 M 文件 1 分钟」去推断 WizTree 的速度。
- 内存：250 k 文件 ≈ **35 MB**；1 M 文件 ≈ **100 MB**（≈100 B/文件，含索引与 UI 结构）；磁盘数据库 45 MB/1 M 文件。

**ncdu 2 内存实测**（[来源](https://dev.yorhel.nl/doc/ncdu2)，作者自述为实测，硬件未详述）：

| 场景 | 文件数 | ncdu 1.16 | ncdu 2.0-beta1 |
|---|---|---|---|
| `-x /` | 3.8 M | 429 MB | **162 MB** |
| `-ex /` | — | 501 MB | 230 MB |
| backup dir | 38.9 M | 3969 MB | 1686 MB |
| many hard links | 1.3 M | 155 MB | 194 MB |

⇒ ncdu 2 在 3.8 M 文件下 162 MB ≈ **43 B/文件（含名字与哈希表开销）**。这是「2 M 文件 ~100 MB 可行」的最有力的公开实测证据。

---

## macOS / APFS

### 候选 API 对照表

| API | 平台 / 系统调用 | 每个条目返回什么 | 每条目系统调用次数 | 需要的权限 | 已知限制 | 适用性 |
|---|---|---|---|---|---|---|
| **`getattrlistbulk(2)`** | macOS 10.10+（OS X Yosemite），`#include <sys/attr.h>` | 一次调用回一缓冲区，每个目录项一组：`ATTR_CMN_NAME`、`ATTR_CMN_FILEID`(inode)、`ATTR_CMN_DEVID`、`ATTR_CMN_OBJTYPE`、`ATTR_CMN_MODTIME`、`ATTR_FILE_LINKCOUNT`、`ATTR_FILE_DATALENGTH`（逻辑）、`ATTR_FILE_ALLOCSIZE`（分配）、`ATTR_DIR_ENTRYCOUNT`、`ATTR_DIR_MOUNTSTATUS`、`ATTR_CMN_FLAGS`、`ATTR_CMNEXT_CLONEID` 等任意组合 | **≈0.01–0.02**（128 KiB 缓冲每次回几十~几百条） | `open(dir, O_RDONLY)` 的搜索权限；非沙盒/非授权路径需 Full Disk Access | 见下方 gotchas：不可请求 volume attrs；**必须**同时请求 `ATTR_CMN_NAME` + `ATTR_CMN_RETURNED_ATTRS`；buffer 打包、需按 length 步进；`ERANGE`、`EIO`；不能与同 fd 上的 `readdir()` 混用；条目顺序未定义 | **首选（Tier 1b）** |
| `getdirentriesattr(2)` | macOS（man page 标题即写明 **`getdirentriesattr(NOW DEPRECATED)`**） | 同 `getattrlistbulk`，另多一个 **`newState`** 出参让调用方检查「读取期间目录是否被修改」 | 同上 | 同上 | man page 原文：**「The `getdirentriesattr()` function is only supported by certain volume format implementations.」** —— 不是 VFS 层通用。2014-12 Apple filesystem-dev 邮件列表也明确：Yosemite 起 VFS 层新增 `getattrlistbulk`（**supported in VFS for all filesystems**），`getdirentriesattr` 已弃用 | **[真机实测] 它在现代 macOS 上是「功能性死亡」的，不要实现这条路径。** macOS 27 上遍历全部 **26 个挂载点**（`/`、`/System/Volumes/Data`、`/nix`、Recovery、Preboot、VM、xarts、Hardware、iSCPreboot、Update、6 个 CoreSimulator `iOS_*` 卷、3 个 Cryptex SimRuntime、`/dev`(devfs)、FSKit 的 `devicefs`、cryptexd、autofs），**没有任何一个 advertise `VOL_CAP_INT_READDIRATTR`，全部返回 `ENOTSUP`**。而且**被调研的 13 个 macOS 磁盘工具没有一个用它**。它的 `newState` 出参（「读取期间目录是否被改过」）思路值得借鉴到自己的竞态检测里 |
| `getattrlist(2)` | 所有 macOS 版本 | 单条 vnode 的任意 `ATTR_*` | **1** | 同上 | 每个 vnode 一次；不能批量 | ① 读目录自身属性；② `getattrlistbulk` 不可用时的兜底；③ 读 `ATTR_VOL_CAPABILITIES`、`ATTR_CMNEXT_CLONEID` |
| `readdir(3)` / `getdirentries64(2)` + `lstat64(2)` | POSIX | 只有名字 + inode；size 要另外 `lstat` | **1 + ε** | 同上 | 最慢；`du` 的走法 | Tier 3 兜底 |
| `Metadata.framework` / `NSMetadataQuery` / `mdfind(1)` | Spotlight 索引 | `kMDItemFSSize`、`kMDItemPath`、`kMDItemFSName` 等 | ≈0（读索引） | 索引已存在 | **不是可靠的扫描来源**：外置卷默认不索引、`.noindex`、`~/Library` 部分排除、索引可能过期、只给 logical size、拿不到 inode/linkcount | 仅作 Tier 0 的「瞬间首屏」提示，**绝不可作为权威数据**（[真机实测] 见下方「Spotlight 实测不可用」） |
| `getattrlist` + `ATTR_VOL_*` | 卷级 | `ATTR_VOL_SIZE`、`ATTR_VOL_SPACEUSED`、`ATTR_VOL_FILECOUNT`、`ATTR_VOL_DIRCOUNT`、`ATTR_VOL_CAPABILITIES` | 每卷 1 次 | 无 | **`getattrlistbulk` 不能请求 volume attrs** | 卷信息 + 决定 APFS 记账策略 |
| `fs_snapshot` / `mount_apfs` / `tmutil` | 私有 / CLI | 快照列表 | — | root（部分） | 无公有 API 枚举 APFS catalog | 只用于「提示快照占用空间」，不用于扫描 |

### `getattrlistbulk` 实现细节（来自 XNU 头文件与 man page）

**`struct attrlist`**（`bsd/sys/attr.h`，[XNU 源码](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)）：

```c
struct attrlist {
    u_short     bitmapcount;   /* 必须 = ATTR_BIT_MAP_COUNT (=5)，否则 EINVAL */
    u_int16_t   reserved;      /* 0 */
    attrgroup_t commonattr;
    attrgroup_t volattr;       /* getattrlistbulk 中必须为 0，否则 EINVAL */
    attrgroup_t dirattr;
    attrgroup_t fileattr;
    attrgroup_t forkattr;
};
#define ATTR_BIT_MAP_COUNT 5
```

**返回值信封**：每个目录项是一个 group，起始是一个 **8 字节对齐的 `uint32_t length`**（含自身），紧接着是 `attribute_set_t returned`（5×`u32`，共 20 B）。**注意**：man page 的示例代码其实是把 `length` 当成普通 `uint32_t` 读的，但文档明确写「will always be 8-byte aligned」，所以解析时**必须按指针步进**而不是按 `memcpy` 到压缩 struct：

```c
attribute_set_t returned = *(attribute_set_t *)field;  /* 只有在 8B 对齐时安全 */
```

属性值按「group 顺序（common → vol → dir → file → fork）+ 组内 bit 从低到高」排列，每个属性按自然对齐填充，因此**缓冲区里会有 padding**。`ATTR_CMN_RETURNED_ATTRS` 必须请求；`ATTR_CMN_ERROR`（`uint32_t`）如果请求了，会紧跟在其后。

**`ATTR_CMN_NAME` 是 `attrreference_t`**（不是内联字符串）：

```c
typedef struct attrreference {
    int32_t   attr_dataoffset;  /* 相对于「该 attrreference_t 自身地址」的偏移 */
    u_int32_t attr_length;      /* 含结尾 NUL */
} attrreference_t;
char *name = (char *)name_ref + name_ref->attr_dataoffset;
```

这是一个经典的踩坑点：偏移是相对**引用本身**而不是相对缓冲区开头。

**关键常量（实现时必须逐个对上）**：

| 常量 | 值 | 类型 | 说明 |
|---|---|---|---|
| `ATTR_CMN_NAME` | `0x00000001` | `attrreference_t` | 必须请求 |
| `ATTR_CMN_DEVID` | `0x00000002` | `dev_t` (int32) | 用于 `st_dev` 等价判断、跨卷检测 |
| `ATTR_CMN_OBJTYPE` | `0x00000008` | `fsobj_type_t` (u32) | `VREG=1`、`VDIR=2`、`VLNK=5`（`VNON=0/VBLK=3/VCHR=4/VSOCK=6/VFIFO=7`） |
| `ATTR_CMN_OBJID` | `0x00000020` | `ino64_t` | 不稳定，别当主键 |
| `ATTR_CMN_PAROBJID` | `0x00000080` | `ino64_t` | 父目录 id |
| `ATTR_CMN_MODTIME` | `0x00000400` | `struct timespec` | mtime |
| `ATTR_CMN_FLAGS` | `0x00040000` | `u_int32_t` | 含 `SF_FIRMLINK` |
| `ATTR_CMN_FILEID` | `0x02000000` | `u_int64_t` | **卷内稳定 inode**，硬链接去重用 |
| `ATTR_CMN_PARENTID` | `0x04000000` | `u_int64_t` | |
| `ATTR_CMN_ERROR` | `0x20000000` | `u_int32_t` | 单项错误码 |
| `ATTR_CMN_RETURNED_ATTRS` | `0x80000000` | `attribute_set_t` | 必须请求 |
| `ATTR_DIR_ENTRYCOUNT` | `0x00000002` | `u_int32_t` | 目录直接子项数 |
| `ATTR_DIR_MOUNTSTATUS` | `0x00000004` | `u_int32_t` | `DIR_MNTSTATUS_MNTPOINT=0x1`、`DIR_MNTSTATUS_TRIGGER=0x2` |
| `ATTR_FILE_LINKCOUNT` | `0x00000001` | `u_int32_t` | 硬链接数 |
| `ATTR_FILE_TOTALSIZE` | `0x00000002` | `off_t` | **逻辑**总大小（data fork + resource fork） |
| `ATTR_FILE_ALLOCSIZE` | `0x00000004` | `off_t` | **分配**总大小 |
| `ATTR_FILE_DATALENGTH` | `0x00000200` | `off_t` | data fork **逻辑**长度 ≈ `st_size` |
| `ATTR_FILE_DATAALLOCSIZE` | `0x00000400` | `off_t` | data fork 分配大小 ≈ `st_blocks*512` |
| `ATTR_FILE_RSRCLENGTH` / `ATTR_FILE_RSRCALLOCSIZE` | `0x00001000` / `0x00002000` | `off_t` | 资源分支，老 app 才有 |
| `ATTR_CMNEXT_CLONEID` | `0x00000100` | `u_int64_t` | **APFS clone 家族 id**，需要 `FSOPT_ATTR_CMN_EXTENDED` |
| `ATTR_CMNEXT_CLONE_REFCNT` | `0x00001000` | `u_int32_t` | clone 引用计数 |
| `ATTR_CMNEXT_PRIVATESIZE` | `0x00000008` | `off_t` | 「私有」大小（扣除共享后），APFS 特有能力 |
| `ATTR_VOL_CAPABILITIES` | `0x00020000` | `vol_capabilities_attr_t` | 见下 |

**`FSOPT_*`（`options` 参数）**：

| 常量 | 值 | 作用 |
|---|---|---|
| `FSOPT_PACK_INVAL_ATTRS` | `0x00000008` | 把**不支持**的属性也用默认值返回，依据 `ATTR_CMN_RETURNED_ATTRS` 判断有效性。**建议始终打开**，这样缓冲布局稳定 |
| `FSOPT_REPORT_FULLSIZE` | `0x00000004` | 报告完整 size |
| `FSOPT_ATTR_CMN_EXTENDED` | `0x00000020` | **请求 `ATTR_CMNEXT_*` 的前置条件**（`FSOPT_RETURN_REALDEV=0x200`、`FSOPT_NOFOLLOW_ANY=0x800`、`FSOPT_RESOLVE_BENEATH=0x1000`） |
| `FSOPT_NOFOLLOW` | `0x00000001` | 不跟随符号链接 |

### 文档化的 gotchas（务必逐条处理）

> **⚠️ 以下 12 条中的第 3、11、13 条、以及「实测补充」小节，来自一次 macOS 27.0 (26A428) / Apple Silicon / APFS 真机只读实测**（Python `ctypes` → libc，未创建任何文件）。凡标注 **[真机实测]** 的，是直接测出来的，不是文档推断。

1. **`ATTR_CMN_NAME` 与 `ATTR_CMN_RETURNED_ATTRS` 是强制的**，不请求直接 `EINVAL`。（man page）
2. **不能请求 volume attributes** → `EINVAL`；`bitmapcount != ATTR_BIT_MAP_COUNT` → `EINVAL`。
3. **buffer 太小 → `ERANGE`，而且 `ERANGE` 的门槛是「单个 entry group 的完整长度」，不会返回部分条目。** [真机实测] 一个 1 条目、需要 104 B group 的目录：`bufsize` 40/56/60/64/72/80/88/96 全部 `ERANGE`，104 才成功。⇒ **必须实现「增长缓冲并重试」的循环**，不要假设缓冲一定够。实践上 128 KiB 最优（dumac 作者实测）。
4. **返回 0 后不能继续读**：必须 `lseek(fd, 0, SEEK_SET)` 或重开 fd。（man page）
5. **同一 fd 上混用 `readdir()` 与 `getattrlistbulk()` 行为未定义。**
6. **条目顺序未指定**（有的 FS 字典序，有的不是）——不要依赖顺序。
7. **符号链接返回链接自身**的属性（相当于 `lstat`），不会跟随。
8. **挂载点**：目录本身的 `ATTR_DIR_MOUNTSTATUS` 会是 `DIR_MNTSTATUS_MNTPOINT`，但返回的属性仍来自**底层**文件系统；要拿挂载根目录的属性必须对该挂载点单独调 `getattrlist()`。**这就是 macOS 上做 `-x`（不跨卷）的正确方式**。
9. **firmlink**：`ATTR_CMN_FLAGS` 带 `SF_FIRMLINK`，返回的是 firmlink 本身的属性而不是目标的；`/System/Volumes/Data` 与 `/` 之间就是 firmlink 关系——**不做处理会重复计算整个数据卷**。
10. **`ATTR_CMN_FULLPATH` 在 bulk 调用中不保证有效**（man page 明说）；`ATTR_CMN_FULLPATH` 与 `ATTR_CMN_PARENTID` 对硬链接项也被文档标为不可靠——**不要用它们为硬链接推导规范路径**。
11. **`ATTR_CMN_FILEID`（u64，bit `0x02000000`）是硬链接去重的正确键，[真机实测] 它恒等于 `st_ino`**（`/bin/ls`：`FILEID = 1152921500312607343` == `st_ino`），且在 APFS、`devfs`、甚至 FSKit 卷上都可靠返回。**`ATTR_CMN_OBJID` 在 APFS 上不等于 `FILEID` 且不可用**：[真机实测] 一个真实 `nlink=2` 的文件上 `OBJID.fid_objno = 53167490`，而 `FILEID = 53167465 == st_ino`（`PARENTID` 与 `PAROBJID.fid_objno` 都为 53167488，`fid_generation = 0`）。XNU 头文件也写明：在置了 `VOL_CAP_FMT_64BIT_OBJECT_IDS` 的卷上「**`ATTR_CMN_FILEID` 和 `ATTR_CMN_PARENTID` 是唯一合法的对象 ID**，而 `ATTR_CMN_OBJID`/`OBJPERMIDENT`/`PAROBJID` 的 32 位 `fid_objno` 是未定义的」。`ATTR_CMN_OBJID` 自 macOS 10.13 起弃用，替代品是 `ATTR_CMNEXT_LINKID`。**另注意：inode 只在卷内唯一 ⇒ 去重键必须是 `(dev, FILEID)`。**
12. **非原生文件系统行为**：man page 没有把 `ENOTSUP` 列进 ERRORS，但 Apple 明确说 Yosemite 起 `getattrlistbulk` 在 **VFS 层实现，对所有文件系统可用**（Jim Luther, 2015-01-12：「getattrlistbulk() works on all file systems. If the file system supports bulk enumeration natively, great! If it does not, then the kernel code takes care of it.」）。[真机实测] `getattrlistbulk` 在 26 个挂载点（含 APFS 各变体、`devfs`、FSKit 的 `devicefs`）上**全部成功**。**但网络卷（NFS/SMB）与 FUSE 仍然没有被测到**（测试机上没有挂载），所以「逐条 `getattrlist` 回退」依然必须实现，只是概率很低。

13. **`ATTR_FILE_VALIDMASK` 之外的 file 位会让整次调用 `EINVAL`。** [真机实测] `fileattr = 0xFFFF` → 整调用 `EINVAL`（合法掩码是 `ATTR_FILE_VALIDMASK = 0x000037FF`）。同理 **`ATTR_CMN_GEN_COUNT` 与 `ATTR_CMN_DOCUMENT_ID` 必须配 `FSOPT_ATTR_CMN_EXTENDED`**，否则 `EINVAL`（实测无 flag 失败、有 flag 成功）。

### [真机实测] 补充：三个会直接毁掉解析器的坑

**坑 1：man page 关于「length 字段 = 实际写入字节数」的说法在本机 APFS 上是错的。** man page 原文说前置 length 字段「always represents the length of the data actually copied into the attribute buffer」，并说 `FSOPT_REPORT_FULLSIZE` 会改为报告所需大小。实测（53 字符路径，缓冲预填 `0xAA`）：

| `attrBufSize` | 实际被写入的字节 | length 字段 | 加 `FSOPT_REPORT_FULLSIZE` |
|---|---|---|---|
| 4 | 4 | **88** | 88 |
| 8 | 8 | **88** | 88 |
| 32 | 32 | **88** | 88 |
| 64 | 64 | **88** | 88 |
| 128 | 88 | 88 | 88 |

内核把**完整逻辑大小 88** 写进 length 字段，却只写了 `attrBufSize` 个字节，**`FSOPT_REPORT_FULLSIZE` 完全没有区别**。⇒ **铁律：永远把每一次读取 clamp 到 `attrBufSize`，绝不相信 length 字段代表「已写入字节数」**；用 `ATTR_CMN_RETURNED_ATTRS`（bulk 用 `ERANGE` 重试循环）判断完整性。

**坑 2：`FSOPT_PACK_INVAL_ATTRS` 会改变 `ATTR_CMN_RETURNED_ATTRS` 报告的内容。** 实测请求 `RETURNED|NAME|ERROR|OBJTYPE|FILEID` + `TOTALSIZE|ALLOCSIZE`：

| options | `returned.commonattr` | ERROR 位 |
|---|---|---|
| `0` | `0x82000009` | **清零** |
| `FSOPT_PACK_INVAL_ATTRS` | `0xa2000009` | **置位** |

不带 flag 时这是一个**诚实的子集**；带 flag 时它变成**请求的回显**（因为不支持/无错误的属性也被填了零值）。所以 `returned` 本身是可信的，但**「按固定字段顺序硬编码解析」的方案会在两种 flag 组合下错位**。

**坑 3：字段顺序是 `RETURNED_ATTRS → ERROR → NAME → …`，即 ERROR 在 NAME 之前。** 实测带 `PACK_INVAL` 时 `xarts` 卷上一个 VREG 条目的原始字节：

```
68000000                     group len = 0x68 = 104
090000a2 00000000 06000000 00000000   attribute_set_t: common=0xa2000009 file=0x6
00000000                     ATTR_CMN_ERROR = 0
24000000 28000000            ATTR_CMN_NAME attrref: dataoffset=36, length=40
01000000                     ATTR_CMN_OBJTYPE = 1 (VREG)
10000000 00000000            ATTR_CMN_FILEID = 16
00600000 00000000            ATTR_FILE_TOTALSIZE = 24576
00600000 00000000            ATTR_FILE_ALLOCSIZE  = 24576
<offset 64 起 40 字节的名字载荷, NUL 结尾>
```

不带 `PACK_INVAL` 且无 per-entry 错误时，**NAME 落在偏移 24**，ERROR 整个缺席。（用这个方案写的解析器已在 `/System/Volumes/xarts`、`/usr/share/man/man1`(1179 条)、`/private/etc/apache2`、`/Applications` 上与 `os.listdir` 对拍，**0 处名字不匹配**。）

⇒ **⚠️ 这意味着一篇广为引用的参考实现（dumac 的 `src/main.rs`）字段顺序是反的**：它先读 NAME、再条件读 ERROR。它今天能跑对，只因为 APFS 在无错误时会清掉 ERROR 位、而 dumac 传的 `options = 0`；一旦某个文件系统在不带 `PACK_INVAL` 的情况下报告了真实的 per-entry 错误，它就会把 NAME 的字节当错误码读，该条目从此错位。

**⇒ 铁律：严格按 `ATTR_CMN_RETURNED_ATTRS` 里置位的 bit 从低到高遍历解析，绝不假设固定字段序列。** 这是唯一在「`PACK_INVAL` 有/无」×「有/无 per-entry 错误」四种组合下都正确的方案。可变长载荷（如同时请求 `NAME|FULLPATH`）按引用顺序追加在定长字段之后（实测 `[40:48] NAME ref, [48:56] FULLPATH ref, 56..72 载荷`，group 长度 80）。

### `ATTR_FILE_*` 的语义与 `st_size` / `st_blocks` 的对应

| 你想要的 | getattrlist 属性 | `stat` 等价 |
|---|---|---|
| 逻辑大小（Finder 的「大小」） | `ATTR_FILE_DATALENGTH`（data fork）／`ATTR_FILE_TOTALSIZE`（含 rsrc fork） | `st_size` |
| 磁盘占用（Finder 的「占用空间」） | `ATTR_FILE_DATAALLOCSIZE` ／ `ATTR_FILE_ALLOCSIZE` | `st_blocks * 512` |
| 稀疏造成的差额 | 上面两者之差 | `st_size - st_blocks*512` |

对 ordinary 文件 `ATTR_FILE_DATALENGTH == st_size`；有资源分支的老 app 包（`.app`、旧 Office 文档）`ATTR_FILE_TOTALSIZE > DATALENGTH`。**默认显示应该用「磁盘占用」，并在详情里同时给出「逻辑大小」**，这与 Finder 一致（见「正确性陷阱」）。

### APFS 特有事实（决定你报的数字是否可信）

1. **clone / 共享 extent 会让 `du`/`st_blocks` 重复计数。** [duh](https://github.com/cheapsteak/duh) 的 README 直接说：APFS clone（`clonefile(2)`、`cp -c`，pnpm/uv/Postgres `FILE_COPY`/git worktree 大量使用）**在每个 clone 上都报告完整大小**，一个满是小文件 clone 的目录树可以**高估真实磁盘成本 10–100 倍**。`du`、Finder、DaisyDisk 都是 per-file 求和，因此都中招。
   - **可编程解决**：`ATTR_CMNEXT_CLONEID` 把同族文件归组，一族只在「最小公共祖先」记一次；这是 duh 的做法，也是 ncdu 作者在 ncdu 2 里明确说「想做但做不到」的事（[ncdu2 文章脚注 2](https://dev.yorhel.nl/doc/ncdu2)）。
   - **`ATTR_CMNEXT_CLONEID` 需要 `FSOPT_ATTR_CMN_EXTENDED`**；XNU 头文件里 `ATTR_CMNEXT_CLONEID = 0x100`。duh README 记录了一个矛盾：「Apple 的 headers/docs 暗示是 `0x40`，但在当前 macOS 上实证是 `0x100`」。我核对了 [XNU main 的 `attr.h`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)：`0x40` 是 `ATTR_CMNEXT_REALDEVID`，`0x100` 才是 `ATTR_CMNEXT_CLONEID`，与 duh 的实证一致。**结论：用 `0x100`，并跑一个自检（造一个真 clone 与一个真 copy，比较 clone id）**。
   - **⚠️ clone 属性的可用性随 macOS 版本变化，所以要按运行时探测而不是版本号判断：**（XNU `attr.h` 跨 tag diff 得出）`ATTR_CMNEXT_PRIVATESIZE` 首次出现在 `xnu-4570.1.46`（**10.13**）；`CLONEID`/`EXT_FLAGS`/`RECURSIVE_GENCOUNT` 在 `xnu-7195.50.7.100.1`（**11**）；`CLONE_REFCNT`/`ATTRIBUTION_TAG` 在 `xnu-10002.1.13`（**14**）；`VOL_CAP_FMT_CLONE_MAPPING` 常量在 `xnu-11215.1.10`（**15**）才出现，**但本机 macOS 27 并没有 advertise 它**。⇒ **先在运行时试读 `PRIVATESIZE`/`CLONEID` 一次，成功就用，失败就退回「按分配大小求和 + UI 提示」。** 另注：`ATTR_CMNEXT_CLONE_REFCNT` **只统计完整 clone**，不是硬链接引用计数，不能当硬链接用。
   - **怎么探测卷能力**：`getattrlist(ATTR_VOL_CAPABILITIES)` 返回 `vol_capabilities_attr_t`，含 `capabilities` 与 `valid` 两个 `vol_capabilities_set_t[4]`（`FORMAT`/`INTERFACES`/`USER`/`INT` 四组）。相关位：`VOL_CAP_INT_CLONE=0x00010000`、`VOL_CAP_FMT_SHARED_SPACE=0x00800000`、`VOL_CAP_FMT_VOL_GROUPS=0x01000000`、`VOL_CAP_FMT_SPARSE_FILES=0x00000040`、`VOL_CAP_FMT_DECMPFS_COMPRESSION=0x00010000`、`VOL_CAP_FMT_DIR_HARDLINKS=0x00040000`、`VOL_CAP_INT_READDIRATTR=0x00000008`。**⚠️ 不要用 `VOL_CAP_FMT_CLONE_MAPPING`（`0x04000000`）作为 clone 能力的门槛 —— 我最初的判断是错的。** [真机实测] 本机 macOS 27 的 APFS 卷上 `FORMAT` 能力位是 `0x019B6EDF`、`valid` 是 `0x07FFFFFF`，**`CLONE_MAPPING` 是清零的**，而 `ATTR_CMNEXT_CLONEID` / `CLONE_REFCNT` / `PRIVATESIZE` **全都正常返回值**（头文件注释里也说 `CLONE_MAPPING` 才带「extended directory statistics, for fast directory sizing」，但显然它不是 clone 属性的必要条件）。⇒ **正确的门槛是 `VOL_CAP_INT_CLONE`，或者干脆「试读一次 `PRIVATESIZE` 看是否成功」。**
   - **[真机实测] clone 的高估幅度可以被量出来，而且正确的度量是 `PRIVATESIZE` 而不是自己做 LCA 归账。** 本机找到三组各 **11 个文件共享同一 `CLONEID`** 的真实 clone 家族（iOS Simulator 的 `com.apple.SharedWebCredentials` 缓存），每个文件 `ALLOCSIZE=4096`（或 8192）、`PRIVATESIZE=0`、`CLONE_REFCNT=11`、`flags=0x67`，但 **FILEID 与路径各不相同**。33 个文件 `sum(ALLOCSIZE)=225,280 B`，真实成本只有 `4,096+8,192+8,192 = 20,480 B` ⇒ **`du`/`st_blocks` 高估 11×**。整树实测：

     | 树（截断遍历） | Σ `ALLOCSIZE` | Σ `PRIVATESIZE` | 高估 |
     |---|---|---|---|
     | `~/Library/Developer/CoreSimulator`（80,003 文件） | 4,030,910,464 | 3,210,170,368 | **820,740,096（20%）**；1,131 文件带 `EF_MAY_SHARE_BLOCKS`；146 个共享 clone-id |
     | `/Applications`（60,050 文件） | 8,994,967,552 | 8,367,157,248 | **627,810,304（7%）** |
     | `~/Library/Application Support`（60,023 文件） | 3,309,969,408 | 3,309,969,408 | 0 |
     | `/nix/store`（30,012 文件） | — | — | 0 个 clone |

     ⇒ **`ATTR_CMNEXT_PRIVATESIZE` 的语义就是「没有被 clone 或 snapshot 困住、删除会立刻释放的字节数」（XNU 头文件原文）**，直接读它即可，**比 duh 的 SQLite + 最小公共祖先归账简单一个数量级**。
   - **两个 caveat**：① **[真机实测] 密封 System 卷上 `PRIVATESIZE` 对一切都返回 0**（`/bin/ls`、`/usr/lib/dyld`、`/usr/bin/clang`），因为 OS 快照钉住了这些块 ⇒ **不要把它读成「可以随便删」**；② **`EF_MAY_SHARE_BLOCKS` 会过度报告**：实测有文件带该标志却 `PRIVATESIZE == ALLOCSIZE` 且 `CLONE_REFCNT == 1`（实际没共享）。**强信号是 `EF_SHARES_ALL_BLOCKS`（`0x40`），不是 `MAY_SHARE`；`CLONE_REFCNT` 对未共享文件也是 1，只作参考。** 完整 `EF_*` 位：`EF_MAY_SHARE_BLOCKS 0x1`、`EF_NO_XATTRS 0x2`、`EF_IS_SYNC_ROOT 0x4`、`EF_IS_PURGEABLE 0x8`、`EF_IS_SPARSE 0x10`、`EF_IS_SYNTHETIC 0x20`、`EF_SHARES_ALL_BLOCKS 0x40`。
   - **陷阱：`ATTR_VOL_ATTRIBUTES.nativeattr` 在 APFS 上低报。** [真机实测] `nativeattr.fileattr = 0x0000062F`（不含 `RSRCLENGTH`/`RSRCALLOCSIZE`），但 APFS **确实**返回它们（值 0），`returned.fileattr = 0x3006`。**不要用 `nativeattr` 做支持性判断——用 `ATTR_VOL_CAPABILITIES` + `ATTR_CMN_RETURNED_ATTRS`。**
2. **APFS 快照会钉住数据，而且「快照占多少」没有公有 API。** [真机实测] 本机 `tmutil listlocalsnapshots /System/Volumes/Data` 为**空**（没有本地 Time Machine 快照），但 `diskutil apfs listSnapshots /` 显示密封 OS 快照存在（`Name: com.apple.os.update-…`、`XID: 7399026`、`Purgeable: No`，注明「This snapshot limits the minimum size of APFS Container disk3」）。**`diskutil apfs listSnapshots` 的输出里没有任何 size 字段**，也没有公有 API 能拿到快照大小：`fs_snapshot` 有 `/usr/include/sys/snapshot.h` 与 `SYS_fs_snapshot = 518`，但**没有 man page、没有公开的 `ATTR_SNAP_*` 常量**，`fs_snapshot_list` 的属性词表未文档化，而且它也不枚举文件。⇒ **「删掉这个快照能释放多少」通过公有 API 无法回答**；实用做法只能是 ① 在 `tmutil thinlocalsnapshots` 前后 diff `df`，或 ② 挂载快照并用 `PRIVATESIZE` 走一遍。`tmutil thinlocalsnapshots <mount> [purge_amount] [urgency]` 的存在本身就说明本地快照占空间。**并且没有公有 API 可以枚举 APFS catalog，所以 macOS 上不存在「比 MFT 更快」的整卷枚举。** 另：`ATTR_CMNEXT_RECURSIVE_GENCOUNT` 只读且只对标记过的目录非零（[真机实测] `ATTR_CMNEXT_SETMASK == 0`，**没有公开 setter**），是变更检测辅助而非 size oracle；`ATTR_DIR_ENTRYCOUNT` 在 APFS 上确实返回，但 man page 警告它在非 HFS+ 卷上「usually expensive」。**`diskutil` 用十进制 GB**：「Data Capacity Consumed 875.0 GB」与 `du`/`df` 的「815 GiB」是**同一个数**。
3. **APFS 容器共享空间**：`df` 在 APFS 上报告的是 container 的空闲空间，多个卷共享。`VOL_CAP_FMT_SHARED_SPACE` 就是标记。卷级 `ATTR_VOL_SPACEUSED` 比 `df` 更贴近单卷用量。
4. **压缩文件（DECMPFS）是活的，检测方式反直觉。** [真机实测] `/bin/ls`、`/usr/bin/clang`、`/usr/bin/true` 的 `st_flags = 0x80020` = `UF_COMPRESSED(0x20) | SF_RESTRICTED`（`ls -lO` 显示 `restricted,compressed`）。`/bin/ls` 逻辑 252,512 vs 分配 57,344（**4.4×**）；`/usr/bin/clang` 200,560 vs 20,480（**9.8×**）。**检测方式是查 `ATTR_CMN_FLAGS`/`st_flags` 里的 `UF_COMPRESSED`，因为 `com.apple.decmpfs` xattr 被 `decmpfs_hides_xattr` 从 `xattr(1)` 里隐藏了——用 `xattr` 检测压缩会漏。** 真实占用 = `st_blocks*512` = `ATTR_FILE_ALLOCSIZE`。
5. **稀疏文件**：`VOL_CAP_FMT_SPARSE_FILES` 与 `VOL_CAP_INT_PUNCHHOLE` 在 APFS 上都置位。**[真机实测] `EF_IS_SPARSE`（0x10）不总是被置位**，所以要**信 `ALLOCSIZE << TOTALSIZE` 这个事实而不是那个标志**。本机真实例子：`dyld_shared_cache_arm64e.symbols` 3,768,320 逻辑 vs 790,528 分配；`QuartzCore …/default.metallib` 179,246,784 vs 50,470,912（28%）。另：`S_BLKSIZE` **恒为 512，且不是 `stat.st_blksize`**（后者是「最佳 I/O 大小」）；被调研的工具全部硬编码 512。`st_blocks` 在 APFS 上是「该文件自己的分配字节 / 512」，不含共享折扣（duh README：一个「64 GB」的稀疏 `Docker.raw` 实占 9 GB 就记 9 GB）。
6. **目录硬链接**：APFS 支持（`VOL_CAP_FMT_DIR_HARDLINKS`）但实际已不用（Time Machine 改用快照 + clone），遍历时仍不能假设「目录只有一条父链接」。另注意 **`ATTR_FILE_LINKCOUNT` 只对非目录返回**（man page：「Requested file attributes are not returned for file system objects that are directories」），目录的链接数要走 `ATTR_DIR_LINKCOUNT`；`ATTR_CMN_FULLPATH` 与 `ATTR_CMN_PARENTID` 对硬链接项也被文档标为不可靠。
7. **macOS Full Disk Access（TCC）**：非授权进程读 `~/Library`、`~/Documents`、`~/Desktop`、`/Volumes/*` 会拿到 `EPERM`。**这不是「空目录」**，必须把 `EPERM` 记成 `unknown/partial` 而不是 0，否则用户会以为 `~/Library` 是空的。（Apple 官方文档：[Protecting user data with App Sandbox / Full Disk Access](https://developer.apple.com/documentation/security/protecting-user-data-with-app-sandbox)；本项目 `REQUIREMENTS.md` 的 R7.4 已经规划了引导。）

### [真机实测] Spotlight 为什么不能当 size oracle

之前我只给了「不可靠」的定性判断；真机实测把它钉成了数字：

| 项 | 实测结果 |
|---|---|
| `kMDItemFSSize` | **是逻辑大小（`st_size`）**。`/usr/lib/dyld` → 4,129,840 而 `st_blocks*512` = 1,556,480；`/bin/ls` → 252,512 vs 57,344；`Docker.raw` → 1,099,511,627,776 vs 10,397,396,992 |
| `kMDItemFSSizeInBytes` | **不存在**（`mdls` 返回 `(null)`） |
| `kMDItemPhysicalSize` | 存在且**有值时**等于 `st_blocks*512`，但对完全共享的 Chrome framework clone、`~/.zshrc`、Android 稀疏镜像、`/usr/lib/dyld`、`/bin/ls`、`/usr/bin/*` **都是 `(null)`** |
| clone / snapshot 信息 | **完全没有** |
| 索引覆盖率（`mdfind -onlyin <dir> '*'` vs `find`） | `~/Documents`：35,799 / 782,743 = **4.6%**；`~/Library/Application Support`：13%；`~/Downloads`：16%；**`/System/Library/Frameworks`：0 / 42,456 = 0%**；`/usr`：~2% |
| System 卷 | **完全没有被索引**（`mdls /System/Library/Frameworks/Foundation.framework/Foundation` → "could not find"），尽管 `mdutil -s /` 说 "Indexing enabled" |
| 隐藏文件 | **不在索引里**（`mdfind 'kMDItemFSName == ".zshrc"'` → 0 结果，但文件存在且 `mdls` 能报出 size） |
| 查询语法 | `mdfind -onlyin "$HOME" '*'` 可用（80,673 条），但 `kMDItemFSName == "*"` 与 `kMDItemContentType == "public.item"` 都返回 **0** |
| `.noindex` / `.metadata_never_index` | **不是任何随系统发布的 man page 里的文档**（`mdutil`/`mdfind`/`mdls`/`mds`/`mdimport` 都没有）——属于社区惯例，**未文档化、随版本变化** |
| 其他 | `-onlyin` **不是硬过滤**（对 `-onlyin /System` 会返回 `/Library/...`）；返回规范化/无 firmlink 路径（`/etc/hosts` → `/private/etc/hosts`）；bundle 内部条目不一致；能复现索引陈旧 |

⇒ **结论：`getattrlistbulk(2)` 在任何维度上都严格优于 Spotlight。Spotlight 只能用于「快速找候选路径」，绝不能用于算大小。**

### [真机实测] 一个反驳「getattrlistbulk 永远最快」的独立 benchmark

Thomas Tempelmann（Find Any File 作者）2019 年测了**只要名字**时的目录读取耗时（秒；[来源](https://blog.tempel.org/2019/04/dir-read-performance.html)）：

| 卷 | `contentsOfDirectoryAtURL` | `getattrlistbulk` | `opendir`/`readdir` |
|---|---|---|---|
| SSD APFS | 10.6 | 6.8 | **3.2** |
| SSD HFS+ | 2.8 | 2.26 | 2.47 |
| 10.14 APFS | 12 | 10 | **8** |
| SSD NTFS | 6 | 6 | **4.7** |
| NAS via AFP | 2.5 | **2.14** | 2.7 |
| NAS via SMB | 15 | 15 | **5.7** |

⇒ **只要名字时，`readdir()` 在 APFS / NTFS / SMB 上都明显快于 `getattrlistbulk`**；`fts` 在本地 HFS+/APFS 上最快；`getattrlistbulk` 只在 AFP 上胜出。**但一旦需要额外属性，`readdir()+lstat()` 就变成最慢的，而 `getattrlistbulk` 几乎不受影响**——而磁盘分析器**恰恰需要元数据**，所以对本项目而言 `getattrlistbulk` 仍然正确。**这是一个必须诚实标注的限定条件：我们的选择理由不是「bulk 永远更快」，而是「我们是需要 size 的那一类用法」。**

### 成熟工具在 macOS 上怎么做

| 工具 | 机制 | 并行 | APFS clone 感知 | 备注 |
|---|---|---|---|---|
| BSD `du` | `fts(3)` / `lstat` 逐条 | 单线程 | 否 | 2.570 s / 409 k 文件（M1 Pro 热缓存） |
| GNU `du` | 同上 | 单线程 | 否 | gdu 实测比 diskus 慢 6.8×（冷） |
| `diskus` | Rust + POSIX | 是 | 否 | M1 Pro 上比 dumac 慢 2.58× |
| `dust` | Rust + `jwalk`（walker threads 默认 = CPU 数，`-T` 可调） | 是 | 否 | 对 NFS 高延迟卷建议调高 `-T` |
| `dua` | Rust，`dua-core`（jwalk 的后继） | 是 | 否 | |
| `gdu` | Go，每目录 goroutine | 是 | 否 | 作者明确说是为 SSD 设计的 |
| `ncdu` | Zig/C，`openat` 家族 | **否**（多线程是待办） | 否 | 明确说 clone/reflink 共享做不了 |
| **`dumac`** | Rust + tokio(64 并发) + `libc::getattrlistbulk` | 是 | 否（按 inode 去重硬链接） | **0.52 s / 409 k 文件（热）**，系统调用占 91% 时间 |
| **`duh`** | Rust + `ATTR_CMNEXT_CLONEID` + SQLite | 流式，内存有界 | **是** | 用 SQLite 存 ~4 M 文件；算 `freeable(dir)`（`rm -rf` 真正释放多少） |
| DaisyDisk / GrandPerspective / OmniDiskSweeper | GUI，逐条 stat | 部分 | 否（DaisyDisk 只做 UI 层提示） | 与 `du` 同源误差 |
| `diskr` / `disky` | Rust，`getattrlistbulk` | 是 | 否 | 同 dumac 思路 |

**结论**：macOS 上没有「整卷目录册」可用，**`getattrlistbulk` 就是天花板**。要把 2 M 文件压进 60 s，靠的是 ① bulk 系统调用，② 目录级并行（64 左右并发），③ 只在需要时才请求重属性。**clone 正确性是差异化竞争力**，但需要 `FSOPT_ATTR_CMN_EXTENDED` + clone 家族聚合，代价是一张 `clone_id → 最小公共祖先` 的映射表。

---

## Windows / NTFS

### 候选 API 对照表

| API | 平台 / 系统调用 | 每个条目返回什么 | 每条目系统调用次数 | 需要的权限 | 已知限制 | 适用性 |
|---|---|---|---|---|---|---|
| **直读 `$MFT`**（`CreateFileW("\\\\.\\X:", GENERIC_READ)` + 大块 `ReadFile`） | Windows / NTFS | `FILE` 记录：`$STANDARD_INFORMATION`(0x10, 时间戳/属性/owner)、`$FILE_NAME`(0x30, **父引用号 + 名字**)、`$DATA`(0x80, **逻辑/分配大小**)、`$ATTRIBUTE_LIST`(0x20, 分片记录) | ≈0（与文件数无关的顺序读） | **管理员**（`GENERIC_READ` 打开卷）；或 `SeBackupPrivilege` + `FILE_FLAG_BACKUP_SEMANTICS` | 需要自己解析 NTFS 结构；`$ATTRIBUTE_LIST`/分片记录、稀疏/压缩、ADS 都要处理；卷被挂载时读原始设备要注意一致性（只读安全） | **Tier 1a 最快** |
| `FSCTL_ENUM_USN_DATA` | `DeviceIoControl` on `\\.\X:` | `USN_RECORD_V2/V3/V4`：`FileReferenceNumber`、**`ParentFileReferenceNumber`**、`FileName`(UTF-16)、`FileAttributes`、`TimeStamp`、`Reason`、`SecurityId` | ≈0（每 1 MiB 缓冲回 ~数千条记录） | **管理员**（卷 handle 需 `FILE_READ_DATA`） | **⚠️ 不含文件大小**！只能拿到名字 + 父引用 + 属性；要大小必须再读 MFT 或 `NtQueryInformationFile`。另：非 NTFS 卷不支持 | Tier 1 的「名字/父节点」部分 |
| `FSCTL_GET_NTFS_FILE_RECORD` | `DeviceIoControl` | 单个 MFT 记录的原始字节 | **1 / 文件** | 管理员 | 逐条 IOCTL，慢；只适合补漏 | 不建议主路径 |
| `FSCTL_GET_NTFS_VOLUME_DATA` | `DeviceIoControl` | `NTFS_VOLUME_DATA_BUFFER`：`BytesPerSector`、`BytesPerCluster`、`MftStartLcn`、`Mft2StartLcn`、`MftValidDataLength`、**`BytesPerFileRecordSegment`**（通常 1024） | 1 / 卷 | 管理员 | 给出 MFT 的物理位置与记录大小 → 直读 `$MFT` 的入口 | **Tier 1a 的必备前置调用** |
| **`NtQueryDirectoryFile(..., FileIdBothDirectoryInformation, ...)`** | `ntdll` | `FILE_ID_BOTH_DIR_INFORMATION`：`EndOfFile`(逻辑)、`AllocationSize`(分配)、`FileId`(8 B 引用号)、`FileAttributes`、4 个时间戳、`FileName`(UTF-16)、`ShortName` | **每次调用可回一整缓冲区（几十~几百条）**；每目录 1–3 次 | **无需特殊权限**（文档原文：「No specific access rights are required to query this information」） | 需要 `NtQueryDirectoryFile` 的 `RESTART_SCAN` 语义；`FileId` 不是 `FILE_ID_128`（ReFS 需要 `FileIdExtdBothDirectoryInformation`）；ReFS 不支持 `FileId`（要 128 位） | **Windows 上最重要的 Tier 2 路径** |
| `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` | `kernel32` | 返回格式与上一行**相同**的 `FILE_ID_BOTH_DIR_INFO` | 同上 | 无 | 由 `kernel32` 转发到 `NtQueryDirectoryFile`，**`FileIdBothDirectoryInfo` 这个 class 实际上没有出现在 Win32 公开文档的 `FILE_INFO_BY_HANDLE_CLASS` 列表里**（社区长期依赖未文档化行为）——**这是一个证据薄弱点，必须写回退** | 有风险，但广泛使用 |
| `FindFirstFileExW(FindExInfoBasic, FindExSearchNameMatch)` | `kernel32` | `WIN32_FIND_DATAW` | **1 / 条目** | 无 | `FindExInfoBasic` 跳过 8.3 短名（显著快）；仍然 per-entry | **Tier 3 兜底**（不提权时的现实选择） |
| `USN journal`（`FSCTL_READ_USN_JOURNAL`） | `DeviceIoControl` | `USN_RECORD_V2/V3` | 批量 | 管理员（或读权限足够时有限） | 用于**增量更新**而非首扫 | Tier 0 增量 |
| `ReadDirectoryChangesW` | `kernel32` | 变更通知 | — | 无 | 缓冲区溢出会丢事件；系统重启期间的变化会漏 | 小范围监控 |

### 为什么「直读 MFT」能几秒扫完整卷

MFT 是 NTFS 的「全卷 inode 数组」，**每条记录 1 KiB**（`BytesPerFileRecordSegment`，由 `FSCTL_GET_NTFS_VOLUME_DATA` 给出），每条 `FILE` 记录里已经有：

- `$FILE_NAME` 属性：**父目录的 MFT 引用号 + 本节点名字 + 名字空间（POSIX/Win32/DOS）**
- `$STANDARD_INFORMATION`：创建/修改/MFT 修改/访问时间、文件属性位、owner id、USN
- `$DATA`（匿名流）：`RealSize`（逻辑大小）、`AllocatedSize`（分配大小）、稀疏/压缩标志
- `$ATTRIBUTE_LIST`：当记录放不下时指向分片

于是「扫全卷」变成：**一次顺序读 ≈ `MftValidDataLength` 字节 + 纯 CPU 解析**，然后**用 `ParentFileReferenceNumber` 建树**。2 M 文件 ≈ 2–2.5 GB MFT，顺序读 1 s 级，解析是唯一的大头。

**解析吞吐实测锚点**：[`mft` crate](https://github.com/omerbenamram/mft) 的 `PERF.md` 记录，在 13 MB 的样例 MFT 上 `mft_dump` 端到端（解析 + JSONL 序列化）baseline 是 **95.94 ms**，四步优化后到 **57.81 ms**（Mac15,6 / 11 core / macOS 26.2 / rustc 1.92）。即 **~135 MB/s → ~225 MB/s 单线程（含 JSON 输出）**。保守取 **200 MB/s/核**，2–2.5 GB MFT ⇒ **单线程 10–12 s**；把 MFT 按记录边界切成 4–8 段并行解析 ⇒ **2–3 s**。加顺序读 ~1–2 s，**总计 4–6 s**，与 WizTree 的量级一致。

**另一组实测（把「为什么必须直读 MFT」钉死）**：在 700 k 文件的 NTFS `C:\` 上，用 **64 KB 输出缓冲**循环调用 `FSCTL_ENUM_USN_DATA`：

- **21 s ⇒ ~33,300 文件/s**；总共只传了 **84 MB ⇒ 4 MB/s**。
- 换成 `FILE_FLAG_SEQUENTIAL_SCAN`、`FILE_FLAG_RANDOM_ACCESS`、`FILE_FLAG_NO_BUFFERING`，**结果完全一样**（「same result: 21 seconds to read」）。
- 同一份报告里，另一个「不使用 `FSCTL_ENUM_USN_DATA`、而是 low-level NTFS parsing」的工具 **< 5 s 索引完 `C:\` ⇒ > 140,000 文件/s**。

（[Stack Overflow 45179671](https://stackoverflow.com/questions/45179671/)）**4 MB/s 的结论**：瓶颈**不是带宽、也不是访问模式提示**，而是每次 `DeviceIoControl` 只回一缓冲、必须往返一次 ⇒ **在 Windows 上真正的旋钮是「输出缓冲大小」和「是否直读 MFT」，不是 readahead 标志**。

**Why not 用 `FSCTL_ENUM_USN_DATA` 一条路走到底？** 因为 **USN 记录里没有 size**。它给名字 + 父引用 + 属性 + 时间戳，是最便宜的「建树骨架」，但大小必须从 MFT 记录或 `NtQueryInformationFile` 再取一次——每文件一次调用，等于把省下来的全还回去。**所以 Windows 首选是直读 `$MFT`，`FSCTL_ENUM_USN_DATA` 作为「只想建骨架、不关心大小」或 MFT 读取被拒时的替代。** 这个「USN 无 size」的事实我从 Microsoft 的 [`USN_RECORD_V2` 文档](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2)字段列表逐一核对确认（字段只有 `RecordLength/MajorVersion/MinorVersion/FileReferenceNumber/ParentFileReferenceNumber/Usn/TimeStamp/Reason/SourceInfo/SecurityId/FileAttributes/FileNameLength/FileNameOffset/FileName`）。

**路径重建**：MFT/USN 给的是「父引用号」，不是路径。做法是：

1. 第一遍把 `(FileReferenceNumber → 父引用号, 名字, 大小, 类型)` 收进一张 `HashMap<u64, ...>` 或按引用号排序的 `Vec`（**MFT 记录号天然有序**，可以直接用 `Vec<Option<Node>>` 索引，省掉哈希表）。
2. 第二遍从根（记录号 5）深度优先下发完整路径，或自底向上按父指针累积大小。
3. **MFT 引用号 = 低 48 bit 记录号 + 高 16 bit 序列号**。比较父引用时**只比较低 48 bit**，否则文件被删除重建后序列号变了会对不上。
4. 硬链接：一个文件有多个 `$FILE_NAME` 属性（多个父 + 多个名字）。**必须把一条 MFT 记录展开成 N 个目录项，但大小只记一次**（按记录号去重），这正是 WizTree 官网强调的「correctly handles hard linked files (doesn't count them more than once)」（[来源](https://diskanalyzer.com/)）。

### 权限、提权与回退

- 打开 `\\.\C:` 需要管理员；[`ntfs` crate README](https://github.com/colinfinck/ntfs) 原文：「to a partition (like `\\.\C:`, on Windows only with administrative privileges)」。
- 一个真实产品的架构讨论（Microsoft Q&A）把这件事讲得非常清楚：标准用户用 `FindFirstFileExW` 走目录，管理员才读 MFT；**「the MFT path is purely a speed optimization (the technique WizTree/TreeSize use), and opening the raw volume needs an administrator elevation」**，并给出了「主进程保持普通权限 + 按需 UAC 启动一个独立的提权 helper + 命名管道 IPC 回传」的模式（[来源](https://learn.microsoft.com/en-us/answers/questions/5944062/msix-win32-app-reading-raw-c-mft-only-when-run-as)）。这正好对得上本项目的需求：**Sift 不应该 `requireAdministrator`，而应该在需要快速全盘扫描时提示一次 UAC，起 helper，扫完退出 helper。**
- `SeBackupPrivilege`：可以让进程绕过 ACL 读文件，但对卷 handle 的 `FILE_READ_DATA` 通常仍需提权；**我没有找到一个权威来源说明「只开 `SeBackupPrivilege` 不提权就能 `FSCTL_ENUM_USN_DATA`」，因此这里标注为不确定**，实现时应实测（`AdjustTokenPrivileges` 后 `CreateFileW` 是否成功）。
- **非 NTFS 卷（FAT32/exFAT/ReFS）**：`FSCTL_ENUM_USN_DATA` 明确「The volume must be NTFS」；ReFS 有 USN journal 但 `FileId` 语义不同（ReFS 用 128 位 `FILE_ID_128`，需要 `FileIdExtdBothDirectoryInformation` / `FILE_ID_EXTD_BOTH_DIR_INFORMATION`，[参考](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_id_both_dir_information)）。**一律走 Tier 2/3。**
- **网络盘/映射驱动器**：MFT/USN 完全不可用 → `FindFirstFileExW` 或 `NtQueryDirectoryFile`。

### Rust 生态

| crate | 最新版 | 许可证 | 说明 |
|---|---|---|---|
| [`ntfs`](https://crates.io/crates/ntfs)（Colin Finck） | **0.4.0** | MIT OR Apache-2.0 | `no_std` + `alloc` 的底层 NTFS 库，**100% safe Rust，无 `unsafe`**；可打开镜像或 `\\.\C:`（需管理员）。**目录索引 O(1) 顺序迭代**、按 `$Upcase` 大小写不敏感查找。**不支持**：写、缓存、压缩、加密、日志、**reparse point**、安全描述符。⇒ 可用于 Tier 1a 的解析内核，但 reparse point / 压缩要自己补。 |
| [`mft`](https://crates.io/crates/mft)（omerbenamram） | **0.7.0** | MIT/Apache | **纯粹解析 MFT 快照**（不碰驱动），100% safe Rust，跨平台，带 `mft_dump` CLI、JSON/CSV 输出、resident stream 提取。**解析一个已有的 `$MFT` 二进制**是它的强项；把「从 `\\.\C:` 读出 `$MFT`」这步留给你。附 `PERF.md` 性能方法论。 |
| `ntfs-core` | 0.9.8 | — | 更偏 forensics 的分层实现 |
| `ntfs-forensic` | 0.8.5 | — | |
| [`windows`](https://crates.io/crates/windows) / `windows-sys` | **0.62.2 / 0.61.2** | MIT OR Apache-2.0 | 官方 Win32 绑定，`DeviceIoControl`、`CreateFileW`、`GetFileInformationByHandleEx`、`NtQueryDirectoryFile` 都在。**需要 `unsafe` FFI**（但 API 由官方元数据生成，比手写 `extern "system"` 可靠）。 |
| `usn-journal-rs` | 0.4.1 | — | USN journal 读取的现成封装（社区小项目，需自行审计） |

**纯 Rust 可行性**：**是**。`ntfs` + `mft` 都是 100% safe Rust，MFT 解析不需要 C 库。唯一必须 FFI 的是「打开卷 / DeviceIoControl / NtQueryDirectoryFile」这几个 Win32 调用（用 `windows` crate 封装）。**不需要 C/C++ 依赖，也不需要额外工具链。**

**建议**：首扫走 `FSCTL_GET_NTFS_VOLUME_DATA` → 自己按簇读 `$MFT` → 用 `mft` crate 解析（比 `ntfs` 更贴「批量解析」场景，且它的 `FlatMftEntryWithName` 已经处理 `$FILE_NAME`）；增量走 `FSCTL_READ_USN_JOURNAL`（`windows` crate 直接调）。`ntfs` crate 留作「按需读取某个目录索引」的备用。

---

## Linux

### 候选 API 对照表

| API | 平台 / 系统调用 | 每个条目返回什么 | 每条目系统调用次数 | 需要的权限 | 已知限制 | 适用性 |
|---|---|---|---|---|---|---|
| **`getdents64(2)`** | Linux 2.4+ | `struct linux_dirent64 { ino64_t d_ino; off64_t d_off; unsigned short d_reclen; unsigned char d_type; char d_name[]; }` —— **有 `d_type`（`DT_DIR`/`DT_REG`/`DT_LNK`/`DT_UNKNOWN`）但无 size** | **≈0.001–0.002**（32 KiB 缓冲回 ~1000–1500 条） | 目录读权限 | `d_type == DT_UNKNOWN` 时（部分 FS，如某些 XFS 配置、旧 NFS）必须 `statx` 兜底 | **Tier 2 枚举层** |
| **`statx(2)`** | Linux 4.11+（glibc 2.28+） | `struct statx`：`stx_mask`、`stx_mode`、`stx_ino`、**`stx_size`**、**`stx_blocks`**、`stx_nlink`、`stx_mtime`、`stx_dev_major/minor`、`stx_mnt_id`、`stx_subvol`(6.10+)、`stx_attributes`(`STATX_ATTR_COMPRESSED`/`IMMUTABLE`/`ENCRYPTED`/`MOUNT_ROOT`) | **1** | 路径搜索权限 | **没有 mainline 的 `STATX_BATCH`**（见下）；`statx` 在某些内核/FS 上比 `stat` 慢（kernel.dk 上有 `statx() was significantly slower than stat()` 的提交记录）；`mask` 请求多了反而慢，应只请求 `STATX_SIZE\|STATX_BLOCKS\|STATX_TYPE\|STATX_NLINK\|STATX_INO` | **Tier 2 元数据层** |
| `fstatat(2)` / `stat(2)` | 所有 | 同上（`struct stat`） | 1 | 同上 | 比 `statx` 少字段（无 `stx_mnt_id`/`stx_subvol`/`STATX_ATTR_*`） | 兜底 |
| `io_uring` `IORING_OP_STATX` | Linux 5.6+ | 同上 | 批量提交 | 无 | 每个 SQE 仍是一个 statx，但省掉 syscall 进入/退出；io_uring 在 2026 仍是「有收益但增加复杂度」 | 可选优化 |
| `io_uring` `IORING_OP_GETDENTS` | **❌ 未进入 mainline** | — | — | — | 2021 与 2023 两组 patch（[LKML v4 2021](https://lkml.org/lkml/2021/2/19/288)、[patchew 2023 v29](https://patchew.org/linux/20230825135431.1317785-1-hao.xu@linux.dev/20230825135431.1317785-30-hao.xu@linux.dev/)）都**没有合入**。我核对了 [io_uring_enter(2) man page](https://man7.org/linux/man-pages/man2/io_uring_enter.2.html) 的 opcode 全表，**没有 GETDENTS**。⇒ **不要设计依赖它** | ❌ |
| `open_by_handle_at(2)` / `name_to_handle_at(2)` | Linux 2.6.39+ | 文件句柄 | 1/文件 | 打开句柄需要 **`CAP_DAC_READ_SEARCH`** | 需要先有 handle；对普通用户不可用；不解决「如何快速枚举」 | ❌（除非做已提权的整卷工具） |
| **`XFS_IOC_FSBULKSTAT`** | XFS（`<xfs/xfs_fs.h>`） | `struct xfs_fsop_bulkreq { __u64 *lastip; __s32 count; void *ubuffer; __s32 *ocount; }` → 数组 `struct xfs_bstat { bs_ino, bs_mode, bs_nlink, bs_uid, bs_gid, bs_rdev, bs_blksize, bs_size, bs_atime/mtime/ctime, bs_blocks, bs_xflags, bs_extsize, bs_extents, ... }` | **≈0（批量，与文件数解耦）** | 打开的 XFS 文件系统 fd（普通读即可） | **只有 stat 信息，没有名字**。XFS inode 号里不编码父目录 ⇒ **仍需 `getdents64` 读目录拿「名字 ↔ inode」映射**。已被 `XFS_IOC_BULKSTAT` 取代（[man page](https://manpages.debian.org/testing/xfslibs-dev/ioctl_xfs_fsbulkstat.2.en.html)） | **XFS 的 Tier 1c** |
| **`BTRFS_IOC_TREE_SEARCH` / `_V2`** | btrfs（`<linux/btrfs.h>`） | 直接搜索 btrfs 内部 B 树（`BTRFS_ROOT_TREE_OBJECTID`、`BTRFS_FS_TREE_OBJECTID`…），可枚举 inode item（含 size/nlink/mode）与 dir item（含 name + parent） | ≈0（批量） | 只读打开任意 btrfs 文件 | `TREE_SEARCH_V2` 需要内核 4.7+；接口复杂（key/offset/`sk` 结构）；subvolume/snapshot/reflink 的共享语义要自己处理 | **btrfs 的 Tier 1c**（需谨慎，收益需实测） |
| `FIEMAP`（`FS_IOC_FIEMAP`） | ext4/XFS/btrfs… | 单个文件的 extent 列表（物理块 + 是否共享 `FIEMAP_EXTENT_SHARED`） | 1/文件 | 文件读权限 | **per-file 且昂贵**，只适合「用户点开某个文件看物理分布」 | ❌ 不用于全盘扫描 |
| `FS_IOC_GETFSMAP` | ext4/XFS | 整卷的**物理块映射**（reverse map） | 每次 ioctl 回一批 extent | 需要读文件系统 | 给的是「物理块 ↔ 拥有者」，可以做「重复计算检测」，但**拿不到文件名**，且需要 `CAP_SYS_ADMIN`（部分 FS） | ❌ 仅作诊断 |
| `statmount(2)` / `listmount(2)` | Linux 6.8+ | 挂载信息（比 `/proc/self/mountinfo` 结构化） | 1/挂载 | 无 | 用于枚举挂载点与 `stx_mnt_id` 对应，**不用于文件扫描** | 卷枚举辅助（R2.1） |

### 「`STATX_BATCH`」的澄清

任务描述里提到的 `STATX_BATCH` **在 mainline Linux 中不存在**。我检索到的最接近的东西是：`statx` 支持 `AT_EMPTY_PATH` + `NULL` path（6.11+）与 `IORING_OP_STATX`（5.6+），以及 `statmount`/`listmount`（6.8，面向挂载信息而非文件）。**不要把设计建立在 `STATX_BATCH` 上。**（[statx(2) man page](https://man7.org/linux/man-pages/man2/statx.2.html)，[Linux 6.8 statmount/listmount](https://www.phoronix.com/news/Linux-6.8-statmount-listmount)）

### 结论：Linux 上「du 式并行」基本就够了

从 gdu 的实测（400 k 文件冷缓存 4.5–6.2 s，热缓存 0.27–0.59 s）外推，**普通 `getdents64` + `statx` + 目录级并行，2 M 文件在冷缓存下 23–31 s，在预算内**。整卷目录册（XFS bulkstat / btrfs tree search）在 Linux 上的**收益远小于 NTFS**（因为 NTFS 的一条 MFT 记录同时含名字和大小，而 XFS bulkstat 只有 stat 没有名字，仍要读目录项），且实现风险高。**建议：ext4 老实走 Tier 2；XFS/btrfs 的 bulk 接口列为「后续优化」，不要进第一条主线。**

**fs 特有注意事项**：

- **ext4**：目录是 htree（B 树），`getdents64` 顺序读目录块；**没有任何公有整卷 inode 枚举接口**（`debugfs` 需要 root 且不是 API）。超大目录（单目录 >100 k 项）是唯一需要特殊处理的场景。
- **btrfs**：subvolume / snapshot / reflink 会让「per-file 求和」系统性高估；`stx_subvol`（6.10+）可以区分 subvolume。`du` 在 btrfs 上会重复计算 reflink。
- **XFS**：`bs_blocks` 含 metadata；`bs_extents` 可以给你「碎片数」这个有用的诊断字段。
- **`/proc`, `/sys`, `/dev`, `/run`**：必须默认排除（gdu 的默认 `--ignore-dirs` 就是这五个），否则会扫到伪文件系统并触发大量无意义 I/O。
- **权限**：普通用户扫 `/` 会大量 `EACCES`；`CAP_DAC_READ_SEARCH` 可以绕过（等于 root 读），但**桌面 app 拿它不现实**。老实报 `partial`。

### 成熟工具在 Linux 上怎么做

| 工具 | 机制 | 并行模型 | 备注 |
|---|---|---|---|
| `du` (GNU) | `fts`/`readdir`+`stat`，按 `(dev, ino)` 去重硬链接 | 单线程 | 400 k 文件冷 30.6 s |
| `diskus` | Rust，`readdir`+`stat`，按 inode 分片 HashSet 去重 | 线程池 | **gdu benchmark 里冷/热都最快** |
| `gdu` | Go，**每目录一个 goroutine** | 全并行（默认 = CPU 核数，`-m` 可限） | 明确「为 SSD 设计」；非交互模式用「只记顶层汇总」的省内存分析器，**内存与树大小无关** |
| `pdu` | Rust，`jwalk`（rayon） | rayon | 热缓存比 diskus 略慢 |
| `dua` | Rust，`dua-core`（jwalk 后继） | rayon | 支持渐进式 UI |
| `dust` | Rust，`jwalk`（walker threads，`-T` 可调） | rayon | 对 NFS 建议提高 `-T` |
| `ncdu` | Zig，`openat` 家族 | **单线程** | 3.8 M 文件 162 MB；明确把多线程列为待办 |
| `duc` | C，索引到数据库 | 单线程 | `duc index` 32.9 s（冷），比 du 还慢——**索引持久化不能救首扫** |
| `btdu` | btrfs 专用，采样 B 树并做统计推断 | — | [btdu](https://github.com/CyberShadow/btdu)：不做精确求和而做**概率采样**，是 btrfs reflink/snapshot 场景的另类解 |

---

## 并行与 I/O 策略

### 并行到底有没有用？有，但**只在目录粒度上**

最关键的架构事实来自 `jwalk` 的 README（原文）：

> This crate's parallelism happens at the **directory level**. It will help when walking deep file systems with many directories. **It won't help when reading a single directory with many files.**

所以正确的并行单元是**目录**，不是文件。一个含 200 k 文件的目录，你没法靠多线程把它读得更快（`getdents64` 已经一次回 1000+ 条，`statx` 也只能一条条发）；但一个含 200 k 个目录、每目录 10 个文件的树，可以完美并行。

**`jwalk` benchmark（Linux 源码树，iMac Late 2015；[来源](https://github.com/Byron/jwalk/blob/main/benches/benchmarks.md)）**：

| 变体 | 1 线程 | 2 线程 | 8 线程 | 8 线程加速比 |
|---|---|---|---|---|
| unsorted | 141.66 ms | 88.416 ms | 54.631 ms | 2.59× |
| sorted | 150.89 ms | — | 56.133 ms | 2.69× |
| sorted + metadata | 313.91 ms | — | 86.985 ms | **3.61×** |
| `walkdir`（单线程） | 134.28 / 170.24 / 310.26 ms | — | — | 基准 |

注意「带 metadata」那一行的加速比最高：**每个 `stat` 的等待时间被并行掩盖了**，这正是扫描场景。

**实测并行规模参考（最重要的一张表：并发不是越大越好）**：

ripgrep 作者 BurntSushi 做了一组目前最干净的实验（Chromium 源码树**热缓存**，扫 `-j` 全表；[ripgrep discussion #2472](https://github.com/BurntSushi/ripgrep/discussions/2472)）：

**`rg -uuu --files`（无过滤），394,576 文件，热缓存：**

| 线程 | 1 | 2 | **4** | 8 | 12 | 16 | 32 |
|---|---|---|---|---|---|---|---|
| real | 0.255 s | 0.203 s | **0.163 s** | 0.198 s | 0.222 s | 0.232 s | 0.274 s |
| 推算文件/s | 1.55 M | 1.94 M | **2.42 M** | 1.99 M | 1.78 M | 1.70 M | 1.44 M |

**`rg --files`（带 `.gitignore` 过滤，即每目录有真实 CPU 工作），393,092 文件：**

| 线程 | 1 | 2 | 4 | **8** | 12 | 16 | 32 |
|---|---|---|---|---|---|---|---|
| real | 0.701 s | 0.479 s | 0.293 s | **0.214 s** | 0.217 s | 0.230 s | 0.249 s |
| 推算文件/s | 0.56 M | 0.82 M | 1.34 M | **1.84 M** | 1.81 M | 1.71 M | 1.58 M |

⇒ **两个结论**：

1. **纯遍历最优是 4 线程，32 线程比 4 线程慢 68%**；每目录的 CPU 工作越多，最优点越往右移（带过滤时是 8）。原因是热缓存下是 CPU/syscall bound，线程过多带来调度、锁与缓存行争用。
2. **磁盘分析器的每目录工作比 `rg --files` 更重**（每个文件都要 `stat` 并累加）⇒ **最优并发应该落在 8 附近**，而不是 4。

ripgrep 因此把自动并行度**硬上限压到 `available_parallelism().min(12)`**——即作者本人认为在 32 核机器上也不该超过 12 线程。（[`ignore/src/walk.rs`](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/crates/ignore/src/walk.rs)）

**⚠️ 一个必须记住的边界**：BurntSushi 自己强调这些数字**全是热缓存**——「ripgrep really isn't causing any kind of disk accesses here… if you're specifically looking to benchmark the case where the directory tree is cold and its metadata needs to actually be read from disk, then that is really an entirely separate thing. Parallelism may well indeed still help there, but it's likely its effects and bottom line differences will be different.」参见上文「冷/热缓存的差别」。

| 来源 | 并发度 | 结果 |
|---|---|---|
| **ripgrep（热缓存）** | 1 / 4 / 8 / 32 | **4 最优**；32 反而最差 |
| dumac（macOS, M1 Pro，热缓存） | tokio `spawn_blocking` per directory，`MAX_CONCURRENT = 64` → 0.52–0.563 s | 比 16 并发的 Go CGO 版（0.850 s）快 1.6× |
| **dumac 后续优化** | **tokio 每目录 spawn → rayon work-stealing** | **再快 1.28×**，系统调用数减半，上下文切换从 **1.2 M 降到 235 k** |
| gdu（Linux） | 默认 = CPU 核数（示例机 8 核），`-m` 可调；`GOMAXPROCS=80`/`100` | 冷 4.901 s vs 4.716 s、热 459 ms vs 466 ms，**在噪声内**：超过核数没有收益 |
| dust | `-T` walker threads，默认 = CPU 数；README：「For high-latency storage like NFS or remote mounts, **a higher count can speed up the walk by overlapping more concurrent stat calls**」 | 高延迟卷需要更高并发 |
| ncdu 2 | **单线程**；作者把多线程列为未来工作 | 尚无比对数据 |

**冷/热缓存的差别**：热缓存下是 CPU/syscall bound（线程收益来自掩盖单个 syscall 的延迟，4–12 就饱和），冷缓存下是 I/O bound（需要更多在途请求填满 IOPS）。`diskus` README 给出 400 k 文件 **冷 1.746 s vs 热 0.500 s（3.5×）**，gdu 同规模 **冷 4.716 s vs 热 0.466 s（10×）**。

**推荐并发策略**（按介质与缓存状态自适应，不要写死）：

| 场景 | 建议并发 | 理由 |
|---|---|---|
| 本地 SSD，热缓存 | **4–12**（`available_parallelism().min(12)`） | 与 ripgrep 的实测最优一致；再多是负收益 |
| 本地 NVMe，冷缓存 | 16–32 | 冷缓存下需要更多在途请求填满 IOPS |
| SATA SSD，冷缓存 | 8–16 | 队列深度有限 |
| 机械盘 | **2–4**，或开 `--sequential` | 随机寻道会互相打断；gdu 原文：「HDDs work as well, but the performance gain is not so huge」 |
| 网络 / FUSE / SMB / NFS | **64–128** | 每条 RTT 数十 ms，靠重叠掩盖延迟（dust `-T` 的存在就是这个原因） |
| 未知 | 先 8，用前 200 ms 的吞吐自动调整 | 自适应比写死好 |

**⚠️ 这里有一个必须诚实标注的矛盾：** ripgrep 把上限压到 **12 线程**，而 **dumac 在 macOS 上用 `min(cores, 224)` 个 rayon 线程 + 16 MiB 栈**，并从中获得了 1.28× 的收益。两者不冲突的解释是：**ripgrep 是热缓存、且每个目录的工作很轻（`rg --files` 不做 per-file stat），所以线程一多就是纯争用；而 dumac 每个目录都要发 `getattrlistbulk`（一次真实的阻塞系统调用），在途请求越多越能掩盖延迟。** 推论：**最优并发取决于「每个工作项里阻塞系统调用的占比」，而不是核数。** `dua` 对这件事的处理最成熟——它用 **crossbeam work-stealing + 一个 CPU/墙钟时间探针**，根据「扫描到底是 CPU-bound 还是 I/O-bound」动态调并发。**建议 Sift 也做自适应探针，而不是写死一个数字。**

**别用「每目录 spawn 一个任务」**：dumac 的实测显示，把 tokio 的 per-directory `spawn_blocking` 换成 **rayon work-stealing** 后快 1.28×、系统调用数减半、上下文切换从 120 万降到 23.5 万。ripgrep 的 `ignore` walker 用的是「每线程 LIFO crossbeam deque + stealing」，并明确注释：

> a breadth first traversal on wide directories with a lot of gitignores is disastrous

⇒ **深度优先 + work-stealing 是正确形态**；广度优先在宽目录上会爆内存/爆锁争用。

**实现形态**：**work-stealing 的深度优先遍历**，不要「每目录 spawn 一个线程/任务」。gdu 用 goroutine-per-directory 是安全的（Go 调度器会把它们多路复用到 OS 线程），但 Rust 里 `std::thread::spawn` per directory 在 200 k 目录上会创建 200 k 线程栈，而 tokio 的 per-directory `spawn_blocking` 也已被 dumac 实测证明比 rayon work-stealing 慢 1.28×。两种可行实现：

- **`rayon` work-stealing（推荐）**：递归时用 `rayon::scope`，对子目录 `scope.spawn`；或用 per-thread LIFO deque（ripgrep `ignore` 的做法）。天然负载均衡，且深度优先不会让宽目录把内存撑爆。`dua-core` 也是这条路。
- **固定池 + `crossbeam-channel`**：N 个 worker + 有界队列，worker 扫完一个目录把子目录推回队列。更容易实现「优先级重排」（因为队列是你自己的），代价是要自己处理负载不均。**本项目因为需要「焦点优先」的优先级队列，这条更贴合**——可以两者结合：固定池 + 每线程 LIFO deque + 一个共享的优先级队列做「焦点注入」。

**注意 `jwalk` 已停止维护**（README 首行：「This crate is no longer maintained or supported. Use `dua-core` instead.」），所以要复用它的话用 `dua-core`（4.1.0），或者干脆自己写——本项目的遍历语义（优先级重排、取消、增量聚合）跟通用 walker 差别太大，**自己写一个 ~300 行的 work-stealing 遍历器比适配 jwalk 更简单**。另外 jwalk 自己的 benchmark 显示**单线程 jwalk（141.7 ms）比 walkdir（134.3 ms）还慢**——并行机制不是免费的，串行场景就是要用串行 walker。

**一个非常具体、可直接抄的性能细节：硬链接去重集合的分片函数。** dumac 的作者实测：用 `inode % 128` 分片时平均每次运行有 **176.66 次锁冲突**；改成 **`(inode >> 8) % 128`** 后降到 **4.66 次**，带来约 5% 的墙钟收益。（[Optimizing My Disk Usage Program](https://healeycodes.com/optimizing-my-disk-usage-program)）原因：连续创建的文件的 inode 号是**连续**的（见上一节），所以低 8 位几乎没有熵，取模会把它们全部塞进同一个分片。**所有按 (dev, ino) 或 clone_id 分片的并发结构都应该用高位。**

### `io_uring` / kqueue / IO completion：结论是**首个版本不要用**（有实测支持）

- **`IORING_OP_GETDENTS` 没有进入 mainline**（详见 Linux 章节；`include/uapi/linux/io_uring.h` on master 里有 `IORING_OP_STATX`，没有 GETDENTS；2021 与 2023 两组 patch 都没合入）。所以「一次 syscall 拿一批目录项」这条路不存在。
- **`IORING_OP_STATX` 是真的**（Linux 5.6+），但**实测没有收益**：有一组对比测量显示批量 `IORING_OP_STATX` 与普通 `statx` 的系统调用总耗时**基本相等（2.319 s vs 2.322 s）**，同时多出 **38 µs/次 `io_uring_enter`** 的开销；另一组测量中 io_uring 输给了 rayon 线程池；`tokio-uring` 实际上已停止维护。（[Rust 用户论坛：Batching statx syscall using io_uring](https://users.rust-lang.org/t/batching-statx-syscall-using-io-uring/110745)、[Status of tokio-uring](https://users.rust-lang.org/t/status-of-tokio-uring/114481)）
- 另外 `io_uring` 在部分发行版/容器里被禁（`kernel.io_uring_disabled`），Rust `io-uring` crate 仍是需要手工 mmap ring 的 unsafe 操作。
- ⇒ **决策：v1 用 work-stealing 线程池；io_uring 留作 v2 的可选实验（feature flag + 内核 5.6+ 检测）。** 注意：dumac 的统计里系统调用占 91% 时间，所以「减少系统调用次数」的收益是真实的——**但收益来自「批量接口」（`getattrlistbulk`/`FileIdBothDirectoryInfo`），不是来自 io_uring。**
- macOS：`kqueue` 不支持目录枚举批量；`getattrlistbulk` 本身就是批量接口，**不需要 AIO**。
- Windows：`FSCTL_ENUM_USN_DATA` 可以配 `OVERLAPPED`，但顺序读 `$MFT` 时单线程大块顺序读已能打满带宽，重叠无收益。`NtQueryDirectoryFile` 自带缓冲，也不需要。

### readahead / 预读 / 缓存提示

| 机制 | 平台 | 对**元数据扫描**是否有用 | 判断 |
|---|---|---|---|
| `posix_fadvise(POSIX_FADV_SEQUENTIAL \| WILLNEED)` | Linux | 目录数据块是顺序的，有一点用；**inode 是散落的，预读无效** | **低收益**，可加但别指望 |
| `readahead(2)` | Linux | 同上 | 低收益 |
| `fcntl(F_RDADVISE)` | macOS | 对目录同理 | 低收益 |
| `FILE_FLAG_SEQUENTIAL_SCAN` | Windows | 对 `ReadFile` 读 `$MFT` **有用**（这是唯一的真顺序读场景） | **建议加** |
| `O_NOATIME` | Linux | 避免扫描本身更新 atime（减少写回与 COW 放大） | **建议加**（需要文件 owner 或 `CAP_FOWNER`；失败就忽略） |

**冷缓存才是真实场景**，而冷缓存下的瓶颈是**随机元数据读的 IOPS**，任何预读都救不了「inode 在磁盘上散落」这件事。真正的解法是 **Tier 1 的整卷顺序读**（NTFS MFT）或**提高并发以填满 IOPS**。

### 深度优先 vs 广度优先 vs 优先级队列

| 策略 | 优点 | 缺点 | 用在哪 |
|---|---|---|---|
| DFS（递归） | 打开的目录 fd 只有「当前路径深度」个（典型 < 32）；父链在栈上，**缓存友好**；子树能立刻算出最终大小（后序累积） | 单个超宽目录会卡住；无法优先服务用户关注的分支 | **基础遍历** |
| BFS（队列） | 天然并行、各层均衡 | 同时打开的目录数 = 一整层，fd 可能上万；工作集铺满整棵树，cache 差；**用户关心的子树可能最后才算完** | ❌ 不推荐 |
| **优先级队列（有界）** | 可以直接实现「焦点目录优先 → 父链 → 兄弟 → 其它」，满足「几百毫秒可用」的硬需求 | 需要自己定义优先级函数并处理重排 | ✅ **本项目应该用的** |

**本项目应该用 DFS 骨架 + 有界优先级队列做调度**：worker 从优先级队列取任务；扫描焦点目录时把它的子目录**插到队头**（优先级最高），父链和兄弟次之，其余最低。用户切换焦点时对一个 `Mutex<BinaryHeap<WorkItem>>` 做「重新打分 + 重建堆」（O(n)，n 通常是几百到几千个待处理目录，1 ms 级）。**不要试图撤销已在飞行的任务**——让它们跑完，只调整后续顺序。

### 两阶段（先 `readdir` 全部再 `stat` 全部）vs 交替

**证据状态：我没有找到「交替 vs 两阶段」的任何 head-to-head benchmark。** 网上常见的说法都是推测。所以下面全部标为**推断**：

| 论点 | 判断 |
|---|---|
| 两阶段能改善**目录顺序局部性** | ❌ 不成立。dumac 后续文章实测打印了基准目录里的 inode 号（`[50075095, 50075096, 50075097, …]`，**严格连续**），并指出「一个目录的内容往往是同时写出来的（如 `npm i`）」。⇒ 同一个目录条目的 inode 本来就在相邻位置，**任何顺序的 stat 的 page-cache footprint 都一样**。两阶段既不能省 cache miss 也不多花 |
| 两阶段能改善**调度** | ✅ 成立。它让你「把整个目录的元数据抽干后立刻释放目录 fd」，也方便把 `(name, inode, d_type)` 列表交给另一个线程/队列，而不必持有 dirent 缓冲 |
| ext4/XFS 上两阶段能改善局部性 | ❌ 更不成立。ext4 的 `dir_index`（htree）与 XFS 的目录块按**哈希序**存放，`getdents64` 的顺序不是物理顺序，「目录项在磁盘上聚簇」这个支撑 `getattrlistbulk` 的论据**不可移植** |
| 两阶段省一次 syscall | ❌ 省不了。Linux `getdents64` 只给 `d_ino`/`d_type`/`d_name`，**没有 size**（[getdents(2)](https://man7.org/linux/man-pages/man2/getdents.2.html)），第二遍 `statx` 无论如何都要做 |
| 两阶段的内存代价 | 2 M 条目的 `(ino, d_type, name)` 列表 ≈ 100 MB 常驻（正好把内存预算吃掉一半）。**这个代价是实打实的** |

**推荐做法（把两阶段的「调度收益」用更便宜的方式拿到）**：

1. **用 `d_type` 免费跳过**：`DT_DIR`/`DT_LNK`/`DT_FIFO`/`DT_SOCK` 直接处理，**只有 `DT_REG` 与 `DT_UNKNOWN` 才发 `statx`**。注意 `DT_UNKNOWN` 必须兜底——`getdents(2)` 原文：「Currently, only some filesystems (among them: Btrfs, ext2, ext3, and ext4) have full support for returning the file type in `d_type`. All applications must properly handle a return of `DT_UNKNOWN`.」libuv 在 FS 不填 `d_type` 时返回 `UV_DIRENT_UNKNOWN`，所以这个兜底分支一定要有。
2. **用 dirfd + 相对名发 `statx`**，避免重复的完整路径查找。这是那条 io_uring 讨论帖里 `the8472` 的明确建议：「operating on directory file descriptors and short paths relative to those FDs should be a bit faster since it will avoid repeated path lookups in the kernel. It does have caches for that, but even cache lookups aren't free.」这条比「两阶段」收益更实在。
3. **可以用 `AT_STATX_DONT_SYNC` / `AT_SYMLINK_NOFOLLOW`**（语义允许时），避免触发属性缓存失效。
4. **不要为「局部性」造两阶段流水线**；如果为了「限制每个子树的内存」或「渐进上报」而做流水线，那是另一回事（见「增量与流式」里 dust 的 `PendingDir` 模式）。

### Windows / macOS 的缓冲调优

- `NtQueryDirectoryFile` / `GetFileInformationByHandleEx`：**缓冲区给大**（64–256 KiB）。一次调用回几十到几百条，调用次数约等于「目录条目数 / 每缓冲条目数」。缓冲太小会退化成近似 per-entry 的 IOCTL 往返。
- `getattrlistbulk`：128 KiB 是 dumac 作者实测的最优值；太小会 `ERANGE` 或增加往返，太大浪费 L2。**建议 64–256 KiB 之间自适应**。
- `FSCTL_ENUM_USN_DATA`：输出缓冲 1 MiB 起步（每条 `USN_RECORD_V2` ≈ 60–600 B）。

---

## 百万级节点的内存架构

### 先看别人实测的每节点成本

| 系统 | 规模 | 内存 | **每节点** | 来源 |
|---|---|---|---|---|
| ncdu 1.16 | 3.8 M 文件 | 429 MB | 113 B | [ncdu2 文章](https://dev.yorhel.nl/doc/ncdu2) |
| **ncdu 2.0-beta1** | 3.8 M 文件 | **162 MB** | **≈43 B** | 同上（含名字与哈希表开销） |
| ncdu 1.16 | 38.9 M 文件 | 3969 MB | 102 B | 同上 |
| ncdu 2.0-beta1 | 38.9 M 文件 | 1686 MB | 43 B | 同上 |
| Everything | 250 k 文件 | 35 MB | 147 B | [voidtools FAQ](https://www.voidtools.com/faq/)（厂商声称） |
| Everything | 1 M 文件 | 100 MB | 105 B | 同上 |
| duh | ~4 M 文件 | 内存有界，**落 SQLite** | — | [duh README](https://github.com/cheapsteak/duh) |

ncdu 2 作者给出的**纯节点结构大小**（不含名字与哈希表）：

| | ncdu 1.16 | ncdu 2.0-beta1 |
|---|---|---|
| 普通文件 | 78 B | **25 B** |
| 目录 | 78 B | **56 B** |
| 硬链接 | 78 B + 8 B/唯一 dev+ino | 36 B + 20 B/ino×目录 组合 |

⇒ **结论：2 M 文件 ~100 MB 是可达的**（ncdu 2 的实测外推 ≈ 86 MB）。但 ncdu 2 有一个有利条件：作者自己说测试树的文件名「平均约 10 字节」。真实世界的平均文件名 15–25 字节，要按偏高估。

### 四种架构的对比与内存估算

假设：**2 M 文件 + 20 万目录**（≈9% 目录，符合真实 macOS/Windows 用户卷），平均文件名 20 字节（UTF-8），平均完整路径长度 70 字节。

#### (a) `HashMap<String, Node>` / `HashMap<PathBuf, Node>`（**不推荐**）

**先看 HashMap 自己的成本。** 一个公开可复现的 harness（[`map_bench`](https://github.com/innovabinaria/map_bench)）在 N = 1 000 000 时报告 `HashMap<u64,u64>`：insert 1644 ms / query 101 ms，**insert 后 RSS = 48 MiB（≈50 B/entry，payload 只有 16 B）**。这就是 `std::HashMap` 的 SipHash + 控制字节 + 「桶数必须是 2 的幂」造成的 ~50% 空槽的代价。

| 项 | 每节点 |
|---|---|
| `PathBuf` 结构体（Unix 下 = `Vec<u8>` 的封装） | 24 B |
| 堆上路径字符串（平均 80 B + `malloc` 元数据/16 B 对齐 ≈ 96 B） | ~96 B |
| `Node`（parent 8 + size 8 + flags/mtime 16 + child 容器 24） | ~48 B |
| hashbrown 槽位：payload `24 + 48 = 72 B` + **1 控制字节**；**桶数取 2 的幂** ⇒ 1 M 节点需要 2²¹ = 2 097 152 槽，实际负载只有 **0.477**（远低于名义 7/8） | ~120 B/节点 |

| 节点数 | 表内存 | 堆上路径 | **合计** |
|---|---|---|---|
| 1 M | 119.5 MB | 96 MB | **≈216 MB** |
| 2 M | 239 MB | 192 MB | **≈431 MB** ⚠️ 逼近 500 MB |
| 4 M | 478 MB | 384 MB | **≈862 MB** ❌ |
| 10 M | 956 MB | 960 MB | **≈1.92 GB** ❌❌ |

**问题**：① 每个节点一次堆分配（malloc 开销 + 碎片）；② **路径字符串完全冗余**（父路径被每个子孙重复存一遍，2 M 节点 × 80 B = 160 MB 全是重复前缀）；③ `std::collections::HashMap` 默认用 **SipHash-1-3**（HashDoS 抗性，但比 `foldhash` 慢数倍——`hashbrown` 0.15 起默认 hasher 已换成 foldhash，std 用的是自己的 `RandomState`）；④ 表扩容时桶数组翻倍，4 M 节点时峰值会同时存在 ~478 MB 的旧表 + 新表。**只适合做辅助索引，不适合做主存储。**

#### (b) arena / 索引树（**推荐**）

**节点布局（CSR 变体，32 B 整）**：

```rust
struct Node32 {
    size_disk:  u64,  // 8  分配大小（on-disk，主口径）
    parent:     u32,  // 4  NIL = u32::MAX
    child_start:u32,  // 4  CSR：直接子项在 children[] 中的起点
    child_count:u32,  // 4
    name_off:   u32,  // 4  指向 name arena
    flags:      u32,  // 4  类型 + 可见性 + 权限 + 「硬链接首见」等位标志
    mtime:      u32,  // 4  Unix 秒（可选）
}                     // = 32 B
```

**关键设计选择**：

- **用 CSR（`child_start` + `child_count`）而不是 `first_child` + `next_sibling`**：children 在全局 `Vec<u32> children` 中连续存放。好处是 ① 省掉 4 B；② 排序 / top-K / `select_nth_unstable` 变成**对连续 slice 的线性扫描**；③ 没有每目录一次 `Vec` 分配。这正好匹配磁盘分析器的热操作——「遍历某目录的子项」和「按大小排序某目录的子项」。
- **`u32` 而不是 `usize` 做索引**：4 个索引字段省 `4×4 = 16 B/节点`（**−28%**），1 M 节点省 16 MB。`u32` 支持 42.9 亿节点，是 10 M 目标的 400 倍。**注意：我找不到「`u32` vs `usize` 索引」的公开 benchmark；这 16 B 是纯算术、确定成立，但性能方向不明确**（节点更窄 → 每 cache line 容更多节点；但 `u32→usize` 转换多几条指令）。DFS/CSR 顺序下缓存密度收益大概率占优，但要实测。
- **只存 basename，绝不存完整路径**。路径靠 `parent` 链回溯重建（ncdu 的 `Dir.fmtPath` 是参考实现）。
- **名字放在一个 `Vec<u8>` arena**，NUL 结尾的 basename，每个只花 `len+1` 字节。

| 组件 | B/节点 |
|---|---|
| `Node32` | 32.0 |
| 目录额外字段（精确 item 数，8 B × 8%） | 0.6 |
| 名字字节 + NUL（20 + 1） | 21.0 |
| `Vec` 增长余量（1.15×） | 4.9 |
| **合计** | **≈58** |

| 布局 | B/节点 | 1 M | 2 M | 4 M | 10 M |
|---|---|---|---|---|---|
| `HashMap<PathBuf, Node>` | ~216 | 216 MB | 431 MB | 862 MB | 1916 MB |
| 每目录 `Vec<(CompactString, Node)>`（gdu 风格） | ~66 | 66 MB | 132 MB | 264 MB | 660 MB |
| **arena + basename（`u32` 索引）** | **~58** | **58 MB** | **116 MB** | **233 MB** | **583 MB** |
| arena 但用 `usize` 索引 | ~74 | 74 MB | 148 MB | 297 MB | 743 MB |
| arena + basename interner | ~47 | 47 MB | 94 MB | 188 MB | 470 MB |
| **arena，仅目录节点** | ~4.6 | 4.6 MB | 9 MB | 19 MB | 46 MB |
| SQLite/redb 行（~30 B/行 + page cache） | ~30 磁盘 | 30 MB 磁盘 | — | 120 MB 磁盘 | 300 MB 磁盘 |

**敏感性**：平均 basename 每多 10 B，arena 布局每百万节点多 10 MB（~17%）。如果同时要 *逻辑大小* 和 *分配大小*，节点变成 40 B，所有 arena 数字 **×1.25**。

**临时峰值**：`Vec` 翻倍扩容。4 M 节点 arena 是 128 MB 节点字节，从 64 → 128 MB 时瞬时峰值 ~192 MB。**做法：先用一次轻量预扫（或滚动估计）给出 `with_capacity`，扫完 `shrink_to_fit()`。**

**2 TB / 2 M 文件的具体账**（目录占比 8% ⇒ **~174 k 目录**，总节点 2.17 M）：

- 全节点 arena：2.17 M × 58 B ≈ **126 MB**（在 500 MB 内，略超 ~100 MB 的目标）
- **仅目录**：174 k × 58 B ≈ **10 MB** ✅
- **仅目录 + 每目录 top-K（K=32）**：174 k × 512 B（top-K 项）+ 174 k × 40 B（目录记录）≈ **96 MB** ✅ ← **唯一同时满足「~100 MB」和「能回答『什么最占空间』」的方案**

#### (c) 溢出到磁盘 / mmap（**仅作为超预算降级**）

| 方案 | 证据 | 判断 |
|---|---|---|
| SQLite（`rusqlite` 0.40.2） | duh 用 SQLite 存 ~4 M 文件，README 说「memory-bounded and streaming」；gdu `--db=*.db` 从 4.7 s 退化到 **45.0 s**（冷）/ 8.2 s（热） | 首扫 **6–10× 变慢**。**只在节点数 > 预算时启用**，或用于 Tier 0 的持久化目录册 |
| BadgerDB（Go） | gdu `--db=*.badger` 27.5 s | 同上，且是 Go 专用 |
| **gdu 的 StoredAnalyzer 形状值得抄** | gdu v5 的 `--db` 模式**一次只在内存里保留一个目录**，把每个目录用 `gob` 序列化后按路径 key 写进 badger，子目录按需重新物化并有显式的缓存淘汰钩子 | **关键洞察：溢出的单位是「目录」而不是「节点」**——这才让随机访问有界 |
| `memmap2` 0.9.11 自建 arena | 无公开对照数据（**证据空白**） | **推荐**：把 `Vec<Node>` + 名字 arena 放进 `memmap2` 映射的文件，正常访问就是普通内存，**压力大时 OS 自动换出**，不需要 DB 的 SQL 开销。代价：① 重新分配 = 重映射；② 如果别的进程截断了文件，访问会 SIGBUS；③ 无法通过裸 mmap 做原子更新 |
| `rkyv` 0.8.18 + `memmap2` 零拷贝归档 | — | **Tier 0 的最佳选择**：`rkyv::to_bytes` 存盘，冷启动 `mmap` + `rkyv::access` 直接得到 `&ArchivedTree`，**零反序列化**。注意 `rkyv` 内部 unsafe 很多，且格式变更必须做版本号 |
| `redb` 4.3.0 | 纯 Rust ACID B 树（mmap），MIT/Apache | SQLite 的纯 Rust 替代；同样是页式开销，不减少常驻内存 |
| `rocksdb` / `sled` | — | RocksDB 是 C++ FFI、构建极重；**sled 已停滞**（其自身 README 建议改用 SQLite）。**都不用** |
| `tempfile` 3.27.0 | — | 溢出文件的生命周期管理 |

**注意**：嵌入式 DB 不会自己降低常驻内存，它只是把树搬到磁盘、把问题从 **memory-bound 变成 I/O-bound**。那是一个合法设计，但你的瓶颈就换人了：gdu 的 `--db` 模式从 4.7 s 变 45 s 就是代价。

#### (d) 压缩名字 / 前缀共享

- **先纠正一个常见误解**：现代 **plocate 1.1.x 并不用「前缀/后缀编码」**。它把文件名按 **32 个一块** 拼接（NUL 分隔）后用**训练过的共享 Zstd 字典**（`ZDICT_trainFromBuffer`，level 6）压缩，另加一个 **trigram 倒排索引**（posting list 用 TurboPFor/PForDelta 做 128 值分块差值编码）。「前缀共享」描述的是旧的 **mlocate** 格式。实测密度：**27 M 文件 → 466 MB ≈ 17.3 B/文件**（mlocate 是 1.1 GB / ~40.7 B/文件）。（[plocate](https://plocate.sesse.net/)）
- 但 **17.3 B/文件是磁盘密度，不是内存**，而且它是「路径的全集」索引，没有树结构、不支持随机树访问。对需要随机展开的 UI 不合适。
- 对树形结构更合适的是 **front-coding 兄弟节点**（同目录下兄弟共享前缀）或 **ART / radix trie**。**ART 的实测数据不支持它**：论文 Table I 给 Node4=52 B、Node16=160 B、Node48=656 B、Node256=2064 B，长字符串键约 **32 B/key**（还不含键字节），且节点类型分派带来 **0.84 次分支预测失败/次查找**、grow/shrink 损失 ~20% 插入吞吐。**对「短、浅、按 DFS 顺序访问」的 basename，ART 不比 byte arena 好。**
- **名字 interning** 只在名字高度重复时才划算。按「40% 唯一名」估算，省下 ~12 B/节点的名字字节，但要付 ~7 B/节点的哈希表项，**大致打平**。值得做的场景：① 你本来就需要 `name → id` 查表；② 名字重复率极高（`part-00000…`、`0001.jpg`、`index.html`）。
- **rustc interner 的教训值得抄**：`Span` 是 8 B，**只有放不下时才 out-of-line intern**（不到 0.1% 的情况）；早期用 4 字节 span 反而更慢，因为只有 80–90% 能内联。**结论：常见情况内联，罕见情况 intern，绝不用一个统一方案去付最坏情况的代价。** 对应到本项目：短 basename 直接放 byte arena，超长名（>255？）走 side table。

### 推荐的具体数据结构

```rust
// 主存储：CSR 排列的索引式 arena
struct Tree {
    nodes:      Vec<Node32>,   // 32 B/节点，u32 索引，NIL = u32::MAX
    children:   Vec<u32>,      // CSR：某目录的直接子项在 children[] 中连续
    names:      Vec<u8>,       // 名字字节 arena（NUL 结尾的 basename）
    dirs_only:  bool,          // 超预算降级标志
    // 以下按需分配、可随时丢弃的「派生索引」
    id_index:   Option<FxHashMap<u64, u32>>,  // (dev, ino) 或 file_id → node，用于硬链接/clone 去重
}
```

**内存预算目标**（按上一节的 58 B/节点、8% 目录）：

| 节点数 | 全节点 arena | 仅目录 | 仅目录 + top-K(32) | 结论 |
|---|---|---|---|---|
| 1 M | ~58 MB | ~4.6 MB | ~50 MB | ✅ |
| 2 M | ~116 MB | ~9 MB | ~96 MB | ✅ **「2 TB / 2 M 文件」正好落在 ~100 MB** |
| 4 M | ~233 MB | ~19 MB | ~190 MB | ✅ 在 500 MB 内 |
| 10 M | ~583 MB | ~46 MB | ~470 MB | ⚠️ **全节点超 500 MB → 必须降级为「仅目录 + 溢出」** |

**实现要点**：

1. **绝不存完整路径**。只存 basename + `parent` 索引，路径靠回溯重建。这是省内存的第一性原理（一个 80 B 的路径里有一大半是祖先目录名，重复存百万次）。
2. **用显式栈做后序累积，不要递归**。目录嵌套深度在真实场景无上界（`node_modules`、`~/.cache`、构建树），Rust 主线程栈默认 8 MiB。显式 `Vec<(NodeId, u32)>` 游标还让遍历**可取消、可恢复**。
3. **两阶段顺序**（先定父子索引，再按 DFS 逆序求和）可以得到一条完全线性、无分支的累积循环，配合 CSR children 数组最自然。
4. **布尔标记全部位图化**：300 万个布尔只有 375 KB。

| 需求 | 推荐 crate | 版本 | 说明 |
|---|---|---|---|
| **节点 arena（无 unsafe）** | **`id-arena`** | 2.3.0 | 密集 `Vec<T>` + `Id<T>(u32)`，`#![forbid(unsafe_code)]`，每节点成本最紧，不支持删除——正好匹配「扫描期只增不删」 |
| 节点数组 / 名字 arena | **标准库 `Vec`** | — | 手写 CSR arena 比任何 crate 都省、都快，且没有 `unsafe`。**首选** |
| `u32`/`u64` 键的快速哈希 | **`rustc-hash`（FxHashMap）** | 2.1.3 | `#![forbid(unsafe_code)]`，Apache-2.0 OR MIT；用于硬链接/clone 去重的 id→node 表 |
| 需要密度 + 索引混合 | `indexmap` | 2.14.2 | 基于 hashbrown 0.17 |
| bump 分配临时结构 | `bumpalo` | 3.20.3 | 只用于**单次遍历内**的临时分配（如临时路径拼接）；**不能单独释放、不跑 `Drop`**，别用来存长期节点 |
| 名字 interning | `string-interner` 0.20（`BufferBackend` 最省）/ `lasso` 0.7.3 | — | 可选；duplicate 率高时才划算（见上文估算：约打平） |
| **UI 热路径的短字符串** | `compact_str` | 0.10.0 | `size_of == 24`，**≤24 字节内联、零堆分配**。**不要全树用它**（比 arena 贵），只用于 top-N 展示的名字副本 |
| 小容器 / 内联 top-K | `smallvec` | 1.16.1 | 内联容量省一次分配 |
| 位图 | `bitvec` 1.1.1 或裸 `u64` 位运算 | — | |
| 哈希表 | `hashbrown` 0.17.1 | — | **注意两个 API 变更**：① `raw_entry` 自 0.15 起被 gated 且「eventually removed」，替代品是 **`HashTable`**（0.14.2 引入、0.15 成为中心 API）；② `DefaultHashBuilder` **已是 `foldhash::fast::RandomState`**，不是 ahash。std 的 `HashMap` 用的是 SipHash-1-3 |
| 并发分片集合 | `dashmap` 6.2.1 或自建 N 个 `RwLock<HashSet>` | — | **dashmap 的每项开销严格大于裸 hashbrown**；dumac 就是「分片 HashSet」来降低锁竞争 |
| 锁 | `parking_lot` 0.12.5 | — | 比 std 锁快、无 poisoning |
| 通道 | `crossbeam-channel` 0.5.17 | — | 有界队列 + `select!`；ripgrep 用的是 crossbeam-deque（work-stealing） |
| 零拷贝解析 packed 结构 | `zerocopy` 0.8.58 | — | `FromBytes`/`TryFromBytes` 解析 `dirent64`、`FILE_ID_BOTH_DIR_INFO`、MFT 记录，**你自己的代码里没有 `unsafe`** |
| 零拷贝序列化 | `rkyv` 0.8.18 | — | Tier 0 归档（内部 unsafe 很重，需要格式版本号） |
| 内存映射 | `memmap2` 0.9.11 | — | Tier 0 + 超预算溢出 |
| 溢出数据库 | `redb` 4.3.0 / `rusqlite` 0.40.2 | — | 仅在需要 SQL / 增量写时。**不要用 `sled`**（已停滞） |
| 并行 | `rayon` 1.12.0 | — | 或 `crossbeam-deque` 自建 work-stealing |

### 不保存文件子节点也能算目录大小

这是标准做法，而且**必须**这么做才能把「内存」和「焦点响应」同时压下来：

1. **后序遍历（post-order）累积 + 在线累加**。每个目录维护 `size_disk`/`size_logical` 与 `items`。每处理完一个子项就立刻 `parent.size_disk += child.size_disk`；当「已计入的子项数 == readdir 返回的条目数」时该目录就是**最终值**。ncdu 就是这么做的。
2. **目录大小天然等于「自身 + 所有子孙文件」之和**，不需要保留文件节点即可精确计算。
3. **只保留目录节点时**，硬链接去重仍需要 `(dev, ino)` 的「已见」集合（一个硬链接文件可能出现在两个不同目录下，两处都会尝试加，必须只加一次）。这个集合的大小 = 硬链接文件数，通常 < 5% 的文件数。
4. **需要「最大文件 Top-N」时**——这是磁盘分析器的核心卖点之一，有三种设计：

   | 设计 | 内存 | 说明 |
   |---|---|---|
   | **惰性计算（推荐默认）** | **0** | 全树只留目录 + 每个目录的 `size`。用户在 UI 里展开某目录时，**只重扫这一个目录**（一次 `getdents64` + 直接子项 stat），用 `select_nth_unstable_by`（stable since 1.49；Rust 1.98+ 还有 `partial_sort_unstable*`，语义就是「只排序前 k 个」）取前 K。成本 = 一次目录读，**毫秒级** |
   | **每目录 top-K + 精确残差（推荐用于「任意目录免重扫」）** | 174 k 目录 × (K×16 B + 40 B) | K=32 → **~96 MB**；K=8 → ~30 MB。每个目录保留：**精确**的 `total_disk`/`total_items`、**精确**的 `total_disk` 全额、K 个最大的直接文件子项、以及被淘汰项的聚合 `rest_bytes`/`rest_items`。UI 可以渲染「…… 另有 4 213 个文件，87.3 GB」。**总量不失真**，这正是关键 |
   | 每目录 `BinaryHeap<T>` | `Vec` 头 24 B + K×16 B + 最小分配 ~32 B ≈ **568 B/目录** | 只对 child_count > K 的目录分配（宽目录是重尾分布，比例很小）。`smallvec<[T; K]>` 内联版把 512 B 直接塞进目录记录、零分配，但**每一条目录记录都变胖** |

   **不要学 gdu 的 `TopList`**：它是「排好序的 slice，每次 insert 后 `sortFiles` 再截断到 N」，O(K log K) **每次插入**。CSR + `select_nth_unstable` 在扫描结束后做一次就够了。

5. **大小实时细化（delta 传播）**：`add_delta(node, Δsize, Δitems)` 沿 `parent` 链上行到根，**O(深度)**：

   ```rust
   fn add_delta(mut d: u32, ds: i64, di: i64) {
       loop {
           nodes[d].size_disk = (nodes[d].size_disk as i64 + ds) as u64;
           nodes[d].items     = (nodes[d].items     as i64 + di) as u64;
           match nodes[d].parent { NIL => break, p => d = p }
       }
   }
   ```

   一次完整自底向上是 O(n)；把**每个完成节点**都沿父链回放是 **O(n·D)**（D = 平均深度 10–15），也就是每个节点多 10–15 次整数加法——**相对于产生这些数据的 `statx` 系统调用完全可以忽略**。gdu 的 `Dir::subtractStats` 就是这个形状。**唯一不能简单 delta 的是硬链接/clone 的「独占大小」**（见「正确性陷阱」）。
   **关键技巧是批量**：不要每个文件发一条 UI 事件，而是**每 16–50 ms 或每 N=4096 个条目 flush 一次**，把同一路径上的多个 delta 合并成一条。
6. **大小只会增长（扫描期间）**，所以任何时刻都能渲染一棵部分树；但注意**删除/截断会让大小变小**，累加器必须做**饱和运算**，不能假设单调。

---

## 增量与流式

### 1. 焦点优先：有界优先级队列

需求是「用户正在看的目录几百毫秒内可用，大小再慢慢补」。这可以分解成两件事：

**(a) 首屏 = 只扫焦点目录本身。** 用户切到 `~/Movies` 时，不启动整棵子树的扫描，而是：

```
1. getdents64 ~/Movies          → 拿到直接子项名字 + d_type   (1 次 syscall 级)
2. 对直接子项 statx（只这一层）  → 拿到大小/时间              (n 次)
3. 立刻 push discovered + size_updated
4. 把子目录作为低优先级任务入队
```

即使 `~/Movies` 有 5000 个直接子项，这一步也是**几毫秒到几十毫秒**。这满足了「几百毫秒可用」。

**(b) 有界优先级队列。** 优先级函数：

```
priority(node) = 
    P0 (0)   焦点目录自身及其直接子项（正在被扫描或已扫描）
    P1 (1)   焦点目录的祖先链（用户点「上级」时要立刻可用）
    P2 (2)   焦点目录的兄弟目录 + 上次扫描时最大的几个目录（大目录优先，收益最大）
    P3 (3)   其余（按深度/上次大小排序）
    P4 (4)   明确排除的路径（/proc、/sys、.Trash、Time Machine 等）——永不入队
```

队列**有界**（例如 4096 个 task），满了就**拒绝入队**（拒绝比阻塞好：阻塞会卡住 worker）。用户切换焦点时：

- 给新的祖先链/兄弟打 P0/P1/P2；
- **不撤销**已发出的任务（撤销成本 > 收益）；
- 对 `BinaryHeap` 重新打分：因为 `BinaryHeap` 不支持改键，实践做法是**双堆 / lazy deletion**（插入新的优先级条目 + 一个 `generation` 计数器，弹出时丢弃过期的），或者干脆维护 `Vec<WorkItem>` 并每 N 次 pop 做一次 `sort_unstable`（N=64 时排序几千项的代价在 100 µs 内）。

### 2. 事件流：合并 + 限速

百万级节点如果「每节点一条 IPC 事件」，2 M 次 IPC 会直接把 Tauri 的事件桥打爆（JSON 序列化 + WebView `postMessage` 的开销比扫描本身大一个数量级）。必须：

- **批量 flush**：固定 16–50 ms 一个 tick（≈20–60 fps），在 tick 内把 `discovered` 按路径前缀合并、`size_updated` 按节点合并（同一节点只发最后一版）；
- **只在「可见范围」发细节**：焦点目录及其父链、兄弟发完整 `discovered`；其余只更新祖先的聚合大小；
- **大批量节点走批量 API**：一次 IPC 传 `Vec<NodeDelta>`（`serde` + 二进制 `tauri::ipc::Response` / `tauri::ipc::Channel`），不要一次一个 `emit`；
- **背压**：如果 WebView 消费不过来，**丢中间帧**（只保证最终一致），而不是让扫描线程阻塞。前端用「滚动更新」而不是「逐帧动画」。

### 3. 取消

- 每个扫描任务带一个 `Arc<AtomicBool>`（或 `CancellationToken`）。
- Worker 在**每个目录边界**检查一次（不是每个文件，检查太频繁会有缓存行争用；每个目录一次大约是 1 µs 粒度，足够）。
- 取消时：立刻停止 `pop`，drop 队列，把 `scan_done` 以 `cancelled: true` 发出，**保留已扫描的部分结果**（gdu 的行为：「press Esc or Ctrl+C during a scan to stop scheduling new work and keep the results found so far」）。
- **不要试图中断一个正在 `readdir`/`stat` 的 syscall**——它最多几百微秒，等它返回即可。

### 3b. 并行下的「渐进发布」参考实现：dust 的 `PendingDir` 模式

**这是本次调研里最值得直接抄的一段代码。** `dust` 的 walker 在 rayon 并行遍历之上自己加了一层 per-directory 记账（[`src/dir_walker.rs`](https://raw.githubusercontent.com/bootandy/dust/master/src/dir_walker.rs)），结构是：

```rust
struct PendingDir {
    parent:   Option<Arc<PendingDir>>,   // 指向父目录，构成可回溯的链
    pending:  AtomicUsize,               // 未完成的子任务计数（自身初始 1，每 spawn 一个子目录 +1）
    children: Mutex<Vec<Node>>,          // 本目录已收集的子项
}
```

为什么这个形状是对的：

- 目录大小只有在**所有**子孙完成后才最终确定；并行 walker 无法「提前造出一个完整的树节点」，**只能造出一个单调递增的部分和**。
- `pending` 用 `AtomicUsize` 计数，「减到 0」就是「这个目录定稿了」——**不需要全局锁、不需要 barrier**。
- `parent: Option<Arc<PendingDir>>` 让任何时刻都能沿链读到祖先的**当前部分和**——这正是「焦点目录几百毫秒可用、大小实时细化」需要的东西。
- 任何要求「节点必须完整」的 UI 都会卡住；**渲染部分和 + 「扫描中」标记的 UI 不会**。

**一条真实的用户压力证据**：gdu 的 issue #150 里，一个用户在 **3 亿条目 / 14 TB** 的卷上不得不 Ctrl-C 杀掉 gdu 并丢失全部结果，然后说「I'm back to using ncdu now」。gdu 因此加了 `--ctrl-c-quits` 与「Esc 停止扫描并保留结果」。（[gdu #150](https://github.com/dundee/gdu/issues/150)）⇒ **「可取消且保留部分结果」是这个产品的一等需求，而它必须和并发设计一起设计**（你必须能排空在飞任务并确定性地定稿部分和）。

**关于「优先级队列遍历」的诚实说明**：我**没有找到任何公开的优先级队列文件系统遍历设计文档**。ripgrep 的 `ignore` 用的是 **LIFO** deque（与优先级队列相反），理由是内存。所以「当前目录 → 父链 → 兄弟 → 其它」这个调度策略**在本项目之外没有先例可抄**，属于我们自己要设计并验证的部分。下面给出一个我认为可行的形态（**推断，需实测**）：

1. **用「两个 deque」而不是一个通用优先堆**。每个 worker：
   - 一个 **focus deque**：装着用户当前视口路径上的目录及其直接子目录，**优先 drain（LIFO）**；
   - 一个 **main deque**：其余全部（也是 LIFO，理由见 DFS 一节）。
   Worker 先清空 focus，再去做 work-stealing。
2. **只在导航时重排，不做持续重排**。用户打开一个目录时，把它的子目录压进 focus deque。**不要重排在飞的队列**——那会把 O(1) 的 push 变成带全局锁的 O(log n)，正好落进 ripgrep 设计要避开的争用陷阱。
3. **祖先立刻以部分和发布**（因为子项大小是原子上卷的），UI 标 `partial` 而不是等完整。

### 3c. 高吞吐事件处理与背压

- **绝不每个文件系统事件发一条 UI 通知**。管线：原始事件 → 按路径去重/合并的 map（有界 LRU；被淘汰时强制重扫该路径）→ 脏子树集合 → 限速的「重算 + 发布」tick（交互态 ~10 Hz，后台 ~0.5 Hz；**可配置**，ncdu 就提供 `-q/--slow-ui-updates`（每 2 s）vs 默认 10 Hz）。
- **有界通道，且丢弃必须被检测**：FS 事件线程与模型线程之间用 `crossbeam_channel::bounded`，让 watcher 合并/丢弃而不是无限增长；**任何丢弃都意味着「重扫该祖先」**。
- **定义降级策略**：脏集合超过阈值 → 收敛到最近公共祖先并安排子树重扫；再超阈值 → 升级为全卷重扫。这就是 FSEvents 的「must scan subdirs」策略的一般化。
- **UI 与扫描线程之间不共享锁**。UI 读一个不可变快照指针（`arc-swap`）。Tauri 里用 `tauri::ipc::Channel` 批量推，**不要 `emit` 一个文件一次**。dumac 的教训是：把一个朴素分片哈希集合交给多个线程会带来可观的锁争用（见上一节的 176.66 → 4.66 次冲突）；**再把这个结构暴露给 UI 线程只会更糟**，因为 UI 会在一次无关的渲染期间持锁。

### 4. 「只有子树变了」的增量重扫

#### 变更检测源

| 平台 | 机制 | 能否覆盖整卷 | 关键坑 |
|---|---|---|---|
| macOS | **FSEvents**（`FSEventStreamCreate` + `kFSEventStreamCreateFlagFileEvents` + `kFSEventStreamEventId` 历史回放） | ✅ 可以（按卷注册） | 批量合并；`kFSEventStreamEventFlagMustScanSubDirs` 表示「这里丢了细节，必须整棵子树重扫」；`kFSEventStreamEventFlagHistoryDone`；**进程不在运行时的事件可以在重启后用 sinceWhen 回放** |
| macOS | `kqueue` / `DispatchSource` | ❌ 需要逐个 fd 注册 | 不适合整卷 |
| Windows | **USN Journal**（`FSCTL_READ_USN_JOURNAL`） | ✅ 可以 | 需要管理员；journal 可能被删除/覆写（`ERROR_JOURNAL_ENTRY_DELETED` → **强制全量重扫**）；`USN_REASON_CLOSE` 把一次文件打开期间的多次修改合并成一条；rename 产生两条记录（旧名 + 新名） |
| Windows | `ReadDirectoryChangesW` | ❌ 逐目录 | 缓冲溢出静默丢事件 |
| Linux | `inotify` | ❌ 逐目录 | `/proc/sys/fs/inotify/max_user_watches`（上游默认历史上 8192，发行版常调高）、`max_user_instances`（默认 128）、`max_queued_events`；溢出给 `IN_Q_OVERFLOW`；**递归 watch 不存在**，监视整卷需要为每个目录建 watch → 在 20 万目录的卷上不可行 |
| Linux | `fanotify`（`FAN_MARK_FILESYSTEM` + `FAN_REPORT_FID`） | ✅ | 需要 **`CAP_SYS_ADMIN`**，桌面 app 拿不到 | 

**结论（Linux 的坏消息）**：**Linux 上没有普通用户可用的整卷变更通知。** 所以 Linux 上的策略必须是：

1. **焦点子树用 inotify 递归监视**（只对用户正在看的目录树，通常几百~几千个目录，可以承受）；或
2. **轮询 + 目录 mtime 短路**（见下），间隔 2–10 s，只轮询**已展开的目录**；
3. 其余部分信任 Tier 0 缓存，用户主动点「重扫」才更新。

#### 目录 mtime 短路（「只重扫变了的子树」的核心启发式，但**必须理解它的边界**）

规则：缓存节点时同时记 `mtime` + `ctime`（目录）与 `size` + `mtime` + `ctime`（文件）。重扫时先 `statx` 目录自身做比较。

**⚠️ 一个非常容易搞错的点：目录 mtime 不是可传递的。** 准确语义是：

- 在目录 D 下**新增/删除/改名**一个条目 → 更新 D 的 `mtime` 与 `ctime`；
- **修改一个已存在文件的内容** → **不**更新其父目录的任何属性；
- **深层变化不向上传播**：新建 `/a/b/c.txt` 只更新 `/a/b` 的 mtime，`/a` 的 mtime 完全不动。

因此：

| 你想得出的结论 | 目录 mtime 能不能支撑 |
|---|---|
| 「D 的直接子项集合没变」 | ✅ 可以 |
| 「D 的整棵子树都没变」 | ❌ **不可以** |
| 「D 子树里没有文件大小变化」 | ❌ **不可以** |

**所以正确的用法**是：D 的 `mtime`+`ctime`+`nlink` 都没变 ⇒ 可以**复用 D 的直接子项列表（省掉一次 `getdents`）**，但**仍然要逐个 `statx` 直接子项**（每个子项比 `(size, mtime, ctime)`）。这是「每个子树省一次目录读」的**常数级**收益，**不是 O(1) 跳过整棵子树**。真正的 O(变化数) 增量只能靠外部变更日志（USN / FSEvents / fanotify）。

其他必须写进文档的坑：

- **mtime 是用户可设置的**（`touch`、`utimensat`、`cp -p`、`rsync -t`、解包工具都会保留旧 mtime），所以它只是**速度启发式**，不是正确性保证。
- **时间粒度**：FAT 2 s、HFS+ 1 s、ext4/NTFS 100 ns 级但可被用户设置。GNU make 的「同一秒内」缓存 bug 就是经典案例。**rsync 的做法值得抄**：默认「大小相同 **且** mtime 相同（带 `--modify-window` 容忍窗）」才跳过；提供 `--size-only` 与 `--checksum` 作为显式权衡。
- **`ctime` 比 `mtime` 强**：`ctime` 无法被用户态设置，任何 inode 元数据变化（含 `chmod`、`link`、内容写入）都会更新它 → 假阳性更多但漏报更少。Windows 上的对应物是 `FILE_BASIC_INFO.ChangeTime`。
- **NFS/SMB 上 mtime 的粒度与一致性都不可靠**（`AT_STATX_FORCE_SYNC` vs `AT_STATX_DONT_SYNC`，见 [statx(2)](https://man7.org/linux/man-pages/man2/statx.2.html)）。**网络卷上默认关闭这个短路。**
- **不要用目录的 `size` 字段**判断「条目数变了」——那是文件系统相关的内部数字。Unix 上 `nlink - 2` 是子目录数，比 `size` 有意义，但也不是所有 FS 都老实维护。

**建议的四级策略**：

| 层级 | 条件 | 行为 | 成本 |
|---|---|---|---|
| L0 | 有 USN/FSEvents 增量日志且能追平 | 只对日志涉及的路径做**增量重扫**（重扫受影响的目录一层） | **O(变化数)** |
| L1 | 日志缺失/过期（Linux 常态、Windows journal 被删、macOS UUID 变了） | **目录 mtime/ctime 短路**：复用它缓存住的子项列表，但逐子项 stat 校验 | O(节点数) 但省掉所有 `getdents` |
| L2 | 用户「强制重扫」/ 首次扫描 / 卷变了 | 全量扫描 | O(节点数) |
| L3 | 扫描进行中 | `scan_generation` 递增；旧 generation 的事件抵达前端时被丢弃（防乱序） | — |

#### 「只重扫受影响的目录」的具体做法

USN/FSEvents 给出的是**文件路径**。把它变成「要重扫的目录集合」：

```
changed_dirs = { parent(p) for p in changed_paths } ∪
               { p for p in changed_paths if is_removed_or_renamed }
```

然后对这几十~几千个目录各做一次「一层浅扫」（`getdents64` + 直接子项 `statx`），用结果**替换**该目录在缓存里的直接子项，再沿祖先链重算 delta。**总工作量 O(变更数 × 平均目录大小)**，在「用户删了几个 G 的文件」这种最常见场景下是 **< 100 ms**。

| ncdu 2.6 二进制导出 | 1.4 **billion** 文件 | 流式，不全驻留 | ~21 GiB | **~16 B/文件（磁盘）** | [ncdu binfmt](https://dev.yorhel.nl/ncdu/binfmt) |
| plocate 1.1.x | 27 M 文件 | mmap，不全量入 RAM | 466 MB | **~17.3 B/文件（磁盘）** | [plocate](https://plocate.sesse.net/) |

---

## 正确性陷阱

> 这一节的目标：**每一个会让用户发现「你的数字不对」的地方**。每一条都给出「怎么检测 / 怎么处理」。

### 1. 硬链接重复计数

| 事实 | 来源 |
|---|---|
| GNU `du` 的去重键是 **`(st_dev, st_ino)`**，绝不是单独 inode；`-l`/`--count-links` 关闭去重 | [GNU coreutils `du` manual](https://www.gnu.org/software/coreutils/manual/html_node/du-invocation.html) |
| GNU `du` **只对 `st_nlink > 1` 的非目录**做去重（`!hash_all && !S_ISDIR && 1 < st_nlink`）；`hash_all` 仅在「多参数」或 `-L` 时为真 ⇒ **单参数、无 `-L` 的 GNU du 不去重重复目录**，它靠 `FTS_TIGHT_CYCLE_CHECK` 防环 | [`coreutils/src/du.c`](https://raw.githubusercontent.com/coreutils/coreutils/master/src/du.c) |
| BSD/macOS `du` **会**去重「有多个硬链接的目录（典型是 Time Machine 备份）」，每次运行只算一次 | [`du(1)` Xcode man page](https://keith.github.io/xcode-man-pages/du.1.html) |
| `ncdu` 明确声明**不支持目录硬链接与 firmlink**，会被扫描并重复计数 | [ncdu manual](https://dev.yorhel.nl/ncdu/man) |
| WizTree 声称「correctly handles hard linked files (doesn't count them more than once)」；其 FAQ 又说 NAS 上「文件分配总和几乎总是略小于 Windows 报告的 Space Used」（因为 NTFS 元数据不算文件） | [WizTree](https://diskanalyzer.com/)、[FAQ](https://diskanalyzer.com/faq) |

**Windows 上的身份 API**：

- 正确做法是 `GetFileInformationByHandleEx(h, FileIdInfo, …)` → `FILE_ID_INFO { ULONGLONG VolumeSerialNumber; FILE_ID_128 FileId; }`。Microsoft 原文：两者组合才能唯一标识一台机器上的文件。（[FILE_ID_INFO](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)，Windows 8 / Server 2012+）
- 传统 `BY_HANDLE_FILE_INFORMATION`（`dwVolumeSerialNumber` + `nFileIndexHigh<<32|nFileIndexLow`）是 64 位，**在 ReFS 和 SMB 上不可靠**——这正是 `READ_USN_JOURNAL_DATA_V1` / `FILE_ID_128` 存在的原因（[READ_USN_JOURNAL_DATA_V1](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v1)）。
- `FILE_ID_INFO` **需要为每个文件开一个 handle**；`FindFirstFileEx` 拿不到任何 file id；`FILE_ID_BOTH_DIR_INFO` 直接给 64 位 `FileId`（不用开 handle，最便宜），但继承 ReFS/SMB 的注意事项。**⇒ 基于 `FindFirstFile` 的扫描器根本不可能正确去重硬链接**，必须走 MFT 或 per-file `CreateFile` + `FileIdInfo`。

**macOS**：`st_dev` + `st_ino` 够用；要跟系统口径一致就用 `ATTR_CMN_FILEID`（64 位）+ `ATTR_CMN_DEVID`。APFS 的 file id 是合成的 64 位，别假设它是原始 catalog node id。

**去重本身的坑**：

- **ID 复用**：扫描期间删除再重建会让两个不同文件撞 ID → **少算**。对策：把 ID 与 `st_ctime`/birthtime（Windows 上是 `ChangeTime`）配对，或用 `(dev, ino, ctime, size)` 做键。Windows 的 MFT `FileReferenceNumber` 高 16 位是**序列号**，天然抗复用——这是它相对 `FILE_ID_INFO` 的优势。
- **seen-set 大小无界**：百万文件 → 几百 MB ~ GB 级。duh 用 SQLite 解决（「memory-bounded and streaming; trees of ~4 million files are fine」）。
- **Bloom filter 在这里是不安全的**：假阳性会静默丢掉一个真实文件的字节。必须精确成员判定，或显式接受内存上限并溢出。
- duh 拒绝在同一个 DB 里扫描重叠的根，因为「indexing the same files twice poisons clone/hardlink analysis」——这个警告对任何缓存设计都成立。

### 2. 符号链接

- 用 `Path::symlink_metadata()`，**不要** `Path::metadata()`（后者会跟随链接）。Rust 文档明确说明。（[Path docs](https://doc.rust-lang.org/std/path/struct.Path.html)）
- `du` 默认 `-P`（physical）；`-H`/`-D` = 只跟随命令行参数；`-L` = 全部跟随（`FTS_LOGICAL`），后者隐含 `-noleaf`。`find` 同语义，且 `-L` 下「无法解析时用链接自身的属性」。（[findutils: Symbolic Links](https://www.gnu.org/software/findutils/manual/html_node/Symbolic-Links.html)）
- **环检测**：`-L` 下必须维护**祖先栈上的 `(dev, ino)` 集合**（大小只受深度限制），拒绝进入已在栈上的目录。GNU `du` 靠 `fts` 的 `FTS_DC` 并打印「WARNING: Circular directory structure」，还有个特例避免把 bind mount 误报成环。
- **macOS 的 Finder alias 不是符号链接**：它是含 alias/bookmark 记录的**普通文件**，POSIX 层不会解析，任何 POSIX 强度的扫描器都会把它当小文件 —— 这**反而是对的**（alias 目标在其真实位置计数，不会重复计数）；但「用 alias 判断重复目录」一定错。
- 悬空/自指向 symlink：`FTS_SLNONE` → GNU `du` 打印 `cannot access` 并且**返回失败**，不会静默算 0。

### 3. 挂载点 / 跨文件系统

- `du -x` = `FTS_XDEV`：**只对根以下的目录生效**，命令行参数本身永不被排除。
- **⚠️ Linux 上 `st_dev` 不够**：bind mount 共享 `st_dev` 但是不同挂载；overlayfs / btrfs subvolume 也会搅浑。**正确做法是用 `statx` 的 `STATX_ATTR_MOUNT_ROOT`**（Linux 5.8+，文档原文「The file is the root of a mount」）+ `STATX_MNT_ID`（5.8）/ `STATX_MNT_ID_UNIQUE`（6.8，保证运行期内不复用）。这同时也是**避免 bind mount 重复计数**的准确信号。（[statx(2)](https://man7.org/linux/man-pages/man2/statx.2.html)）
- **macOS**：`ATTR_DIR_MOUNTSTATUS` 的 `DIR_MNTSTATUS_MNTPOINT` / `DIR_MNTSTATUS_TRIGGER`（Apple 自己的 `FSMegaInfo` 示例就打印这些）；挂载点枚举用 `getfsstat(2)` 的 `f_fstypename` + `f_mntonname`。
- **Windows**：卷挂载点与 junction 都是 **reparse point**，不是「设备」。`FILE_ATTRIBUTE_REPARSE_POINT` + tag 区分 `IO_REPARSE_TAG_MOUNT_POINT`（junction / 卷挂载点）、`IO_REPARSE_TAG_SYMLINK`、`IO_REPARSE_TAG_APPEXECLINK`、OneDrive 占位符（`IO_REPARSE_TAG_CLOUD*`）。**Windows 自己的文件夹大小算法就「检测 reparse point 并不递归进去」**（[Raymond Chen](https://learn.microsoft.com/en-us/previous-versions/technet-magazine/hh148159(v=msdn.10))）。跨卷判断用 `GetFileInformationByHandleEx(h, FileStorageInfo)` 拿 `VolumeSerialNumber` + `FileSystemName`。
- **网络 / FUSE / SMB**：**检测并警告，不要静默穿越**。Linux 用 `statfs().f_type` magic（`NFS_SUPER_MAGIC 0x6969`、`SMB2_MAGIC_NUMBER 0xfe534d42`、`CIFS_MAGIC_NUMBER 0xff534d42`、`FUSE_SUPER_MAGIC 0x65735546`）；macOS 用 `f_fstypename ∈ {nfs, smbfs, afpfs, webdav, ftp, autofs}`；Windows 用 `GetDriveType() == DRIVE_REMOTE`。理由：属性缓存导致 size 陈旧、SMB/FUSE 上 `st_blocks` 常为 0 或假的、硬链接/file-id 语义不同、`statfs` 空闲空间是服务端的；`notify` 文档也说 NFS「may not emit any events」。
- **macOS firmlink 与 `/System/Volumes/Data`（会重复计算整个数据卷）**：Catalina 起系统拆成只读 System 卷 + 可写 Data 卷，APFS firmlink 让一个卷的目录出现在另一个卷的命名空间里，`readlink` 看不到。**天真地遍历 `/` 会把同一批字节在 `/Users/...` 下算一次、在 `/System/Volumes/Data/Users/...` 下再算一次。** DaisyDisk 为此做了专门特例（「revealing some obscure system items such as non-linked content of the 'firmlinked' Data volume」），ncdu 直接声明不支持。**建议：把 `/System/Volumes/**` 整体排除，只扫 `/` 作为并集；绝不两个都扫。** `ATTR_CMN_FLAGS` 的 `SF_FIRMLINK` 可以判 firmlink，但**我没有找到 Apple 明确文档化的第三方 firmlink 检测 API**——先按「特例 + 断言 + 真机自检」处理。

### 4. APFS clone / 共享 extent / 快照

- `clonefile(2)`（`cp -c`）创建共享物理块的拷贝；**APFS 对每个 clone 都报完整大小**，所以按 `st_blocks` 求和会重复 2–N 倍。duh：「a directory can 'contain' 20 GB and free 3 MB when you delete it… can overstate its real disk cost by **10–100x**」。真实世界里制造 clone 的：`pnpm`、`uv`、Postgres `FILE_COPY` 分支、git worktree、Time Machine。
- **GNU `du` 自己就免责了 CoW**：手册原文——「In file systems that use copy-on-write, if two distinct files share space the output of `du` typically counts the space that would be consumed if all files' non-holes were rewritten, not the space currently consumed.」并建议把 `du` 当作「备份体积估计」而不是「设备占用度量」。**这个框架就是最好的 UX 说法。**
- **duh 的算法（目前最详细的公开描述）**：扫进 SQLite；用 `ATTR_CMNEXT_CLONEID` 识别 clone 家族、用 inode 识别硬链接家族；计算 `freeable(dir)` =「`rm -rf dir` 会还给 `df` 多少」；**一个家族只在成员的「最小公共祖先」记一次账；成员落在被查询目录之外的家族对该目录记 0**；另给 `locked_here` / `clusters`（必须一起删才释放的兄弟集合）。UI 三种口径：**Freeable / Allocated / Logical**。
- **DaisyDisk**：`only the first occurrence of each clone` 记全量、其余记 0；**clone 检测要求 macOS 14 Sonoma 以上**；并诚实承认**部分共享的 clone 无法正确记账**（「macOS does not currently provide any reliable tools to count the partially shared blocks」）。
- **这就是关键**：OS 只给**完整 clone 的家族身份**（`ATTR_CMNEXT_CLONEID`，未文档化、版本相关），**不给 per-extent 共享图**。部分 clone 只能表示为「共享量未知」。**不要在这上面承诺精确。**
- **APFS 本地快照（Time Machine）**会钉住不再被活动文件引用的块，表现为 `df used − Σ(扫描)` 的差额。枚举：`tmutil listlocalsnapshots /`、`diskutil apfs listSnapshots`。DaisyDisk 把它做成可删除的一等条目，并把「hidden space」分解为 purgeable（含快照）/ snapshots / other volumes / still hidden（其他用户 home、Spotlight 索引、文档版本、文件系统开销 ~2–3 GB、文件系统错误）；第三方快照（如 CCC）**不可 purge**，落在 still hidden。
- **三数模型（建议直接采纳）**：**Logical**（`st_size`）、**Allocated**（`st_blocks × 512`）、**Exclusive / Freeable**（删除真正释放多少，共享家族在 LCA 记一次）。三个都显示，并且用一行**对账脚注**把差额说清楚：`Σ exclusive + 快照/purgeable + 文件系统开销 + 不可读 = 卷已用`。
- **架构影响**：clone/hardlink 家族追踪需要一个无界集合；而「LCA 记账」需要**全部家族成员**，所以**单遍流式累加器只能给出「被扫描根的总量」正确，给不出每个子树的正确值**，除非 ① 扫完再定稿，或 ② 从 DB 惰性重算子树的 credit。**这是 clone-aware 分析器最主要的架构约束。**

### 5. 稀疏文件与 NTFS 压缩

- `statx.stx_blocks` 定义：「The number of blocks allocated to the file on the medium, in 512-byte units. (This may be smaller than `stx_size`/512 when the file has holes.)」`st_blocks × 512` = 分配；`st_size` = 逻辑。GNU `du -A`/`--apparent-size` 用 `st_size`，默认用块数。
- 枚举洞：`lseek(fd, off, SEEK_DATA)` / `SEEK_HOLE`；整文件 extent map：`FS_IOC_FIEMAP`。注意 FIEMAP 可能返回 `FIEMAP_EXTENT_UNKNOWN`/`DELALLOC`/`UNWRITTEN`，且它报的是**物理布局**，不等同于所有权（`FIEMAP_EXTENT_SHARED` 只说明「被共享」，**不说明与谁共享**）。
- **Windows**：`GetFileSize` = 逻辑；`GetCompressedFileSize` = 「the actual size allocated on disk for a sparse file. This total does not include the size of the regions which were deallocated because they were filled with zeros.」`FSCTL_QUERY_ALLOCATED_RANGES` 找非零区间（非稀疏文件只回一个区间）。⇒ **Windows 上「分配」就是「簇取整后的逻辑大小」，除非文件是压缩或稀疏的**——这正好等于 Explorer 的 Size on disk 口径。
- **NTFS 压缩文件**：用压缩单元（通常 16 簇），有 slack，所以**不可压缩数据的 allocated 可能大于 logical**。`GetCompressedFileSize` 对压缩和稀疏都返回磁盘占用。
- **NTFS 常驻小文件**：小文件和目录**常驻在 MFT 里**，0 个数据簇 ⇒ **「allocated = 0, logical = N」是正常且正确的**。HFS+/APFS 有 inline small file，btrfs 有 inline extent。
- **Alternate Data Streams（ADS）**：`FindFirstFile` 的 `nFileSize` 只是匿名 `$DATA` 流。额外的流（`Zone.Identifier`、WOF 压缩数据等）会增加 allocated，而 Explorer 的 Size on disk 和大多数扫描器都忽略。要穷尽需要 per-file 枚举流（`FindNextStreamW`），很贵。**可辩护的设计是「不计 ADS，并作为已知缺口标注」。**
- **macOS 资源分支**：`st_size` 不含；`st_blocks` 通常包含。`.DS_Store` 是普通文件、正常计数。**建议主口径用 `st_blocks`，逻辑大小标注为「数据大小，不含元数据/资源分支」。**

### 6. btrfs reflink/CoW/subvolume 与 ZFS

- **`du` 在 btrfs 上会重复计算 reflink。** duh 的 Linux 章节说得最清楚：「btrfs/XFS reflinks (`cp --reflink`, `FICLONE`) share extents but **have no clone id**; FIEMAP can only say an extent is 'shared', not with which file.」⇒ **任何仅靠 FIEMAP 的方案都会高估 reflink 文件的 freeable。** ext4 没有 reflink，所以不受影响。
- **`btrfs filesystem du`** 用 FIEMAP 给出 `total` / `exclusive` / `set shared`，其中 set = 递归集合（会下探 subvolume 但**不下探挂载点**），且「set shared takes into account overlapping shared extents, hence it isn't as simple as adding up shared extents」。**这是「独占 vs 共享」的权威定义，实现如果要做就按它做**，并注意它需要**按集合聚合**，不是 per-file。
- `btrfs filesystem usage` / `df` 是另一个维度：Device size / allocated / unallocated / slack / **Used** / Free(estimated) / Free(statfs) / Data ratio / Metadata ratio / Global reserve / Multiple profiles。**`df` 的 used 与 `du` 的求和在 btrfs 上永远不会相等，这是预期行为。**
- 要真正知道「哪些文件共享这个 extent」需要 `BTRFS_IOC_LOGICAL_INO` / `btrfs inspect-internal logical-resolve`（或 `FS_IOC_GETFSMAP`），**很慢**（有内核报告的 logical_ino ioctl 死循环问题）。duperemove 的 FAQ 也承认它「can not resolve which files those extents are shared with」。
- btrfs `compression=zstd|lzo|zlib` 与 `nodatacow`（`chattr +C` / `-o nodatacow`）改变分配语义；用 `statx.stx_attributes` 的 `STATX_ATTR_COMPRESSED` 做标记，**在压缩树上永远不要声称 freeable == logical**。
- **ZFS**：`used`（计入 quota，`used = usedbychildren + usedbydataset + usedbyrefreservation + usedbysnapshots`）、`logicalused`（忽略 compression/copies，更接近应用看到的数据量）、`referenced`（「may or may not be shared with other datasets」）。快照的 `used` 是「仅被该快照引用的空间」，`usedbysnapshots` **不是各快照 used 之和**（因为可能互相共享）。且「Pending changes are generally accounted for within a few seconds」。**ZFS dedup 让 logical 与 physical 的差无法 per-file 计算**——在 per-file UI 里当作「共享量未知」。

### 7. 权限拒绝与 I/O 错误

- **永远不要把不可读子树渲染成 0。** GNU `du` 在 `process_file` 里区分三种情况：`FTS_DNR` → `cannot read directory %s` 且 `ok = false`（但目录自身的块数仍计入）；`FTS_NS`/`FTS_SLNONE` → `cannot access %s`，什么都不计；`FTS_ERR` → 报错但 size 已知。退出码非零。
- **值得抄的 UX**：`ncdu` 用单字符前缀标注节点状态——`!`「读取该目录时出错」、`.`「读取某个子目录时出错，指示的大小可能不正确」、`e`「空目录」。**这正是「真的空」与「读不了」的区别，并且在节点级向上传播。**
- **错误分类必须显式建模，不能塌缩成一个「error」**：`EACCES`/`EPERM`（可用提权逆转）、`EIO`（硬件/介质错误，可能影响同一 stripe 的邻居）、`ESTALE`（NFS 陈旧 handle；NFS 的 `.nfsXXXX`「silly rename」文件是另一个现象）、`ENOENT`（竞态：readdir 与 stat 之间被删）、`ENOTDIR`、`ELOOP`、`ENAMETOOLONG`、`EMFILE`/`ENFILE`/`ENOMEM`（**你自己的资源上限——必须给并发设界**）、`EOVERFLOW`、`EINTR`、`ETIMEDOUT`。
- **macOS TCC**：`~/Desktop`、`~/Documents`、`~/Downloads`、Photos、Mail、Time Machine 备份等需要 **Full Disk Access**；DaisyDisk 的指引还说明**即使以管理员扫描，仍会有几 GB 的 hidden space 属于文件系统自身，这是正常的**。
- **Windows ACL**：`SeBackupPrivilege`（提权后 `AdjustTokenPrivileges` 开启 + `FILE_FLAG_BACKUP_SEMANTICS` 打开 handle）可以绕过 ACL 检查，是备份软件的标准做法。注意：**列目录需要 `FILE_LIST_DIRECTORY`；目录 handle 额外需要 `FILE_FLAG_BACKUP_SEMANTICS`。** `GetFileAttributesEx`/`FindFirstFile` 常能在「打开失败」的文件上成功 ⇒ 对磁盘分析器来说正确的谓词是 **「能 stat」而不是「能读」**。
- **Linux**：`CAP_DAC_READ_SEARCH` 绕过读/搜索权限检查（不绕过文件类型限制），`CAP_DAC_OVERRIDE` 绕过所有 DAC。桌面 app 拿它不现实；**必须老实报 `partial`。**

### 8. 竞态：扫描期间被删除/变化

一个「被观察到大小，然后被删除」的文件会污染父目录的总量——这个数字既不符合 `df` 也经不起重扫。缓解手段按强度排序：

1. **用变更日志/时间点源**（USN journal、FSEvents）。USN 记录按 USN 有序，是元数据变更的**一致时间点视图**，能避免「在两条记录之间创建又删除」这类竞态（按序应用到缓存模型即可）。
2. **持有 fd 再 `fstat`**：Unix 上 `open(path, O_RDONLY|O_NOFOLLOW|O_CLOEXEC)` + `fstat` 保证你量的是**那个 inode**；如果 open 之后被 unlink，你仍能看到它（`st_nlink == 0`）。策略：**`st_nlink == 0` 的从父目录总量里剔除，并记为「扫描期间被删除」，而不是静默放大父目录**。Windows 上 `CreateFile` + `FILE_FLAG_BACKUP_SEMANTICS|FILE_FLAG_OPEN_REPARSE_POINT`（**故意不加 `FILE_SHARE_DELETE`**，让删除短暂阻塞）再 `GetFileInformationByHandleEx`。
3. **检测并标记「扫描期间被改动」**：读目录前后各取一次该目录的 `mtime`/`ctime`，任一变化就把该目录标为 `stale` 并重扫一次。**不要静默发布。**
4. **在根上对账**：始终把 `Σ allocated` 与 `df`/`GetDiskFreeSpaceEx`/`statfs` 比较，**把残差作为显式的 `unaccounted` 桶暴露出来**（这正是 DaisyDisk 的 hidden space 和 duh 的 df-ground-truth 框架）。残差是回答「为什么数字对不上」的唯一诚实答案。

另外：大小可能**变小**（截断），累加器必须做饱和运算、不能假设单调增长；总量**超过** `df used` 通常是重复计数（共享块），跟竞态是两种不同的失效模式。

### 9. Unicode 与大小写

| 平台 | 事实 | 后果 |
|---|---|---|
| macOS / APFS | 规范化不敏感但保留原样；**默认大小写不敏感但保留大小写**（大小写敏感卷是格式化时可选） | `"Café"` 与 `"Cafe\u0301"` 是同一个文件但是不同的 `String`；**大小写敏感卷上 `Foo` 与 `foo` 可以共存**，所以任何基于小写的分组在某些卷上错、在某些卷上对，而你**只能通过查询卷能力才知道是哪种** |
| Windows / NTFS | 默认大小写不敏感（可按目录开启，如 WSL）；比较用卷的 **upcase table**，是**简单 1:1 大写映射**，不做完整 Unicode case folding | `to_lowercase()` 复现不了它的行为（ß、土耳其 i 等） |
| Linux | 文件名是**任意字节串**（除 `NUL` 和 `/`），编码只是惯例；大小写敏感 | 任何假设 UTF-8 的代码都会丢/坏条目 |

**Rust 具体规则**：

- `OsStr` 在 Unix 上是 `[u8]` 包装，Windows 上是 **WTF-8**（允许非法 UTF-16 代理对）。
- **`Path::to_str()` 在非 UTF-8 Unix 名字上返回 `None`；`to_string_lossy()` 是「有损且非单射」的**——两个不同文件会映射到同一个 `String`，**静默破坏重复检测，而且得到的路径无法用于重开文件**。（[Path docs](https://doc.rust-lang.org/std/path/struct.Path.html)）
- 规则：① 端到端用 `OsStr`/`OsString`/`PathBuf`，**绝不把 `String` 当存储或键**；② Unix 上用 `as_bytes().to_vec()` 做键，Windows 上用 `encode_wide()`（`Vec<u16>` 最忠实）；③ 只在**显示**时用 `to_string_lossy()`，并额外提供转义视图；④ `Path::components()` **只做词法规范化**（重复分隔符、`.`、尾部分隔符），`a/c` 与 `a/b/../c` 是**不同**的（因为 `b` 可能是符号链接）——所以身份判定只能用 file id。
- 确实需要大小写/规范化不敏感比较时（例如「这个目录是不是我已经扫过的那个」）：Windows 用 `CompareStringOrdinal(..., TRUE)`；macOS 用 `CFStringCompare` + `kCFCompareCaseInsensitive|kCFCompareNonliteral`（或 `precomposedStringWithCanonicalMapping` + case-fold）；Linux 就是字节比较。**永远不要 `to_lowercase()`。**
- **磁盘分析器上最容易踩的几个点**：① **不要按文件名分组重复文件**——要按 `(size, 内容哈希)`；② 判断「同一棵树挂载了两次」要按 `(dev, fileid)`；③ 增量缓存以路径字符串为键时，一个只改规范化/大小写的重命名会看起来像「删+建」——macOS/Windows 上应优先用 file id，或接受这种抖动；④ 排除规则/扫描根去重不能用小写路径；⑤ 排序与搜索要用平台 collation（Finder 自然序、`StrCmpLogicalW`、`strcoll`）；⑥ CJK/emoji 的显示宽度是另一个独立的真问题（ncdu 也承认会「garbled」）。

### 10. 逻辑大小 vs 磁盘占用（怎么报才可信）

| 工具 | 「大小」 | 「占用空间」 | 备注 |
|---|---|---|---|
| **Windows Explorer** | Σ `WIN32_FIND_DATA.nFileSize`（逻辑）；**可能陈旧**（正在写入的文件在 handle 关闭前不准） | 若卷支持压缩且文件是 `FILE_ATTRIBUTE_COMPRESSED`/`SPARSE_FILE` → `GetCompressedFileSize`；否则 **`nFileSize` 向上取整到簇** | 它**不去重硬链接**、跳过无权限子目录、**检测 reparse point 且不递归**、符号链接按 0 计；**不计** MFT 常驻数据、文件名、目录项、元数据、ADS。原文：「aren't meant to be a byte-for-byte accounting… just a rough estimate」 |
| **Finder** | 逻辑大小 | 有「占用空间」 | 与 `du`/DaisyDisk 都不一致（DaisyDisk 专门有一页讲这个） |
| **`du -sh --apparent-size`** | 对**普通文件和符号链接**求 `st_size`，其它文件类型不计 | 默认块数 | |
| **WizTree** | `Size` 列 | `Allocated` 列（簇取整；压缩文件 Allocated < Size） | 与 Explorer 口径对齐 |
| **ncdu** | `--apparent-size` | 默认 `--disk-usage`，会话内用 `a` 切换 | 还有 `u` 切换的 **shared/unique 列**——现有工具里最接近我们需要的 UX |

**建议的 UX**：

1. 每个节点**始终给两个明确标注的数字**：**逻辑大小（数据）** 与 **磁盘占用（分配）**。
2. 存在共享时给**第三个**：**独占 / 可释放**（「`rm -rf` 会释放多少」），并标注「与另外 N 处共享」而不是把字节静默归给某一方。duh 的 `locked_here`/「delete-together clusters」是兄弟共享场景的正确思路。
3. **始终显示对账脚注**：`Σ 独占 + 快照/purgeable + 文件系统开销 + 不可读/不可测 = 卷已用`，残差显式列出。
4. **给部分结果打徽章**：`partial`（有不可读子树）、`stale`（扫描期间变化）、`shared-unknown`（部分 clone、无 clone id 的 reflink、ZFS/Windows dedup）、`network`（远端/FUSE，大小仅供参考）。
5. 默认排序：**清理工具**按 Exclusive；**取证工具**按 Allocated。切换要显眼。
6. **有读错误的子树，永远不要给一个自信的数字。**

### 11. 其他著名「翻车点」

- **ReFS block cloning**：`FSCTL_DUPLICATE_EXTENTS_TO_FILE` 让多个文件共享逻辑簇，写时按**簇粒度**分配（对共享 4 KB 簇写 1 字节就复制 4 KB）。**Windows 11 Moment 5（KB5034848）起，受支持的复制操作会原生使用 block cloning。** 后果：ReFS 上普通文件夹复制就会产生共享 extent，而 `GetCompressedFileSize`/Size on disk **会按完整大小报每个文件**（因为 block cloning 不给文件打压缩/稀疏标记）⇒ **per-file 扫描器会严重高估**。⚠️ **我没有找到 Microsoft 明确文档化「block clone 与 `GetCompressedFileSize` 的交互」；这是从 API 定义推出的推断，上线前必须实测。**
- **Windows Server Data Deduplication**：chunk store，文件保留表观大小，节省只能通过 `Get-DedupStatus`/`Get-DedupVolume` 和 `df` 看到。per-file 工具看不见。与 ZFS/btrfs dedup 同类。
- **稀疏 VM 镜像**（`Docker.raw`、`*.qcow2`）：duh 明确按分配块计——「a '64 GB' sparse image that occupies 9 GB counts as 9 GB. This is correct for 'what would deleting free' but differs from what Finder shows.」**两种口径都会让用户困惑，所以必须标口径。**
- **挂载的磁盘镜像 / `.sparsebundle`**：band 是独立文件；同时扫挂载点和背后的镜像文件会重复计数。
- **配额与预留**：ZFS/btrfs 的 quota 记账（`used`/`referenced`）与「文件大小之和」是**不同的数字**；两个都显示能避免「为什么数据集说 50 GB 而 du 说 30 GB」。
- **`df` 本身不是瞬时的**：btrfs 延迟更新、ZFS「pending changes… within a few seconds」、APFS purgeable 延迟（DaisyDisk：「the system often delays updating the size of the purgeable space by few minutes」）。**不要把 `df` 的差值当操作后的即时 oracle。**
- **溢出**：`statx` 是 64 位的，但 `EOVERFLOW` 和 `i32` 计数器仍会咬人（ncdu 记录了 8 EiB 与 32 位 item count 的截断）。
- **扫描根本身是符号链接 / 尾随斜杠 / `..` 组件 / 大小写与规范化别名**：同一个目录被两条路径到达时，必须在**入口**按 `(dev, fileid)` 去重，不能按字符串。

---

## 推荐设计

### 1. 分层策略与决策表

```
                        ┌─────────────────────────────────────────┐
                        │ Tier 0  目录册缓存 + 变更日志            │
                        │  命中 → 0.2–3 s，且只重扫变了的目录      │
                        └───────────────┬─────────────────────────┘
                                        │ 未命中 / 日志失效
                        ┌───────────────▼─────────────────────────┐
                        │ Tier 1  平台整卷 / 批量快路径            │
                        │  Win(提权·NTFS): 直读 $MFT   → 3–8 s    │
                        │  macOS: getattrlistbulk      → 8–20 s   │
                        │  Linux(XFS/btrfs, 可选, v2): bulkstat   │
                        └───────────────┬─────────────────────────┘
                                        │ 不支持 / 无权限 / 非原生 FS
                        ┌───────────────▼─────────────────────────┐
                        │ Tier 2  可移植批量回退（必须做对）       │
                        │  每平台「批量目录枚举」+ work-stealing   │
                        │  Win: FileIdBothDirectoryInfo → 15–30 s │
                        │  macOS: getattrlist 逐条兜底 → 20–35 s  │
                        │  Linux: getdents64 + statx   → 20–30 s  │
                        └───────────────┬─────────────────────────┘
                                        │ 批量接口也失败（网络/FUSE/怪 FS）
                        ┌───────────────▼─────────────────────────┐
                        │ Tier 3  read_dir + symlink_metadata      │
                        │  40–90 s，必须给「慢速模式」提示与进度条 │
                        └─────────────────────────────────────────┘
```

**每平台的路径选择（实现时的 if/else 表）**：

| 平台 | 条件 | 走哪条 | 关键 API |
|---|---|---|---|
| Windows | NTFS + 已提权（`\\.\X:` 打开成功） | **Tier 1a** | `FSCTL_GET_NTFS_VOLUME_DATA` → 直读 `$MFT` → `mft` crate 解析 |
| Windows | NTFS + 未提权 | **Tier 2** | `NtQueryDirectoryFile` / `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` |
| Windows | 非 NTFS（FAT32/exFAT/ReFS）或网络盘 | **Tier 2/3** | `FindFirstFileExW(FindExInfoBasic, FindExSearchNameMatch)` |
| macOS | 任意本地卷，`getattrlistbulk` 首次调用成功 | **Tier 1b** | `getattrlistbulk` + 128 KiB 缓冲 |
| macOS | `getattrlistbulk` 返回 `ENOTSUP`/`EIO` | **Tier 2** | 逐条 `getattrlist`（**仍是批量接口的降级，不是 `readdir+stat`**） |
| macOS | 网络卷（`f_fstypename ∈ {smbfs, nfs, afpfs, webdav}`） | **Tier 3** | 并给「大小仅供参考」徽章 |
| Linux | ext4/ext2/ext3/btrfs/xfs | **Tier 2** | `getdents64` + `statx`（`d_type` 过滤） |
| Linux | XFS（后续优化） | Tier 1c | `XFS_IOC_FSBULKSTAT` 拿 size + `getdents64` 补名字 |
| Linux | btrfs（后续优化） | Tier 1c | `BTRFS_IOC_TREE_SEARCH_V2` |
| 全部 | 有 Tier 0 缓存且日志可追平 | **Tier 0** | USN journal / FSEvents / mtime 短路 |
| 全部 | `/proc`, `/sys`, `/dev`, `/run`, `~/.Trash`, `/System/Volumes/**` | **永不扫描** | 硬编码排除 |

### 2. 吞吐预算（2 TB / ≈2 M 文件 + ≈200 k 目录，冷缓存）

| 阶段 | macOS/APFS | Windows 提权 | Windows 不提权 | Linux/ext4 |
|---|---|---|---|---|
| 目录枚举 | `getattrlistbulk` 6–15 s | 读 MFT 1–2 s | `FileIdBothDirectoryInfo` 12–25 s | `getdents64`+`statx` 16–26 s |
| 解析 / 建树 | 1–3 s | 2–3 s（4–8 段并行解析 MFT） | 1–3 s | 1–3 s |
| 聚合 + 事件发布 | 0.5–2 s | 0.5–2 s | 0.5–2 s | 0.5–2 s |
| **合计** | **8–20 s** | **4–8 s** | **14–30 s** | **18–31 s** |
| 相对 60 s 的余量 | 3–7× | 8–15× | 2–4× | 2–3× |
| 焦点首屏 | <100 ms | <100 ms | <100 ms | <100 ms |
| 内存峰值 | 60–120 MB（全节点）/ 10–25 MB（仅目录） | 同 | 同 | 同 |

**预算的来源**（不是拍脑袋）：

- Linux 的 16–26 s：gdu 冷缓存实测 400 k 文件 4.716 s ⇒ **85 k 文件/s** ⇒ 2 M ≈ 24 s；diskus 实测 229 k 文件/s ⇒ 2 M ≈ 9 s。取区间 **9–24 s**，加上 2× 余量给随机读 IOPS 波动。
- macOS 的 6–15 s：dumac 热缓存 786 k 文件/s，403 k 文件 0.52 s。冷缓存按 diskus 的冷/热比（3.5×）外推 ⇒ ~270 k 文件/s ⇒ 2 M ≈ 7.5 s。取 6–15 s。
- Windows 提权：MFT 2–2.5 GB 顺序读 1–2 s + 200 MB/s/核 × 4 核解析 2–3 s。
- Windows 不提权：`FileIdBothDirectoryInfo` 是每目录一次批量调用，与 Linux 同量级但少了 `statx`，所以比 Linux 略快。

### 3. 关键签名与伪代码

> 以下代码是**设计示意**，放在本文档里，不落地为单独文件。

#### 3.1 统一接口

```rust
#[derive(Clone, Copy, Debug)]
pub struct EntryMeta {
    pub name_off:  u32,          // 指向名字 arena
    pub name_len:  u16,
    pub kind:      EntryKind,    // Dir | File | Symlink | Other
    pub size_disk: u64,          // 分配大小（主口径）
    pub size_logical: u64,
    pub file_id:   u64,          // 卷内稳定 inode/FileId
    pub dev:       u32,          // 卷 id
    pub nlink:     u32,
    pub mtime:     i64,
    pub flags:     u32,          // 可见性 / 权限 / 挂载点 / firmlink / reparse
    pub error:     i32,          // 0 = OK
}

/// 每个平台实现一个「读一个目录的直接子项 + 元数据」的批量原语。
/// 契约：绝不递归；绝不 per-entry 系统调用（除 Linux 无可避免的 statx）。
pub trait DirReader {
    fn read_dir(&mut self, path: &Path, out: &mut Vec<EntryMeta>) -> Result<(), ScanError>;
}
```

#### 3.2 macOS 快路径（`getattrlistbulk`）

```rust
use std::mem::MaybeUninit;
use std::os::fd::RawFd;

// 这些常量 libc 都有：ATTR_CMN_NAME / ATTR_CMN_RETURNED_ATTRS /
// ATTR_CMN_OBJTYPE / ATTR_CMN_FILEID / ATTR_CMN_DEVID / ATTR_CMN_MODTIME /
// ATTR_CMN_FLAGS / ATTR_DIR_ENTRYCOUNT / ATTR_DIR_MOUNTSTATUS /
// ATTR_FILE_ALLOCSIZE / ATTR_FILE_DATALENGTH / ATTR_FILE_LINKCOUNT /
// ATTR_BIT_MAP_COUNT / FSOPT_PACK_INVAL_ATTRS / FSOPT_ATTR_CMN_EXTENDED /
// ATTR_CMNEXT_CLONEID / DIR_MNTSTATUS_MNTPOINT。
//
// ⚠️ 我逐条 diff 了 [XNU bsd/sys/attr.h](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)
// 的 105 个常量与 libc 0.2 的 81 个：**libc 缺 ATTR_CMN_ERROR（0x20000000）
// 与 ATTR_CMNEXT_CLONE_REFCNT（0x1000）**，必须自己声明：
const ATTR_CMN_ERROR: u32 = 0x2000_0000;
const ATTR_CMNEXT_CLONE_REFCNT: u32 = 0x0000_1000;
// （其余缺失的都是 SETMASK/VALIDMASK 与几个已废弃常量，扫描用不到。）

fn bulk_scan(dirfd: RawFd, buf: &mut [u8], want_clones: bool) -> std::io::Result<()> {
    // ⚠️ libc::attrlist 的 `reserved` 字段是私有的 Padding<u16>，
    // 不能直接用结构体字面量构造 —— 必须 zeroed 后逐字段赋值。
    let mut al: libc::attrlist = unsafe { std::mem::zeroed() };
    al.bitmapcount = libc::ATTR_BIT_MAP_COUNT;
    al.commonattr = libc::ATTR_CMN_RETURNED_ATTRS
                  | libc::ATTR_CMN_NAME
                  | ATTR_CMN_ERROR                 // libc 没有，见上
                  | libc::ATTR_CMN_OBJTYPE
                  | libc::ATTR_CMN_FILEID
                  | libc::ATTR_CMN_DEVID
                  | libc::ATTR_CMN_MODTIME
                  | libc::ATTR_CMN_FLAGS;
    al.dirattr  = libc::ATTR_DIR_ENTRYCOUNT | libc::ATTR_DIR_MOUNTSTATUS;
    al.fileattr = libc::ATTR_FILE_ALLOCSIZE
                | libc::ATTR_FILE_DATALENGTH
                | libc::ATTR_FILE_LINKCOUNT;
    // volattr 必须为 0，否则 EINVAL

    let mut opts: u64 = libc::FSOPT_PACK_INVAL_ATTRS as u64;
    if want_clones { opts |= libc::FSOPT_ATTR_CMN_EXTENDED as u64; } // 才可请求 CMNEXT_*

    loop {
        let n = unsafe {
            libc::getattrlistbulk(dirfd, (&mut al as *mut libc::attrlist).cast(),
                                  buf.as_mut_ptr().cast(), buf.len(), opts)
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            // ENOTSUP / EIO / EPERM（网络卷、FUSE）→ 调用方降级到逐条 getattrlist
            return Err(e);
        }
        if n == 0 { return Ok(()); }               // 目录读完；继续读必须先 lseek(0) 或重开 fd

        let mut p = buf.as_ptr();
        for _ in 0..n {
            let group_len = unsafe { (p as *const u32).read_unaligned() } as usize; // 8B 对齐
            let group_end = unsafe { p.add(group_len) };
            let mut q = unsafe { p.add(4) };

            let returned = unsafe { (q as *const libc::attribute_set_t).read_unaligned() };
            q = unsafe { q.add(std::mem::size_of::<libc::attribute_set_t>()) };

            // ATTR_CMN_ERROR 紧跟 RETURNED_ATTRS 之后（如果请求了）
            let entry_err = if returned.commonattr & libc::ATTR_CMN_ERROR != 0 {
                let v = unsafe { (q as *const u32).read_unaligned() }; q = unsafe { q.add(4) }; v
            } else { 0 };

            // ATTR_CMN_NAME 是 attrreference_t，偏移**相对于引用自身地址**
            let name_ref = unsafe { (q as *const libc::attrreference_t).read_unaligned() };
            let name_ptr = unsafe { q.offset(name_ref.attr_dataoffset as isize) } as *const u8;
            let name = unsafe { std::slice::from_raw_parts(name_ptr, name_ref.attr_length as usize - 1) };
            q = unsafe { q.add(8) };

            // 后续属性必须按 attrlist 的 bit 顺序 + 自然对齐逐个读，
            // 不能按固定 struct —— 缓冲区里有 padding。
            // ATTR_CMN_OBJTYPE(u32) 决定后续该读 dirattr 还是 fileattr。
            // ...（省略具体字段读取，规则：只读 `returned` 里置位的属性）

            let _ = (name, entry_err);
            p = group_end;
        }
    }
}
```

**实现要点回顾**：① `bitmapcount = ATTR_BIT_MAP_COUNT (=5)`；② 必须请求 `ATTR_CMN_NAME` 与 `ATTR_CMN_RETURNED_ATTRS`（SDK 里就是 `ATTR_BULK_REQUIRED` 宏）；③ `volattr` 必须为 0；④ `reserved` 是私有字段，用 `zeroed()`；⑤ `options` 在 libc 里是 `u64`，常量是 `u32`，要显式转换；⑥ **libc 缺 `ATTR_CMN_ERROR`，要自己定义 `0x2000_0000`**；⑦ 请求 `ATTR_CMNEXT_*` 必须先置 `FSOPT_ATTR_CMN_EXTENDED`（同理 `ATTR_CMN_GEN_COUNT`/`DOCUMENT_ID` 也要它，否则 `EINVAL`）；⑧ **每个 group 用 `length` 步进，但必须把每次读取 clamp 到 `attrBufSize`——[真机实测] length 字段可能是「完整逻辑大小」而不是「已写入字节数」**；⑨ **严格按 `ATTR_CMN_RETURNED_ATTRS` 置位的 bit 从低到高解析，不要硬编码字段顺序**（`ERROR` 在 `NAME` **之前**）；⑩ 返回 0 后必须 lseek/重开；⑪ `ERANGE` 说明单个 group 放不下（不会返回部分条目）⇒ 增长缓冲重试；⑫ 实战缓冲 128 KiB。

#### 3.3 Windows 快路径（直读 `$MFT`）

```rust
// 需要 `windows-sys` 或 `windows` crate；逻辑示意图
unsafe fn scan_volume_mft(drive: char) -> Result<Vec<MftNode>, ScanError> {
    // 1) 打开卷 —— 需要管理员；失败（ERROR_ACCESS_DENIED）→ 返回 Err 让调用方走 Tier 2
    let vol_path = format!("\\\\.\\{}:", drive);
    let h = CreateFileW(vol_path, GENERIC_READ,
                        FILE_SHARE_READ | FILE_SHARE_WRITE, null_mut(),
                        OPEN_EXISTING, 0, 0);
    if h == INVALID_HANDLE_VALUE { return Err(ScanError::NeedElevation); }

    // 2) 取 MFT 的物理位置与记录大小 —— Tier 1a 的必备前置调用
    let mut vd: NTFS_VOLUME_DATA_BUFFER = zeroed();
    DeviceIoControl(h, FSCTL_GET_NTFS_VOLUME_DATA, null(), 0,
                    &mut vd as *mut _ as *mut _, size_of::<NTFS_VOLUME_DATA_BUFFER>(),
                    &mut br, null_mut())?;
    // vd.BytesPerCluster, vd.BytesPerFileRecordSegment (通常 1024),
    // vd.MftStartLcn, vd.MftValidDataLength

    // 3) 顺序读 $MFT：从 MftStartLcn * BytesPerCluster 开始，读 MftValidDataLength 字节
    //    4–16 MiB 一块，FILE_FLAG_SEQUENTIAL_SCAN
    let mft_offset = vd.MftStartLcn as u64 * vd.BytesPerCluster as u64;
    let mut buf = vec![0u8; 8 << 20];
    SetFilePointerEx(h, mft_offset as i64, null_mut(), FILE_BEGIN);
    // 4) 按 BytesPerFileRecordSegment 切块，用 4–8 个 rayon 任务并行解析
    //    解析交给 `mft` crate（100% safe Rust），提取：
    //      $STANDARD_INFORMATION(0x10): 时间戳、属性位、owner
    //      $FILE_NAME(0x30):            ParentFileReferenceNumber + 名字（可能有多个 → 硬链接）
    //      $DATA(0x80):                 RealSize(逻辑) / AllocatedSize(分配)
    //      $ATTRIBUTE_LIST(0x20):       分片记录，需要递归拼装
    // 5) 建树：MFT 记录号天然有序 ⇒ 用 Vec<Option<Node32>> 按记录号索引，
    //    比较父引用时只比较低 48 bit（高 16 是序列号，删除重建后会变）。
    //    硬链接：一条记录展开成 N 个目录项，但大小按记录号只记一次。
    todo!()
}
```

**Windows 增量（Tier 0）**：持久化 `(UsnJournalID, NextUsn)`，用 `FSCTL_READ_USN_JOURNAL` + `READ_USN_JOURNAL_DATA_V1{ ReturnOnlyOnClose: 1, ReasonMask: USN_REASON_CLOSE, BytesToWaitFor: 0 }` 做「每文件每个 open/close 周期一条记录」的增量。**四种失效都要退回全量重扫**：`UsnJournalID` 不匹配、`StartUsn < FirstUsn`（`ERROR_JOURNAL_ENTRY_DELETED`）、`FirstUsn < LowestValidUsn`、`ERROR_JOURNAL_DELETE_IN_PROGRESS`。

**提权形态（重要）**：**Sift 不应该 `requireAdministrator`。** 主进程保持普通权限；用户点「快速全盘扫描」时按需 UAC 启动一个**独立的提权 helper**，通过命名管道把结果流回主进程，扫完即退出。这正是 Microsoft Q&A 里给出的建议模式，也是 Everything Service 的形态。

#### 3.4 可移植回退（Tier 2）

```rust
/// work-stealing 的深度优先遍历 + 有界优先级注入
fn traverse(root: NodeId, focus: Option<PathBuf>, pool: &rayon::ThreadPool) {
    // 每个 worker：LIFO deque（DFS，见 ripgrep 的理由）
    // + 一个共享的 focus deque：装「用户视口路径及其直接子目录」，优先 drain
    // 目录条目数 > SPLIT_THRESHOLD (≈4096) 时，把条目切成 chunk 作为独立工作项
    //   ⇒ 避免单个大目录成为尾延迟
    pool.scope(|s| {
        s.spawn(|_| walk(root, focus.clone()));
    });
}

unsafe fn walk(dir: NodeId, focus: Option<PathBuf>) {
    // 1) read_dir（platform batched）
    //    Linux:   openat(O_RDONLY|O_DIRECTORY|O_CLOEXEC|O_NOATIME) → getdents64 循环
    //             → 对 DT_REG / DT_UNKNOWN 用 **dirfd + 相对名** 发 statx
    //                mask = STATX_TYPE|STATX_SIZE|STATX_BLOCKS|STATX_INO|STATX_NLINK
    //                        |STATX_MTIME|STATX_MNT_ID
    //             → DT_DIR/DT_LNK 直接处理，不发 statx
    //    macOS:   getattrlistbulk(128 KiB)；失败 → 逐条 getattrlist
    //    Windows: GetFileInformationByHandleEx(FileIdBothDirectoryInfo, 256 KiB–1 MiB)
    //             失败 → FindFirstFileExW(FindExInfoBasic, FindExSearchNameMatch)
    // 2) 对每个条目：
    //    - 挂载点/跨卷/reparse point/firmlink → 标记并停止下探
    //    - (dev, file_id) 已在 seen 集合 → 记为 hardlink，size 记 0
    //    - 目录 → 打优先级后入队；文件 → 直接 add_delta(parent, +size) 并进 top-K
    // 3) add_delta 沿 parent 链上行（O(depth)），批量 flush 事件
}
```

**Linux 关键细节**：`O_NOATIME`（需要文件 owner 或 `CAP_FOWNER`，失败就忽略）避免扫描本身改 atime；`getdents64` 的 `DT_UNKNOWN` 必须走 `statx` 兜底；跨卷检测**不要用 `st_dev`**，用 `statx` 的 `STATX_ATTR_MOUNT_ROOT` + `STATX_MNT_ID_UNIQUE`。

### 4. 依赖集（版本为 2026 年当前 crates.io 稳定版）

| crate | 版本 | 用途 | `unsafe` / FFI | 备注 |
|---|---|---|---|---|
| `tauri` | **2.11.6** | 壳（已有） | 否 | |
| `notify` | **8.2.0** | 变更通知（已有；macOS 默认走 FSEvents） | 否 | 项目当前锁在 7，可升 |
| `trash` | **5.2.9** | 回收站（已有） | 否 | |
| `sysinfo` | **0.39.6** | 卷枚举（已有） | 否 | |
| `serde` / `serde_json` | **1.0.229 / 1** | IPC | 否 | |
| **`libc`** | **0.2.189** | **macOS `getattrlistbulk` / `attrlist` / `attrreference_t` / 大部分 `ATTR_*` 常量；Linux 的 `statx` 结构** | **`unsafe` 调用，但无需自写 `extern "C"`** | ✅ **已核实 libc 0.2 提供 `getattrlist`, `getattrlistbulk(dirfd, *mut c_void, *mut c_void, size_t, u64) -> c_int`, `attrlist`, `attrreference_t`, `attribute_set_t`, `ATTR_BIT_MAP_COUNT`, `ATTR_CMN_RETURNED_ATTRS`, `ATTR_CMNEXT_CLONEID`, `FSOPT_PACK_INVAL_ATTRS`, `ATTR_DIR_MOUNTSTATUS`, `DIR_MNTSTATUS_MNTPOINT`**。⚠️ 两个坑：① `attrlist.reserved` 是私有 `Padding<u16>` ⇒ 必须 `zeroed()` 构造；② **我逐条 diff 后确认 libc 缺 `ATTR_CMN_ERROR` 与 `ATTR_CMNEXT_CLONE_REFCNT`（105 个常量里只有 81 个），要自己声明** |
| **`rustix`** | **1.1.5** | Linux 的 `getdents`（经 `fs::Dir`/`DirEntry`）与 `statx` | 否（内部封装） | ✅ 已在源码确认 `getdents`/`getdents_uninit` 走 `__NR_getdents64`；`fs::statx` 公开。**Linux 路径可以是 100% safe Rust** |
| `windows-sys` | **0.61.2** | Windows Win32 调用 | **`unsafe` FFI**（官方元数据生成，比手写 `extern` 可靠） | `DeviceIoControl`/`CreateFileW`/`GetFileInformationByHandleEx`/`NtQueryDirectoryFile` |
| **`mft`** | **0.7.0** | **解析 `$MFT`**（Tier 1a 的核心） | **无 `unsafe`**（100% safe Rust，跨平台） | MIT/Apache；带 `PERF.md`；把「从 `\\.\C:` 读出 `$MFT`」留给你 |
| **`ntfs`** | **0.4.0** | 备用：按需读 NTFS 目录索引 | **无 `unsafe`**，`no_std`+`alloc` | 不支持缓存/压缩/reparse point/安全描述符 ⇒ 只作补充 |
| `usn-journal-rs` | 0.4.1 | USN journal 增量 | 小 | 社区小项目，需自行审计；也可以直接用 `windows-sys` 自己写 ~300 行 |
| `rayon` | **1.12.0** | work-stealing 遍历 | 否 | 或自写 crossbeam-deque |
| `crossbeam-channel` | **0.5.17** | 有界结果通道 | 否 | |
| `crossbeam-deque` | 0.8.x | 自写 work-stealing | 否 | |
| **`id-arena`** | **2.3.0** | 节点 arena（可选） | **`#![forbid(unsafe_code)]`** | 或直接手写 `Vec<Node32>` |
| `rustc-hash` | **2.1.3** | `FxHashMap<u64, u32>`（硬链接去重） | **无 `unsafe`** | 比 std SipHash 快数倍 |
| `ahash` / `foldhash` | **0.8.12 / 0.2.0** | 备选 hasher | 小 | `hashbrown` 0.17 默认已是 foldhash |
| `parking_lot` | **0.12.5** | 锁 | 小 | |
| `smallvec` | **1.16.1** | 内联 top-K | 小 | |
| `compact_str` | **0.10.0** | UI 热路径短字符串（≤24 B 内联） | 多（设计如此） | **不要全树用** |
| `bitvec` | **1.1.1** | 布尔标记位图 | 小 | 或裸 `u64` |
| **`zerocopy`** | **0.8.58** | 零拷贝解析 packed 结构（`dirent64`/`FILE_ID_BOTH_DIR_INFO`/MFT 记录头） | 多（设计如此） | **你自己代码里没有 `unsafe`** |
| `memmap2` | **0.9.11** | Tier 0 归档映射 / 超预算溢出 | 小 | |
| `rkyv` | **0.8.18** | Tier 0 零拷贝归档（可选） | 很多 | 需要格式版本号 |
| `redb` / `rusqlite` | **4.3.0 / 0.40.2** | Tier 0 持久化（二选一） | 小 / thin FFI | **不要 `sled`**（已停滞） |
| `tempfile` | **3.27.0** | 溢出文件 | 小 | |
| `blake3` / `xxhash-rust` | — | 内容哈希（重复文件检测，E5） | 小 | 本轮未调研版本 |
| `walkdir` | 2.5.0 | **仅 Tier 3 兜底** | 否 | 不要用于主路径 |

**纯 Rust 可行性结论（分平台）**：

| 平台 | 纯 Rust？ | 说明 |
|---|---|---|
| **Windows** | **解析层是**（`mft` / `ntfs` 都是 100% safe Rust，无需 C/C++ 工具链）；**系统调用层需要 FFI**（`windows-sys`） | 唯一必须 FFI 的是「打开卷 / `DeviceIoControl` / `NtQueryDirectoryFile`」这几个调用，由官方 crate 生成，**不需要额外构建依赖** |
| **macOS** | **不需要 C 代码，但需要 `unsafe` 调用 `libc::getattrlistbulk`** | `libc` 已经提供全部结构体与常量，**无需 `cc`、无需 build script、无需 C 编译器**。若想要 safe 封装可用 `getattrlistbulk-rs`（0.1.0，但很新、需审计） |
| **Linux** | **可以完全 safe**（`rustix` 封装 `getdents64` 与 `statx`） | 唯一可能需要 `unsafe` 的是 `O_NOATIME`（rustix 已支持） |

⇒ **整个项目不需要任何 C/C++ 依赖，也不需要 vendored 的第三方驱动。** `libc` + `windows-sys` + `rustix` + `mft` 就够了。

### 5. 实施顺序建议（按 ROI 排序）

| 优先级 | 做什么 | 为什么 |
|---|---|---|
| **P0** | Tier 2 的**每平台批量枚举** + work-stealing + arena 内存布局 | 这是「达标」的地基，也是所有回退路径的归宿 |
| **P0** | 「仅目录 + 精确残差 + top-K」的内存模型 | 唯一能同时满足 ~100 MB 与「什么最占空间」的方案 |
| **P0** | 三数模型（Logical / Allocated / Exclusive）+ `df` 对账脚注 | 可信度的地基；不做的话所有数字都会被质疑 |
| **P1** | 焦点优先的有界队列 + 批量事件流（16–50 ms flush）+ 保留部分结果的取消 | 直接对应 R2.2/R2.4/R3.2 的验收标准 |
| **P1** | macOS `getattrlistbulk` | 单独一项就有 3.3× 收益，是 macOS 上最大的杠杆 |
| **P1** | Windows `FileIdBothDirectoryInfo`（不提权也能用） | Windows 上不提权用户的默认路径 |
| **P2** | Windows 直读 `$MFT` + 独立提权 helper | 把 Windows 从 15–30 s 压到 4–8 s；是差异化卖点 |
| **P2** | Tier 0 缓存 + mtime 短路（先不做日志） | 第二次启动几乎瞬时 |
| **P2** | Windows USN journal / macOS FSEvents 增量 | 真正的 O(变化数) |
| **P3** | APFS clone 家族记账（`ATTR_CMNEXT_CLONEID` + LCA 归账） | 高价值但高风险，必须配 `selftest` |
| **P3** | Linux XFS bulkstat / btrfs tree search | ROI 低（XFS bulkstat 不给名字），风险高 |
| **P3** | io_uring | **不要做**（已实测无收益） |

### 6. 必须交付的「自检」（`selftest`）

对每个**未文档化或版本相关**的 OS 行为，都要有一个真机自检（`duh selftest` 是范例）：

1. 造一个真硬链接 → 断言 `(dev, fileid)` 相同且只被计一次。
2. **造一个真 APFS clone（`clonefile`）与一个真 copy → 断言 clone 的 `ATTR_CMNEXT_CLONEID` 非零且相同、copy 的不同；并且断言 clone 的 `ATTR_CMNEXT_PRIVATESIZE == 0`、copy 的 `> 0`，同时 `CLONE_REFCNT` 为 2。** 这一条同时验证了有争议的 `0x100` 常量、`FSOPT_ATTR_CMN_EXTENDED` 的必要性、以及「`PRIVATESIZE` 才是可释放量」这个记账口径。
3. 造一个稀疏文件 → 断言按 `st_blocks` 计而不是 `st_size`。
4. 造一个 NTFS 压缩文件 → 断言 Allocated 与 Size 都正确读出。
5. 造一个 firmlink / 跨卷挂载点 → 断言不重复计数、不越界下探。
6. 在 SMB/exFAT/FUSE 上跑一次 `getattrlistbulk` → 断言降级路径被触发而不是崩溃。

---

## 参考与数据点

### 一手文档（OS / 内核 / API）

**Linux**：[`statx(2)`](https://man7.org/linux/man-pages/man2/statx.2.html) · [`getdents(2)`](https://man7.org/linux/man-pages/man2/getdents.2.html) · [`readahead(2)`](https://man7.org/linux/man-pages/man2/readahead.2.html) · [`posix_fadvise(2)`](https://man7.org/linux/man-pages/man2/posix_fadvise.2.html) · [`io_uring_enter(2)`](https://man7.org/linux/man-pages/man2/io_uring_enter.2.html) · [mainline `include/uapi/linux/io_uring.h`](https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/io_uring.h) · [`inotify(7)`](https://man7.org/linux/man-pages/man7/inotify.7.html) · [`fanotify(7)`](https://man7.org/linux/man-pages/man7/fanotify.7.html) · [`fanotify_init(2)`](https://man7.org/linux/man-pages/man2/fanotify_init.2.html) · [`ioctl_xfs_fsbulkstat(2)`](https://manpages.debian.org/testing/xfslibs-dev/ioctl_xfs_fsbulkstat.2.en.html) · [btrfs-filesystem(8)](https://btrfs.readthedocs.io/en/latest/btrfs-filesystem.html) · [OpenZFS zfsprops(7)](https://openzfs.github.io/openzfs-docs/man/master/7/zfsprops.7.html) · [kernel fiemap docs](https://raw.githubusercontent.com/robimarko/linux/refs/heads/master/Documentation/filesystems/fiemap.rst) · [Linux 6.8 statmount/listmount](https://www.phoronix.com/news/Linux-6.8-statmount-listmount)

**Windows**：[`FSCTL_ENUM_USN_DATA`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_enum_usn_data) · [`MFT_ENUM_DATA_V0`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-mft_enum_data_v0) · [`USN_RECORD_V2`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_record_v2) · [`USN_JOURNAL_DATA_V2`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-usn_journal_data_v2) · [`READ_USN_JOURNAL_DATA_V1`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-read_usn_journal_data_v1) · [Change Journals](https://learn.microsoft.com/en-us/windows/win32/fileio/change-journals) · [Creating/Modifying/Deleting a Change Journal](https://learn.microsoft.com/en-us/windows/win32/fileio/creating-modifying-and-deleting-a-change-journal) · [`FSCTL_CREATE_USN_JOURNAL`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_create_usn_journal) · [`FILE_ID_BOTH_DIR_INFORMATION`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_id_both_dir_information) · [`FILE_ID_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info) · [`ReadDirectoryChangesW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-readdirectorychangesw) · [Reparse Points](https://learn.microsoft.com/en-us/windows/win32/fileio/reparse-points) · [Block Cloning](https://learn.microsoft.com/en-us/windows/win32/fileio/block-cloning) · [Obtaining the Size of a Sparse File](https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-the-size-of-a-sparse-file) · [FSCTL_QUERY_ALLOCATED_RANGES (MS-FSCC)](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fscc/d2cde38a-d0b9-4412-b966-52011f8cf6cb) · [Raymond Chen: Just What Is "Size on Disk"?](https://learn.microsoft.com/en-us/previous-versions/technet-magazine/hh148159(v=msdn.10)) · [MSIX + raw volume + 按需提权 helper（Microsoft Q&A）](https://learn.microsoft.com/en-us/answers/questions/5944062/msix-win32-app-reading-raw-c-mft-only-when-run-as)

**macOS**：[`getattrlistbulk(2)` man page（macOS 13.6.5）](https://man.freebsd.org/cgi/man.cgi?query=getattrlistbulk&sektion=2&manpath=macOS+13.6.5) · [`getdirentriesattr(2)` man page（标题即 `NOW DEPRECATED`）](https://man.freebsd.org/cgi/man.cgi?query=getdirentriesattr&sektion=2&manpath=macOS+13.6.5) · [XNU `bsd/sys/attr.h`（本报告所有 `ATTR_*` 常量与 `struct attrlist` 的来源）](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h) · [Apple filesystem-dev 邮件列表：`getattrlistbulk` 取代 `getdirentriesattr`（2014-12）](https://lists.apple.com/archives/filesystem-dev/2014/Dec/msg00004.html)（⚠️ **该归档站对脚本抓取返回 403，我只能通过二手引用（dumac 博客的逐字引文）核实其内容**） · [FSEvents Programming Guide](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/FSEvents_ProgGuide/UsingtheFSEventsFramework/UsingtheFSEventsFramework.html) · [kFSEventStreamCreateFlagFileEvents](https://developer.apple.com/documentation/coreservices/1455376-fseventstreamcreateflags/kfseventstreamcreateflagfileevents?language=objc) · [kFSEventStreamEventFlagMustScanSubDirs](https://developer.apple.com/documentation/coreservices/1455361-fseventstreameventflags/kfseventstreameventflagmustscansubdirs/) · [kFSEventStreamCreateFlagNoDefer](https://developer.apple.com/documentation/coreservices/1455376-fseventstreamcreateflags/kfseventstreamcreateflagnodefer) · [kFSEventStreamEventExtendedFileIDKey](https://developer.apple.com/documentation/coreservices/kfseventstreameventextendedfileidkey?language=objc) · [firmlink fileflag](https://developer.apple.com/documentation/system/fileflags/firmlink)

**GNU / POSIX**：[GNU coreutils `du` manual](https://www.gnu.org/software/coreutils/manual/html_node/du-invocation.html) · [`coreutils/src/du.c`](https://raw.githubusercontent.com/coreutils/coreutils/master/src/du.c) · [macOS/FreeBSD `du(1)`](https://keith.github.io/xcode-man-pages/du.1.html) · [GNU findutils: Symbolic Links](https://www.gnu.org/software/findutils/manual/html_node/Symbolic-Links.html) · [Rust `Path` docs（`to_string_lossy` 有损非单射）](https://doc.rust-lang.org/std/path/struct.Path.html) · [`slice::select_nth_unstable`](https://doc.rust-lang.org/std/primitive.slice.html#method.select_nth_unstable)

### 工具与 benchmark

| 数据点 | 数字 | 类型 |
|---|---|---|
| gdu benchmark（90 GB / 100 k 目录 / 400 k 文件 / 500 GB SSD） | diskus 冷 4.489 s / 热 0.271 s；gdu 冷 4.716 s / 热 0.466 s；dua 冷 6.030 s；dust 冷 6.181 s；**du 冷 30.608 s / 热 1.255 s**；ncdu 冷 33.163 s / 热 2.222 s；`gdu --db=*.db` 冷 44.989 s | **[实测]**（hyperfine，`drop_caches`）[来源](https://github.com/dundee/gdu#benchmarks) |
| diskus README（15 GB / 400 k 文件） | diskus 冷 1.746 s / 热 0.500 s；`du -sh` 冷 17.776 s / 热 1.098 s | **[实测]** [来源](https://github.com/sharkdp/diskus) |
| jwalk bench（Linux 源码树，iMac 2015） | unsorted 1 线程 141.66 ms / 8 线程 54.631 ms；**sorted+metadata 1 线程 313.91 ms / 8 线程 86.985 ms（3.61×）**；walkdir 单线程 134.28 ms（**比单线程 jwalk 还快**） | **[实测]** [来源](https://github.com/Byron/jwalk/blob/main/benches/benchmarks.md) |
| ripgrep 并发扫描（Chromium 394,576 文件，热缓存） | `-uuu --files`：1T 0.255 s → **4T 0.163 s（最优）** → 32T 0.274 s；带 gitignore：1T 0.701 s → **8T 0.214 s（最优）** → 32T 0.249 s | **[实测]** [来源](https://github.com/BurntSushi/ripgrep/discussions/2472) |
| dumac（409,500 文件 / M1 Pro / 热缓存） | **⚠️ 博客与 README 是两次不同的运行，不要混用**。博客（单次运行）：`du -sh` **2.570 s**、Go/CGO **0.850 s**、Rust/tokio **0.52 s** → 博客正文说 6.4×/2.58×。README（`hyperfine --warmup 3 --min-runs 5`）：`du -sh` **3.186 s ± 0.198**、`diskus` **1.834 s ± 0.157**、`dumac` **563.1 ms ± 22.6** → 5.66× du、3.26× diskus。**引用「dumac vs du」时用 README 的 hyperfine 那组，并注明是哪一组。** 系统调用占 **91%** 时间；锁竞争 1.5% | **[实测]** [来源 1](https://healeycodes.com/maybe-the-fastest-disk-usage-program-on-macos) [来源 2](https://github.com/healeycodes/dumac) |
| **Tempelmann 独立 macOS 目录读 benchmark**（只要名字，秒） | APFS SSD：`contentsOfDirectoryAtURL` 10.6 / `getattrlistbulk` 6.8 / **`readdir` 3.2**；10.14 APFS：12 / 10 / **8**；NTFS SSD：6 / 6 / **4.7**；SMB(NAS)：15 / 15 / **5.7**；AFP(NAS)：2.5 / **2.14** / 2.7；HFS+ SSD：2.8 / 2.26 / 2.47。**只要名字时 readdir 更快；需要属性时 getattrlistbulk 显著胜出** | **[实测]** [来源](https://blog.tempel.org/2019/04/dir-read-performance.html) |
| 真机 APFS clone 实测（macOS 27 / Apple Silicon） | 3 组各 11 文件共享同一 `CLONEID`：`sum(ALLOCSIZE)=225,280` vs 真实成本 **20,480**（**11×**），每文件 `PRIVATESIZE=0`；另一台探针发现 **117 成员 clone 家族**：`du` 报 1,437,696 B 而实占 **12,288 B**；完全共享的 Chrome framework：`st_blocks*512 = 508,866,560` 而 `PRIVATESIZE = 0`（`du -k` 报 ~497 MB，删除释放 0）。整树：CoreSimulator **高估 20%**、`/Applications` **7%**、`~/Library/Application Support` 0% | **[实测]** |
| dumac tokio → rayon work-stealing | 910.4 ms → **711.9 ms（1.28×）**；系统调用数减半；**上下文切换 1.2 M → 235 k** | **[实测]** [来源](https://healeycodes.com/optimizing-my-disk-usage-program) |
| 硬链接去重分片函数 | `inode % 128` → 平均 **176.66** 次锁冲突；`(inode >> 8) % 128` → **4.66** 次；约 5% 墙钟收益 | **[实测]** [来源](https://healeycodes.com/optimizing-my-disk-usage-program) |
| `strace -c`：`statx` vs io_uring `IORING_OP_STATX`（42,046 目录 / 439,333 条目 / NVMe） | 单线程：`statx` 439,333 次 × 3.38 µs = 1.485 s（**63.93%**），`getdents64` 17.22%，`openat` 8.44%，`close` 5.73%，`fstat` 4.64%，合计 2.322 s。io_uring 批量：合计 **2.320 s（持平）**，但 `io_uring_enter` **38.2 µs/次** | **[实测]**（`strace` 有放大效应）[来源](https://users.rust-lang.org/t/batching-statx-syscall-using-io-uring/110745) |
| io_uring vs rayon 线程池（328,155 文件哈希遍历） | 1 线程：rayon 38,295 ms vs io_uring 39,651 ms；2 线程：19,548 ms vs 21,022 ms | **[实测]** [来源](https://users.rust-lang.org/t/help-understanding-why-io-uring-io-performs-worse-than-stdlib-in-a-thread-pool/97853) |
| `FSCTL_ENUM_USN_DATA`（700 k 文件，64 KB 缓冲，冷） | **21 s ⇒ 33,300 文件/s；84 MB ⇒ 4 MB/s**；`FILE_FLAG_SEQUENTIAL_SCAN`/`RANDOM_ACCESS`/`NO_BUFFERING` **全部无效**；对比：直读 MFT 的工具 **<5 s ⇒ >140,000 文件/s** | **[实测]** [来源](https://stackoverflow.com/questions/45179671/) |
| `mft` crate 解析（13 MB 样例 MFT，Mac15,6） | 端到端 JSONL **95.94 ms → 57.81 ms** ⇒ **~135 → ~225 MB/s 单线程** | **[实测]** [来源](https://github.com/omerbenamram/mft/blob/master/PERF.md) |
| Everything 索引与内存 | 250 k 文件 ≈ 5 s / 35 MB RAM / 14 MB 磁盘；**1 M 文件 ≈ 1 分钟 / 100 MB RAM / 45 MB 磁盘** | **[厂商声称]** [来源](https://www.voidtools.com/faq/) |
| WizTree vs WinDirStat | 25 GB HDD：4.34 s vs 3 min 20 s（46×）；460 GB SSD：5.23 s vs 1 min 55 s（22×）；**未披露文件数与硬件细节** | **[厂商声称]** [来源](https://diskanalyzer.com/wiztree-vs-windirstat) |
| ncdu 2 内存 | `-x /` 3.8 M 文件：ncdu 1.16 **429 MB** → ncdu 2.0 **162 MB（≈42.6 B/文件）**；38.9 M 文件：3969 MB → 1686 MB；**每节点**：普通文件 25 B / 目录 56 B（不含名字与哈希表） | **[实测]** [来源](https://dev.yorhel.nl/doc/ncdu2) |
| ncdu 2.6 二进制导出 | **1.4 billion 文件 → ~21 GiB ≈ 16 B/文件（磁盘）** | **[实测]** [来源](https://dev.yorhel.nl/ncdu/binfmt) |
| plocate 1.1.x | 27 M 文件 → **466 MB ≈ 17.3 B/文件（磁盘）**；mlocate 同规模 1.1 GB（~40.7 B/文件） | **[实测]** [来源](https://plocate.sesse.net/) |
| `HashMap<u64,u64>` 1 M 项 | insert 1644 ms / query 101 ms / **RSS 48 MiB ≈ 50 B/项**（payload 只有 16 B） | **[实测]**（第三方 harness，RSS 口径）[来源](https://github.com/innovabinaria/map_bench) |
| gdu 并发规模 | 默认 8 核冷 4.716 s；`GOMAXPROCS=80` 冷 4.901 s（更慢，σ=1.95 s）；热缓存 466.1 ms vs 459.1 ms（噪声内） | **[实测]** |

### 我对来源冲突 / 证据薄弱的明确标注

1. **Everything 的「1 M 文件约 1 分钟」 vs WizTree 的「秒级」** —— 两者不是同一件事（Everything 建持久化索引并支持排序，WizTree 只做一次性聚合）。**不要混用**。
2. **`ATTR_CMNEXT_CLONEID` 的常量值**：duh README 说「Apple 文档暗示 0x40，实测是 0x100」。我核对了 [XNU main 的 `attr.h`](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/attr.h)：`0x40` 是 `ATTR_CMNEXT_REALDEVID`，`0x100` 才是 `CLONEID`，与 duh 的实测一致。**用 `0x100` + `selftest`。**
3. **`getattrlistbulk` 在非原生 FS 上的失败模式**：man page 的 ERRORS 里**没有** `ENOTSUP`，但 Apple 邮件列表称 VFS 层支持所有文件系统。**我找不到权威的「哪些卷会失败」列表**，所以必须无条件实现回退 + 运行时探测。
4. **「目录硬链接」的 `du` 行为因工具/调用方式而异**：BSD `du` 去重 Time Machine 的目录硬链接，GNU `du` 单参数时不去重目录，ncdu 明确不支持。**不要泛化。**
5. **`STATX_BATCH` 在 mainline 不存在**（最接近的是 `IORING_OP_STATX` 5.6+ 与 `statmount`/`listmount` 6.8）。**不要把设计建立在这个名字上。**
6. **「交替 vs 两阶段」没有 head-to-head benchmark**；「优先级队列遍历」没有公开先例；「`fadvise`/`readahead` 对目录 fd 的效果」没有测量。这三处本文标为推断。
7. **`GetCompressedFileSize` 与 ReFS block cloning / Windows Server dedup 的交互**：没有任何 Microsoft 文档明确说明，这是从 API 定义推出的**推断**，上线前必须实测。
8. **`find` / `fd` / `mdfind` 都没有可信的公开吞吐数字**，本文不给数字。
9. **io_uring 的「无收益」结论有实测支持**（两组独立测量），但两组都是社区测量、非论文级；`strace` 路径有放大效应。**结论方向（不值得为 v1 引入）我认为是稳的。**
10. **Everything 的论坛上关于内存的讨论（voidtools forum）在我调研时返回 HTTP 503 维护页**，所以内存数字只有 FAQ 一个来源。
11. **[真机实测] 已确认 Apple 的文档有错的地方：** ① `man 2 getattrlist` 声称前置 length 字段是「实际拷贝进缓冲的字节数」，实测在 APFS/macOS 27 上它填的是**完整逻辑大小**（缓冲只有 4 字节时仍写 88），且 `FSOPT_REPORT_FULLSIZE` 无影响 ⇒ **Rust 实现若相信这个字段会越界读**；② `getdirentriesattr` 在 26 个挂载点上全部 `ENOTSUP`，与它仍在 SDK 里存在形成反差；③ `VOL_CAP_FMT_CLONE_MAPPING` 未置位但 clone 属性正常工作；④ `ATTR_VOL_ATTRIBUTES.nativeattr` 低报 APFS 实际支持的属性。**凡是依赖这四点的设计都必须以运行时探测为准。**
12. **Apple 的 `lists.apple.com` 归档在调研时返回 HTTP 502，Wayback 也没有可用快照**，所以 2014-12 那封邮件只能通过同线程的 [mail-archive 镜像](https://www.mail-archive.com/filesystem-dev@lists.apple.com/msg00260.html) 核实（注意 mail-archive 的编号与 Apple 的不同，`.../msg00004.html` 是另一封无关邮件）。
13. **网络卷（NFS/SMB/FUSE）上 `getattrlistbulk` 的行为仍未验证** —— 两台测试机都没有挂载这类卷。可用的间接证据：FSKit 的 `devicefs` 卷上 bulk 可用而 `getdirentriesattr` 为 `ENOTSUP`；Tempelmann 在 AFP/SMB 上测得 `getattrlistbulk` 可用且在 AFP 上最快。**回退路径仍然必须实现。**

### 与本项目现有文档的对应

| 本报告结论 | 影响的 `REQUIREMENTS.md` 条目 |
|---|---|
| 分层策略 + 每平台批量枚举 | R2.2 增量优先级扫描引擎 |
| 有界优先级队列 + 保留部分结果的取消 + 批量事件流 | R2.2 / R2.4 |
| 三数模型 + `df` 对账 + `partial`/`stale` 徽章 | R2.3 扫描事件协议、R5.x |
| 仅目录 + 精确残差 + top-K 的内存模型 | R2.4 / R7.2 内存优化 |
| 「能 stat 而非能读」的可删除性判定 | R4.2 权限判定 |
| Windows 独立提权 helper + 命名管道 IPC | R7.4 通知与授权 |
| FSEvents / USN journal / inotify 三档增量 | R4.3 删除状态监听（`notify` 已引入） |
| 需要 `selftest`（硬链接/clone/稀疏/压缩/firmlink） | 新增建议 |
