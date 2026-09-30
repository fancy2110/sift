# Sift 作为 DeepSeek Harness 插件

> 状态：**设计提案，尚未实现**。
> DSH 侧机制全部来自本机已安装的 `@deepseek-ai/dsh-*` 包与官方开发文档（见文末出处）；Sift 侧现状来自本仓库。
> 结论先行：值得做，但**不是"给 CLI 套个壳"**，而是把 Sift 拆成「一份引擎 + 两个适配面」。做对了它同时是分发杠杆和安全样板；做错了就是给 agent 递一把没有保险的刀。

---

## 0. 一句话结论

把 Sift 从**一个窗口**变成 agent 的**一条能力面**：

- **引擎留 Rust（只有一份）**：`sift-core / platform / scan / analyze / store / monitor` 不动，新增一个 headless 服务层 + 一个可执行入口。
- **DSH 侧只加薄薄一层**：工具（`disk_*`）+ 按需加载的技能 + 常驻值守 + 审批门禁。
- **桌面 app 从"唯一入口"退位为"可视化控制台"**：探索、treemap、授权引导、托盘值守仍归它；决策与批量操作交给对话。

一句话的形态：

```bash
dsh plugin --profile web add @sift/dsh-plugin
# 然后： "我这块盘为什么满了？能清出多少？先给我看清单，我点批准再删。"
```

---

## 1. DSH 插件机制：可用的事实清单

DSH 的扩展模型是 **bundle（组合包）+ profile（配置档）**：bundle 是一个 npm 包，带一份 cordis patch 层；profile 是 `$DSH_HOME/profiles/<name>` 下的目录，声明 `dsh.profile.bundles` 的顺序列表。最终配置在空根之上按层组合，后面的层按行胜出。

| 机制 | 事实 | 对 Sift 的意义 |
|---|---|---|
| 安装 | `dsh plugin --profile web add <pkg>` 在 profile 目录里转发给 pnpm；用 `dsh --profile web --dump-config` 校验层 | 分发成本 ≈ 一次 npm install |
| 包契约 | `package.json` 里 `dsh.bundle.patch: "./cordis.patch.yml"`；ESM；dsh 核心包放 `peerDependencies` + `devDependencies` | 与宿主共享 cordis / tools 实例，不重复安装 |
| 插件签名 | 命名导出 `name` / `inject` / `apply(ctx, config)`；`Config` 用 Schemastery（实现 Standard Schema 才被接受）；**`export default` 会遮蔽同名命名导出**，Loader 丢掉 `inject`/`Config` 后在加载期爆炸 | 薄适配层，几十行一个工具 |
| 自动清理 | `ctx.on()` / `ctx.tools.register()` / `ctx.effect()` 都是 effect，卸载时自动撤销 | 引擎子进程、监听器随插件生命周期收尾，无需手写 teardown |
| 工具注册 | `ctx.tools.register(defineTool({ name, description, parameters, output: { schema, render }, execute }))`；`execute` **只返回规范 JSON 值**，`render` 才产生模型可见内容 | 参数 schema 自动进系统提示词；`output.schema` 是给程序化调用（PTC）的规范值 |
| 长任务 | `ctx.jobs.start({ kind, label, owner: exec.agent, run })` + producer 的 `run_in_background`；**job 是进程内的，不跨重启** | 全量扫描走这条；但**任务持久性只能由引擎侧负责**，harness job 只是观察句柄 |
| 无进度流 | 运行中的工具**没有**向 UI 增量推送的 API，长任务只能"提交 → 轮询" | 扫描进度由 `disk_scan_status` 拉取，不要设计成推流 |
| 事件 | `ctx.on('session/event')` 观察持久化事件；`agent/pre-step` 是 waterfall；`sessionProjections` 提供可回放的 UI 状态 | 值守告警的入口 |
| 主动开口 | 运行中的 agent 用 `agent.inject(msg)`（贴到下一步，**不**抢当前回合）；空闲 agent 用 `agent.followup(msg)`（**会**唤醒一个新回合）；告警用 `createUserMessage({ source: { kind: 'plugin', plugin: 'sift', form: 'notice' } })` | "打断"和"路过时说一句"是两种产品决定，不能混 |
| 策略钩子 | `tools/pre-execute` waterfall 返回 `{ kind: 'allow' \| 'deny' \| 'ask' }`；`ask` 走 `ctx.approval`，**没有应答者时 fail closed**；`ctx.tools.guard()` 是单调拒绝，后续监听器无法撤销 | 危险操作的门禁不该硬编码在工具里 |
| MCP 桥 | `dsh-mcp-client` 每个 server 一个插件实例，工具注册为 `mcp__<server>__<tool>`；**只桥接 tools，不支持 resources/prompts**；Electron 宿主用 `process.execPath` + `ELECTRON_RUN_AS_NODE` 起代理 | 「最大兼容」路线：同一引擎同时喂 DSH 与其它 harness |
| 技能 | `dsh-skill-filesystem` 可挂 `bundledSkillDir`，按需加载；bundled 目录优先级**最低**，用户可用同名技能覆盖 | 过程知识进技能，不占系统提示词；还能被用户替换 |
| 插件清单 | `dsh-host-plugin-inventory` 暴露只读清单（条目、fiber 状态、按预设组合） | 分发侧零额外工作，插件自动出现在设置里 |
| 客户端卡片 | 内建 Web Client **不消费** `presentCall/presentResult`；自定义卡片必须在 client 插件里注册 keyed slot `tool.call.toolview`，且 `lib/client.js` 启动前就得是 lazy-CJS 工厂格式 | 第一版用纯文本 + `presentationMeta`，端上卡片推迟（构建成本见 §3.7） |
| 已有第三方先例 | `@openviking/dsh-memory-plugin` 就是一个真实第三方 bundle（`dsh.bundle.patch` + 技能 + MCP + 客户端） | 路线已被走通，不是纸上推演 |

出处：[打包与安装插件](https://deepseek-harness.github.io/deepseek-harness/develop/basic/publish.md)、[插件与生命周期](https://deepseek-harness.github.io/deepseek-harness/develop/framework/index.md)、[事件系统](https://deepseek-harness.github.io/deepseek-harness/develop/framework/events.md)、[开发一个工具](https://deepseek-harness.github.io/deepseek-harness/develop/basic/tool.md)、[工具编写参考](https://deepseek-harness.github.io/deepseek-harness/reference/cookbook/adding-a-tool.md)。

---

## 2. 现状盘点：Sift 有什么、缺什么

### 2.1 已有资产（对插件化是加分项）

| 资产 | 插件场景下的价值 |
|---|---|
| 扫描引擎（61 万文件 2.28s，整卷 1388 万文件 202s） | agent 工作负载里"偶发全量 + 高频小查询"都扛得住，前提是别把它放进 prompt 循环 |
| 72 字节定长目录节点 / 文件不建节点 / 326 MB 打满 4M 节点 | 常驻索引在 agent 常开的环境里是可接受的成本 |
| 焦点确定性优先 + 事件流 | 天然映射"边扫边答"，工具可以立刻回答"当前目录"而不是等全量 |
| `sift-analyze` 三段式（规则提名 → 判定 → 路由） | 把"什么能删"变成**可审计的结论 + 理由**，而不是模型的自由发挥 |
| 安全边界 7 条（只进回收站、模型只能削弱、指纹复核、`home_only`、批次上限、密钥不落盘、远程 opt-in） | 这正是 agent 生态最稀缺的东西：**可回滚的写操作语义** |
| `sift-store` 的 `cleanable.json` / 决策日志 / 例行任务 | 跨会话的"长期许可"，DSH 的会话级审批替代不了这一层 |
| `sift-monitor` 纯函数策略 + 全分支测试 | 可以被 agent 复用，不需要在 JS 里重写一套"能不能自动删" |

### 2.2 缺口（不做这些，插件就是玩具）

| 缺口 | 说明 |
|---|---|
| **没有 headless 入口** | 只有 Tauri app 和 `scan_bench` 示例；agent 侧无法拉起引擎 |
| **没有协议** | 33 个 Tauri 命令 + 4 个前端事件是进程内契约，不是可版本化、可跨进程的接口 |
| **状态绑在 app 进程** | 索引、扫描管理器、watcher 都活在 Tauri 进程里，插件拿不到热数据 |
| **API 形状是"命令 + 事件流"，不是"查询"** | agent 需要 `top(n)`、`children(node)`、`find(predicate)`、`freshness` 这类分页查询，而不是自己消费百万级事件 |
| **没有"索引会过期"的表达** | 模型必须能知道"这是 3 分钟前的快照，覆盖率 94%，1627 个目录被拒绝访问" |
| **Windows / Linux 仍是 Tier 2** | 插件会把这些平台的短板直接暴露给 agent 用户 |

---

## 3. 目标架构

```
┌─ 适配面 A：Sift.app (Tauri + Svelte) ── treemap / 授权引导 / 托盘 ─┐
│                                                                    │
│        ┌──────────── crates/sift-api（headless 服务层）─────────┐  │
├────────┤  ScanManager · Analyzer · Store · Monitor · Watcher     ├──┤
│        │  请求/响应类型 = Tauri 命令与 engine 协议的唯一真源     │  │
│        └───────────────────────┬────────────────────────────────┘  │
│                                │                                    │
└─ 适配面 B：sift-engine (Rust bin) ── NDJSON / JSON-RPC over stdio ──┘
                                 │  或 unix socket（复用 Sift.app 的热索引）
                                 ▼
                    ┌─ @sift/dsh-plugin（cordis bundle）─┐
                    │  tools: disk_*  ·  skills/  ·  值守  │
                    │  审批门禁 · 卡片投影                  │
                    └──────────────┬──────────────────────┘
                                   ▼
                    ┌─ DeepSeek Harness (web / desktop) ──┐
                    │  ctx.tools · ctx.jobs · approval     │
                    └─────────────────────────────────────┘
```

**分层原则（沿用本仓库现有约束）**：产品逻辑只向下，适配层不含逻辑。现在的唯一适配层是 `src-tauri`；插件化只是**增加第二个适配层**，而不是把逻辑搬到 TypeScript。

### 3.1 引擎面：`crates/sift-api` + `crates/sift-engine`

- `sift-api`：把 `src-tauri/src/{scanner,analyze,disks,watcher}.rs` 里现在被命令闭包包裹的状态与编排提取成普通 Rust 结构（`SiftService`），返回**可序列化的请求/响应类型**。Tauri 命令退化为「反序列化 → 调 service → 序列化」。
- `sift-engine`：一个 bin，`sift-engine serve --stdio`（默认）或 `--socket <path>`。协议是 **NDJSON 上的 JSON-RPC 2.0**：请求有 `id`，服务端主动推 `method` 通知（扫描进度），`$/cancel` 取消。

方法集（第一版就够用了）：

| 方法 | 语义 | 风险 |
|---|---|---|
| `engine.hello` | `{ protocol: 1, capabilities: [...], version }` 能力协商 | 只读 |
| `volumes.list` | 卷、容量、可用、可移除 | 只读 |
| `index.status` | `{ root, scannedAt, coverage, denied, nodes, freshness }` | 只读 |
| `index.refresh` | 增量刷新（走已有 FSEvents / inotify / ReadDirectoryChangesW） | 只读 |
| `index.scan` | 全量扫描，进度以通知推送，可取消 | 只读（重） |
| `tree.children` | `{ node, sort, limit, cursor }` → 子节点 + `truncated` + `total` | 只读 |
| `tree.top` | `{ root, kind: dir\|file, k }` → 最大目录/文件 | 只读 |
| `search.find` | `{ root, predicate: {sizeGt, ageDaysGt, ext, nameGlob}, limit }` | 只读 |
| `analyze.run` | 规则提名 + 判定 → `Candidate[] + Verdict` | 只读 |
| `cleanable.list` / `cleanable.approve` / `cleanable.forget` | 长期许可清单（复用 `cleanable.json`） | 状态变更 |
| `trash.execute` | `{ items: [{path, fingerprint}], dryRun }` → 逐条结果 | **危险** |
| `monitor.status` / `monitor.setMode` | 后台值守状态 | 状态变更 |

三条硬约定：

1. **每个响应都带 `freshness` + `coverage` + `truncated` + `total`**，让模型永远不可能把"部分结果"当成"全部"。这是 Sift `Progress.denied` / `unrecorded_dirs` 语义的延续。
2. **删除只有一个实现**：`trash.execute` 走系统回收站，且**必须带指纹**；没有 `unlink` 路径，没有 `--force`。
3. **`trash.execute` 不接受"模型自报的批准"**：它只接受由 `cleanable.approve`（长期许可）或 DSH 审批层签发的、与指纹绑定的批准凭据。

**单实例与热索引**：`sift-engine --socket` 常驻；插件启动时先探测 `~/Library/Application Support/Sift/engine.sock`（各平台对应路径），有就附着，没有就自己拉起 headless 实例。这样 Sift.app 在跑时是"索引保温器"，插件不再重复 202s 全量扫。

### 3.2 插件面：`@sift/dsh-plugin`

```
@sift/dsh-plugin/
├── package.json          # dsh.bundle.patch + optionalDependencies（各平台引擎二进制）
├── cordis.patch.yml      # 插入 id: sift 的插件行
├── index.mjs             # apply(): 注册工具、技能、值守
├── engine.mjs            # NDJSON JSON-RPC 客户端（spawn / 附着 socket）
├── approval.mjs          # 批准凭据签发与校验
├── skills/
│   └── disk-triage/SKILL.md
└── presentation.mjs      # output.presentationMeta：容量条 / 候选表 / 结果表
```

```json
{
  "name": "@sift/dsh-plugin",
  "version": "0.1.0",
  "type": "module",
  "main": "index.mjs",
  "files": ["index.mjs", "engine.mjs", "approval.mjs", "presentation.mjs", "skills/", "cordis.patch.yml"],
  "dsh": { "bundle": { "patch": "./cordis.patch.yml" } },
  "optionalDependencies": {
    "@sift/engine-darwin-arm64": "0.1.0",
    "@sift/engine-darwin-x64": "0.1.0",
    "@sift/engine-win32-x64": "0.1.0",
    "@sift/engine-linux-x64": "0.1.0"
  },
  "peerDependencies": {
    "@deepseek-ai/cordis": "4.0.2",
    "@deepseek-ai/dsh-tools": "^0.1.5-rc.3",
    "@deepseek-ai/dsh-skill-filesystem": "^0.1.5-rc.3",
    "@deepseek-ai/dsh-user-approval": "^0.1.5-rc.3"
  }
}
```

```yaml
# cordis.patch.yml —— 用户的 profile 层可以整行覆盖它，所以默认值要保守
- insert:
    - id: sift
      name: '@sift/dsh-plugin'
      config:
        homeOnly: true                 # 主目录之外不自动删（默认开）
        maxBatchBytes: 2147483648      # 超过就转"需要确认"，绝不静默截断
        remoteAi: false                # 远程判定 opt-in
        monitor: { enabled: false, thresholdPct: 10, cooldownMinutes: 720 }
```

工具表（模型看到的）：

| 工具 | 引擎方法 | 形态 | 备注 |
|---|---|---|---|
| `disk_volumes` | `volumes.list` | 只读 | 入口：先看是哪块卷 |
| `disk_scan` | `index.scan` / `index.refresh` | **后台任务** | `ctx.jobs.start({ owner: exec.agent })`，返回 jobId + freshness |
| `disk_scan_status` / `disk_cancel` | `$/cancel` | 只读 | 长任务三件套 |
| `disk_children` | `tree.children` | 只读 | 逐层钻取，默认 20 条 + `total` |
| `disk_top` | `tree.top` | 只读 | "哪里最占地方" |
| `disk_find` | `search.find` | 只读 | 谓词查询：大于 1G、90 天没动、.dmg/.zip |
| `disk_analyze` | `analyze.run` | 只读 | 返回候选 + 安全等级 + 理由 |
| `disk_cleanable` | `cleanable.list` | 只读 | 已批准的长期清单 |
| `disk_trash` | `cleanable.approve` + `trash.execute` | **危险** | 默认 `dryRun: true`；真正执行需要批准凭据 |

设计要点：

- **`output.schema` 是程序化 API**（PTC 模式下模型可以直接 `await tools.disk_top(...)`），`output.render` 才负责人话。不要返回内容块，不要把 id 塞进自然语言让调用方解析。
- **`presentationMeta` 放可回放的结构化事实**（容量条数值、候选表格行、逐条删除结果），卡片在 client 插件里渲染。**treemap 的几何数据永远不进模型上下文**。
- **上下文预算**：默认"汇总 + top-N + truncated"，钻取靠下一次调用。整卷 400 万目录的任何一次全量 dump 都会直接烧掉会话。

### 3.3 技能：把"怎么分诊"放在按需加载的地方

`skills/disk-triage/SKILL.md` 写过程知识，而不是写进系统提示词：

1. 先 `disk_volumes` 确认是哪块卷、是真满还是被 firmlink 重复计数；
2. `disk_scan`（后台）→ 看 `freshness` / `denied`，**先声明不确定性**；
3. `disk_top` / `disk_find` 定位，再 `disk_children` 逐层收窄；
4. `disk_analyze` 拿候选与理由，按 `Safe / Review / Keep` 分三堆呈现；
5. **只对 `Safe` 且用户逐条批准的条目**调 `disk_trash`；
6. 报告"释放前 / 释放后 / 失败条目 / 仍不确定的部分"。

系统提示词里只留一句策略：*删除只进回收站；`Review` 不是许可；没有批准凭据不许调用 `disk_trash`。*

技能用第二个 provider 挂进引擎（不改用户已有技能根，且 bundled 目录优先级最低——用户放一个同名技能就能覆盖我们）：

```js
import * as skillFilesystem from '@deepseek-ai/dsh-skill-filesystem'
// 在 apply(ctx) 里：
ctx.plugin(skillFilesystem, {
  providerName: 'sift',
  includeDefaultRoots: false,
  bundledSkillDir: SKILLS_DIR,   // 包内的 skills/
})
```

### 3.4 两种"批准"必须分清

| 语义 | 载体 | 生命周期 | 谁能签发 |
|---|---|---|---|
| 会话内一次性批准 | `tools/pre-execute` 返回 `{ kind: 'ask' }` → `ctx.approval` → `'allowed-once'` | 本次调用 | 人（无应答者时 fail closed，即拒绝） |
| 跨会话长期许可 | `cleanable.json` 里的逐条 approval + 指纹 | 直到指纹失效 | 人，在对话或 app 里都可以 |

**模型永远不能签发任何一种**，它只能"提议"。这是 Sift 现有 `sanitize_model_verdict` 语义在插件层的镜像：远程/模型判断只能让结论更保守。落地时把 `disk_trash` 拆成两步：

```text
disk_trash(proposalId, dryRun?: true)      # 默认 dry-run，只回逐条结果与受影响清单
   └─ 真正执行需要批准凭据：{ proposalId, fingerprints[], approver: 'human' }
        └─ 凭据由插件在 ctx.approval 返回 allowed-once 之后签发，模型无法伪造
```

### 3.5 值守与主动性

- 插件激活时启动引擎的 `monitor`（或附着已有的），用 `ctx.interval()` 低频采样（disposer 随 fiber 回收）。
- 命中阈值且不在冷却期时，按 agent 状态分流：**运行中** → `agent.inject(notice)` 贴到下一步、不抢当前回合；**空闲** → `agent.followup(notice)` 唤醒一个新回合（这是一个需要用户同意的产品决定，默认可以只 inject 不 followup）。
- 消息用 `createUserMessage({ source: { kind: 'plugin', plugin: 'sift', form: 'notice' } })` 构造：它是持久化的会话事件，可回放、对 compaction 可见。
- 长期形态接 DSH 的 schedule/goal：把"每周五清一次 DerivedData"变成例行任务，**双保险**——Sift 的纯函数策略决定"能不能自动删"，DSH 的调度决定"什么时候问"。
- 已知坑（来自 OpenViking 插件的实战记录）：**不要往系统提示词里塞动态上下文**——声明 `complete: true` 的 persona 预设会在装配后恢复成唯一段落，静默丢掉其它贡献。注入走 pre-step / session 事件。

### 3.6 MCP 路线：以最大兼容为目标的那条腿

同一份 `sift-engine` 再包一层 `sift-mcp`（stdio），任何 MCP 客户端一行配置即可用：

```yaml
- id: mcp-sift
  name: '@deepseek-ai/dsh-mcp-client'
  config: { serverName: sift, transport: stdio, command: sift-mcp }
```

两条路线的取舍：

| | MCP 路线 | DSH bundle 路线 |
|---|---|---|
| 成本 | 低（一个 server） | 中（工具层 + 技能 + 审批 + 值守） |
| 覆盖面 | 所有 MCP 客户端 | 只 DSH |
| 审批/长期许可 | 靠 server 自己 | 接 DSH 审批 UI + `cleanable.json` |
| 技能/主动性/卡片 | 无（MCP 只桥接 tools） | 有 |

**建议两条都发**：MCP 是"到处能用"，bundle 是"用起来最好"。

### 3.7 表现面

- **阶段一**：纯文本 + `presentationMeta`（容量条、候选表、逐条结果表）。零前端成本；`present` 工具还能把"磁盘体检报告"落成可打开的交付文件。
- **阶段二**：`@sift/dsh-client-ui` client 插件，manifest 用 `dsh.client`（`platform: 'web'` + `exports["./client"]`），在 `ctx.slots.inject('tool.call.toolview', …)` 里按 **wire 工具名**注册卡片；侧栏面板是"注册 tab + keyed slot `sidebar.right.pane.tab`"两处注册。
  - **先算清成本**：`lib/client.js` 必须是 lazy-CJS 工厂格式、且启动前就存在，生产环境 HMR 不生效；第三方 client bundle 的构建预设（`packages/client/tsdown.client.ts`）**没有发布成包**，出仓插件得自己复刻这套构建。这是把卡片排到最后的唯一原因。
- **桌面 app 不可替代的部分保留**：treemap 全屏探索、TCC 授权引导、托盘值守。
- **闭环**：app 里加一个"交给 agent 处理"的动作（复制一段带上下文的 prompt），把探索结果带进对话。

---

## 4. 安全边界迁移：7 条现有约束在插件里怎么落地

| Sift 现有边界 | 插件层实现 |
|---|---|
| 删除永远进回收站 | `trash.execute` 是唯一路径；协议里没有永久删除方法 |
| 自动清理只动 `Safe` 且已逐条批准的条目 | 引擎侧策略不变；插件侧不新增"模型认为安全"这条路径 |
| 删除前重新校验指纹 | 批准凭据绑定 `PathFingerprint`；执行时重算，不匹配则跳过并回报 |
| `home_only` 默认开启 | 插件配置项，默认 `true`；无法证明在主目录内 → 拒绝 |
| 超大批次强制确认 | 超 `maxBatchBytes` 返回 `needs_confirmation`，不截断、不拆分 |
| API Key 不落盘 | 插件配置只收环境变量名；`remoteAi: false` 默认 |
| 远程判定 opt-in | 同上，且结构性家族（缓存/构建产物/回收站）永不出本机 |

**新增的、Sift 以前不需要考虑的风险：**

1. **权限继承**：引擎作为 DSH 的子进程运行，TCC「完全磁盘访问」等授权取决于**宿主进程**而非 Sift.app。`dsh-bash-sandbox` 约束的是 bash 执行器，MCP/子进程路径是否同样受限**尚未实测**，上线前必须做一次明确实验（见 §7）。
2. **信任面扩大到 agent**：以前是"用户亲手点删除"，现在是"agent 提议 + 用户点批准"。默认 `dryRun: true`、全量审计（DSH 的 event-sourced session log 天然可回放）、失败逐条回报，是三道闸。
3. **并发**：一个引擎、多个会话。全量扫描必须串行化（相同 root 复用进行中的任务），否则 N 个 agent 会同时把磁盘打满。
4. **版本漂移**：engine 协议版本与插件包版本必须协商（`engine.hello`）；`peerDependencies` 对 `0.1.5-rc.x` 这种预发布序列要显式写范围，semver 不会自动包含。

---

## 5. 分阶段落地

| 阶段 | 交付 | 完成定义（验收） |
|---|---|---|
| **P0 引擎面** | `crates/sift-api` + `crates/sift-engine`；`src-tauri` 改为消费 `sift-api` | `sift-engine volumes.list` / `index.scan` 与 app 行为逐字段一致；现有 `cargo test` 全绿；无新增产品逻辑进适配层 |
| **P1 MCP 桥** | `sift-mcp`，暴露 6 个只读工具 + `trash.execute`（默认 dry-run） | 在 DSH 里能让 agent 扫 `/Applications`、列出 top-20、跑一次 `dryRun` 删除并解释结果 |
| **P2 DSH bundle** | `@sift/dsh-plugin`：工具、技能、审批凭据、`presentationMeta`；平台二进制包 + 签名/公证 | `dsh plugin --profile web add` 后 `--dump-config` 有 `sift` 层；一次"满盘分诊"全流程跑通，全程无永久删除 |
| **P3 值守** | monitor 桥 + 阈值注入 + 冷却 + 与 schedule 整合 | 人为把卷塞到阈值以下，能在不打断用户的前提下产出一次带理由的提醒；冷却生效 |
| **P4 视觉** | `@sift/dsh-client-ui` 卡片；app 侧"交给 agent"入口 | 卡片可回放（纯函数、不读会话状态）；刷新页面后历史卡片仍正确 |

每个阶段独立可发布：**P1 单独发就有价值**，P2 才是完整体验。

---

## 6. 价值

### 6.1 用户价值：把"打开一个 app"换成"问一句话"

- **归因**："盘为什么满了" → 一次对话给出卷 / 目录 / 文件三级归因，附证据（规则命中、mtime、大小、以及**哪些目录没权限看**）。
- **项目感知**：`node_modules`、`target`、`DerivedData`、`.gradle`、Docker、模型权重——agent 知道这个目录属于哪个项目、多久没动过，这是纯 GUI 给不了的上下文。
- **顺手**：agent 在跑构建/测试/下载模型之前先查空间，把"磁盘满"从事故变成前置条件检查。
- **可信**：每条建议有理由、有等级、可逐条批准、全部进回收站、全程留痕。市面上"AI 帮你清盘"最缺的就是这个。
- **主动性**：低于阈值主动开口，而不是等用户想起来打开 app。

### 6.2 产品与分发价值

- **分发杠杆**：桌面 app 的获客要跨"下载 → 安装 → 授权 → 打开"四道坎，而插件是 `add` 一行；而且用户**已经在** agent 会话里，使用频次高一个数量级。
- **形态跃迁**：从"一个要人主动打开的工具"变成"agent 随时可调的能力"。
- **跨 harness**：MCP 形态可插进所有支持 MCP 的客户端，一次引擎投入覆盖多个生态（社区已有把 DSH 桥到其它 harness 的先例）。
- **反哺桌面版**：插件带来的"这玩意儿哪来的"会导流回 app；探索和可视化仍然只有 app 做得好。

### 6.3 工程价值

- **一份引擎，两个前端**：不重写扫描器、不重写安全策略。Sift 现在这套测试（策略函数全分支穷举、平台快路径逐字段对比）**直接复用**。反过来说，任何"在 JS 里再实现一遍判定"的路线都会重演本仓库已经踩过的语义漂移（见 `adjudicate` 里那段 node_modules 被错误提权的历史）。
- **可审查的接口**：引擎方法表就是一份能力清单，配 `freshness/truncated/denied` 语义，模型永远不会被"部分结果"骗。
- **架构不变形**：插件化=加第二个适配层，依赖方向仍然单向向下。

### 6.4 数据与生态价值

- **决策样本变多**：习惯挖掘要求"跨 ≥3 个不同日期重复出现"，更多会话 = 更快积累到可信阈值。
- **规则包可分享**：规则是确定、可审计的，天然适合做成可分享的技能/规则包，形成社区资产。
- **隐私反而是卖点**：结构性家族永不出本机、只发脱敏路径、密钥不落盘——在一个"什么都要上云"的市场里这是差异化。

### 6.5 商业价值

- 免费插件做获客与留存，桌面 app 做付费（可视化、批量、托盘、授权引导）。
- 专业/团队版：远程判定服务、策略下发、审计导出、构建机与开发机的磁盘治理。
- 企业场景最硬：CI 构建机磁盘满了会直接停线，而 agent 已经在这些机器上跑了。

### 6.6 对 DSH 生态的价值

DeepSeek Harness 有文件读写、有沙箱、有审批，但**没有"空间"这个概念**。磁盘是长跑 agent 环境最常见的物理故障源。一个把"高风险写操作 + 审批 + 可回滚"做成样板的插件，对生态的价值不止于清垃圾。

### 6.7 反价值与风险（诚实版）

- **如果只是把 CLI 包一层，价值≈0**。必须做索引常驻 + 查询式 API + freshness 语义，否则每次对话都 202s 全量扫，用户体验是负的。
- **误删是品牌风险**，不是 bug。宁可多一次确认，也不要"聪明地"自动删。
- **权限摩擦可能吃掉全部体验**：TCC 引导在对话里比在 app 里更难做；权限不足时"看起来扫了其实没扫全"是最坏的结果——所以 `denied` 必须默认可见。
- **上下文成本**：每个工具定义都进每一次请求；工具要少而正交（v1 定 6 个，别定 20 个）。
- **双适配面维护成本**：engine 协议一旦发布就要保持兼容。
- **不是所有用户都用 agent**：桌面 app 仍是主入口，插件是增量而不是替代。

---

## 7. 待验证清单（动手前先做实验）

1. **实验（半天）**：MCP stdio 子进程能否读 workspace 之外的路径？是否受 DSH 沙箱约束？宿主 TCC 授权是否继承？结论直接决定架构是否需要"引擎独立守护进程 + 用户手动授权"。
2. **实验（半天）**：`dsh-mcp-client` + 一个假 `sift-mcp`，观察工具定义带来的固定 token 成本与启动延迟。
3. **决策**：常驻 socket 单实例 vs 每次 spawn。前者省扫描、要求生命周期管理；后者简单、每次都冷。
4. **决策**：`tree.top` 的默认 k 与返回粒度（只回大小/名字/路径，不回子节点）——直接决定上下文预算。
5. **决策**：引擎与插件的版本协商策略，以及预发布阶段的 peer 范围写法（形如 `>=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1`，semver 不会把 `0.1.5-rc.*` 算进第一段）。
6. **确认**：Windows / Linux Tier 2 在冷缓存下的真实表现，是否需要在插件里给出"该平台当前较慢"的诚实提示。
7. **注意**：`desktop` 这个 profile 名被 Electron 端保留，CLI 会拒绝它——插件文档/安装脚本要按 `web` / 自建 profile 来写。
8. **确认**：`ctx.jobs` 是进程内实现、不跨重启，所以"扫到一半重启"必须由引擎自己续上；这决定引擎要不要写扫描日志（仓库里已有 `sift-scan/src/journal.rs` 与 `sift-store/src/journal_sqlite.rs` 的雏形，正好接上）。

---

## 8. 如果只做一件事（一周 MVP）

一个 MCP server，两个工具，一个技能：

- `disk_top(volume, k)` —— 只读，立刻回答"哪里最占地方"；
- `disk_analyze(path)` —— 只读，返回候选 + 等级 + 理由（**先不做删除**）；
- `skills/disk-triage/SKILL.md` —— 分诊流程。

先验证"agent 能不能把磁盘问题讲清楚"。**删除能力放到 P2**——先证明它值得被信任，再给它刀。

---

## 附：DSH 侧出处

- [打包与安装插件](https://deepseek-harness.github.io/deepseek-harness/develop/basic/publish.md)
- [插件配置](https://deepseek-harness.github.io/deepseek-harness/develop/basic/config.md)
- [开发一个工具](https://deepseek-harness.github.io/deepseek-harness/develop/basic/tool.md)
- [插件与生命周期](https://deepseek-harness.github.io/deepseek-harness/develop/framework/index.md)
- [事件系统](https://deepseek-harness.github.io/deepseek-harness/develop/framework/events.md)
- [工具编写参考（长任务 / 策略钩子 / UI 卡片 / PTC）](https://deepseek-harness.github.io/deepseek-harness/reference/cookbook/adding-a-tool.md)
- 本机参考实现：`@deepseek-ai/dsh-tool-todo`（工具插件）、`@openviking/dsh-memory-plugin`（第三方 bundle：技能 + MCP + pre-step 注入的实战笔记）
- 本机另有 DSH 源码 checkout（`~/Documents/deepseek-harness`，比 npm 上的包更全，含 `docs/subsystems/`、`docs/cookbook/`），实现阶段以它为准
- 逐条机制核验与文件路径引用见 [DSH 插件契约调研](../research/dsh-plugin-contract.md)
