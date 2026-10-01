# DSH Plugin / Extension Authoring Contract — research report

## Sources and a version caveat

Two independent sources were used, and they are **not the same version**:

- **Installed packages** — `/Users/xiaocy/.npm/_npx/1e7f6d9597241db0/node_modules/@deepseek-ai/`, at `@deepseek-ai/dsh` **0.1.5-rc.3** (`dsh/package.json`).
- **Full repo checkout** — `/Users/xiaocy/Documents/deepseek-harness`, HEAD `639ed01539` = **0.2.0-rc.2** (`package.json` → `@deepseek-ai/dsh-root 0.2.0-rc.2`), remote `git@github.com:deepseek-ai/deepseek-harness.git`.

The npm tree contains only the launcher's dependency closure; the Web GUI packages (`dsh-client-ui-slots`, `ui-dockkit`, `ui-primitives`) are **not** in it. Where the checkout docs describe newer behavior than the installed `.d.ts`, I say so.

Doc paths worth citing (all under the checkout, mirrored at `github.com/deepseek-ai/deepseek-harness`): `docs/user/develop/basic/{index,tool,config,publish}.md`, `docs/cookbook/{adding-a-tool,extension-cookbook,adding-a-settings-card}.md`, `docs/subsystems/{slots,sidebar-right,client-modules}.md`, `docs/postmortem/0001-acp-default-export-drops-inject.md`, `docs/config-catalog.md`, `apps/cli/reference/README.md`. Package READMEs ship on npm under each `@deepseek-ai/*` package.

---

## 1. Package contract

A third-party plugin is an **npm package that declares `dsh.bundle.patch`**. The launcher only treats it as a profile layer if that field exists — `plugin-Ddi42qoW.js:25-33`:

```js
return readProfileManifest(NAME, dir).dsh?.bundle?.patch !== void 0;
```

and `dsh-app-boot/lib/index.js:851` fails loud for a listed bundle without it: `` `profile bundle ${name} declares no dsh.bundle in its package.json` ``. `DshBundleManifest` is `{ patch: string }` (`dsh-package-manifest/lib/types/types.d.ts`); `publish.md:64` adds that `patch` may also be an ordered list of files (newer than the installed type, which is `string`).

Real third-party package (`/Users/xiaocy/.dsh/profiles/web/node_modules/@openviking/dsh-memory-plugin/package.json`):

```json
"type": "module",
"main": "index.mjs",
"exports": { ".": "./index.mjs" },
"files": ["index.mjs", "...", "cordis.patch.yml", "README.md"],
"dsh": { "bundle": { "patch": "./cordis.patch.yml" } },
"peerDependencies": {
  "@deepseek-ai/dsh-llm": ">=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1",
  "@deepseek-ai/dsh-mcp-client": ">=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1",
  "@deepseek-ai/dsh-skill-filesystem": ">=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1"
}
```

ESM only (`"type": "module"`); no runtime dsh dependencies. `publish.md:103`: declare packages whose instances must be shared with the host under **both** `peerDependencies` and `devDependencies`; keep independent third-party deps under `dependencies`. Version pinning convention: a dual-range peer (`>=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1`) because semver does not admit `0.1.5-rc.*` in the first clause, with devDeps/`overrides` pinned to the minimum supported rc.

**Install:** `dsh plugin --profile web add @openviking/dsh-memory-plugin`. This is a thin pnpm forwarder run with cwd = `$DSH_HOME/profiles/<p>/`, then **reconciled by installed state**: every dependency resolving to a `dsh.bundle` package is appended to `dsh.profile.bundles`; non-bundle deps get a warning; removed ones leave (`plugin-Ddi42qoW.js:46-78,101-128`). Relative specs are re-anchored to your cwd (`add .` would otherwise self-link the profile). A git install needs a `prepare` script **and** the user must add `allowBuilds: { <pkg>: true }` to the profile's `pnpm-workspace.yaml`.

**Profile manifest** (`~/.dsh/profiles/web/package.json`) carries `dsh.profile.bundles` (ordered) + `patchReload: "live" | "startup"`. The profile's `pnpm-workspace.yaml` is generated with `nodeLinker: hoisted`, `autoInstallPeers: false`.

**`@deepseek-ai/*` peer resolution is two-anchor, installation-first**: `resolveBundleDir` tries the dsh installation anchor, then the profile dir (`dsh-app-boot/lib/index.js:824-832`). At boot, `healProfilesModuleFallback` mirrors the installation's dependency closure into `$DSH_HOME/profiles/node_modules` (symlinks on plain Node, ESM proxies for packaged executables). Hence the OpenViking README: *"Keep those packages in the host installation rather than adding individual DSH core packages to the profile."* Adding `@deepseek-ai/dsh-skill*` directly can shadow the host copy and break startup.

## 2. Cordis plugin shape

Three forms (`basic/index.md:105-137`): function (named exports), object (`export default { name, inject, apply }`), class (`export default class X extends Service`). Canonical form:

```js
export const name = 'hello-plugin'
export const inject = ['tools']
export function apply(ctx, config) { /* registrations */ }
```

`inject` makes Cordis hold the fiber in PENDING until every service exists; if a service disappears the plugin auto-unloads and reloads.

**Config** (`basic/config.md`) — export a `Config` interface plus a same-named Schemastery schema; defaults live on the schema fields, and it must implement Standard Schema (a plain object is rejected):

```ts
import Schema from '@deepseek-ai/schemastery'
export const Config = Schema.object({
  greeting: Schema.string().default('Hello'),
  verbose: Schema.boolean().default(false),
})
```

`dsh-tool-todo/lib/index.js` uses `const Config = z.object({ allowParallelInProgress: z.boolean().required() })`, i.e. a required field with no default **fails the load** when omitted.

**Why not `export default apply`:** a postmortem, `docs/postmortem/0001-acp-default-export-drops-inject.md`. The loader normalizes via `unwrapExports`:

```ts
exports = exports.default ?? exports   // ← prefers .default
```

so `export default apply` makes the Loader build the fiber from the **bare function**, discarding the sibling `name`/`inject`/`Config` named exports → `cannot get property "agents" without inject`. Fix: delete it. `docs/testing.md:40` encodes this as a guard: composition plugins assert `expect('default' in mod).toBe(false)`. So a default export *is* loadable (object/class form), but a default that shadows named metadata is a known failure mode.

**`cordis.patch.yml`** is a top-level YAML array of loader patch entries. Insert new rows with `- insert: [...]`; override by id with `- id: <row id>` + `config:` (a patch **replaces** a row's whole `config`, it does not deep-merge — `publish.md:129-132`); `disabled: true` disables. The OpenViking layer nests a group:

```yaml
- insert:
    - id: openviking-memory
      name: '@deepseek-ai/cordis-plugin-group'
      group: true
      isolate: { openvikingMemory: true }
      config:
        - id: openviking-memory-runtime
          name: '@openviking/dsh-memory-plugin'
```

`dsh --profile web --dump-config` prints one commented layer per source file (`# == dsh-hello-plugin`, `publish.md:112`) with the composed rows. Nested group entry ids use `:` separators (`cordis-plugin-group/README.md`). `--dump-default-config` skips the user layer/overlays. Caveat: `prepareProfile` **rewrites** the profile's `cordis.yml` on every dump, and it also parses `!!js`; I did not run it in this read-only task.

## 3. Registering a model-facing tool

```ts
import { defineTool } from '@deepseek-ai/dsh-tools'
export const inject = ['tools']
ctx.tools.register(defineTool({
  name: 'read_file',
  description: 'Read a file from disk.',
  parameters: {
    path: { type: 'string', required: true, description: 'Absolute path' },
    limit: { type: 'number' },
  },
  output: { schema: { type: 'string' }, render: (_a, v) => [{ type: 'text', text: v }] },
  async execute(args, exec) { return readFile(args.path, { encoding: 'utf8', signal: exec.signal }) },
}))
```

- Registration is effect-based; disposing the fiber unregisters. Schemas flow into prompt assembly automatically.
- `args` are validated by `defineTool` against `ParameterSchemaSpec` before `execute`; hand-check what the DSL cannot express. `exec` carries immutable `callId`/`name`/`arguments`/`agent`/`token` plus the **required `exec.signal`**, and `deferContext(msg)` / `concludeTurn()`.
- **Result shape:** return *only* the canonical JSON value declared by `output.schema`; the registry snapshots, validates, freezes it, then calls `output.render`. Never return content blocks.
- **Errors:** throwing (or returning an invalid value) becomes `isError`; the model sees exactly `Error: <message>`. Unknown/hidden tools → `UNKNOWN_TOOL`. Failures never end the turn.
- **Approval (deletion gate):** the reorderable hook is the `tools/pre-execute` waterfall returning `{kind:'allow'|'deny'|'ask'}` (`docs/cookbook/extension-cookbook.md:11-33`). `ask` resolves through `ctx.approval` (`ApprovalService.request(req) → 'allowed-once' | 'rejected' | 'cancelled' | 'unavailable'`); **a missing answerer fails closed to denial**. Compose `@deepseek-ai/dsh-user-approval`; policy is `ask` (default) or `never`. For a hard invariant use `ctx.tools.guard(exec => reason | undefined)` — monotonic, no later listener can undo it. Prefer hooks over building policy into the tool.
- **Visibility/scoping:** registering through `agent.ctx` puts the tool in that agent's scope, where it **shadows** globals. `ctx.tools.restrict({allow?, deny?})` filters the inherited surface (masks intersect; own-scope registrations are exempt); `ctx.tools.get(name, scope)` / `schemas(scope)` resolve a scope's view.

## 4. Dynamic / rich content

`ContentBlockMap` (`dsh-llm/lib/types/types.d.ts:94-102`) = `text | reasoning | image | file | tool-call | tool-result`. An **image is an attachment reference**, not bytes: `{ type: 'image', attachment: ImageAttachmentRef }`.

Structured replayable data goes through `output.presentationMeta(args, value)` — persisted on `tool/result` and handed to `presentResult` so cards survive replay (skipped for nested PTC dispatches).

Two card vocabularies exist, and **the built-in Web Client consumes neither**. `adding-a-tool.md:95` is explicit: *"The built-in Web Client does not consume `presentCall` or `presentResult`."* They are Host-local render intents (`ToolCallView`: `generic`/`terminal`/`diff`; `ToolResultView`: `generic`/`terminal`/`diff`/`search`/`read`/`web`), pure functions of args (+result) that also run on replay — no I/O, clock, or session state. To get a real Web card you must ship a client plugin and register the **wire tool name** into the keyed slot:

```text
ctx.slots.inject('tool.call.toolview', () =>
  ctx.slots.register({ name: 'tool.call.toolview', key: '<wire tool name>' }, BusinessToolRow))
```

There is no documented third-party card-registry API beyond this slot; props are derived from raw `ToolCallBlock` args/content/error/meta.

**Progress/streaming:** I found no API for a running tool to emit incremental UI updates. Live token streaming is `agent/assistant-stream` (an emit event), which is the *model's* stream, not tool progress. Long-running work is dispatched to `run_in_background`/`ctx.jobs` and read back by polling, not pushed.

`dsh-tool-present` (`present`) declares existing files as deliverables: `files: [{ path, description? }]`, `maxFiles: 8`; relative paths resolve against the session workspace, absolute paths may point outside; contents are never copied. It appends `deliverables/presented` and requires `tools`, `fs`, and the `turnBoundary` projection.

## 5. GUI / client plugin

Declaration is `package.json` → `dsh.client` (`DshClientManifest`): `platform: 'web'`, `inject` (informational package names), `immediately` (boot phase-one barrier), `external` (extra module-table requests). Plus an `exports["./client"]` bundle. Node half is usually an empty `apply`:

```js
// dsh-client-ui-settings-plugins/lib/index.js
/** Host plugin body — no host-side behavior for this surface plugin. */
function apply() {}
```

and the manifest:

```json
"exports": { ".": {...}, "./client": { "types": "./lib/types/client/index.d.ts", "default": "./lib/client.js" } },
"dsh": { "client": { "inject": ["@deepseek-ai/dsh-client-locale", "@deepseek-ai/dsh-client-ui-settings"], "platform": "web" } }
```

`ctx.slots.register({name, id|key, order, ...}, Component)` inside `ctx.slots.inject(key, ...)`. Slots are `single | list | keyed | chain`, scoped `root | session-maybe | session`; register under an unoccupied `id`/`key` for additive extensions. A sidebar tab is **two** registrations sharing an `id`: `ctx.sidebarRightTabs.register({id, kind, patterns, priority, title, canOpen, keepMounted, guide})` plus the keyed slot `sidebar.right.pane.tab` (`docs/subsystems/sidebar-right.md`).

**Build/HMR:** the client build must have produced `lib/client.js` in the lazy-CJS factory format **before launch** — a missing bundle fails activation loudly (`dsh-client-modules/README.md`). `dsh-client-hmr` swaps a plugin in place when a rebuild watcher rewrites the bundle; it needs `pnpm run dev:web` (or any tsdown watch) running, and does nothing in production. Important for third parties: the `clientBundle` tsdown preset lives at `packages/client/tsdown.client.ts`, **not in a published package**, so an out-of-repo package must reproduce that build itself (`adding-a-settings-card.md:60`).

## 6. Skills and MCP

**Skills** — `ctx.skills` is a `SkillRegistry` accepting any provider. Contribute a bundled directory with a second provider (`skills.mjs` in OpenViking):

```js
import * as skillFilesystem from "@deepseek-ai/dsh-skill-filesystem";
export function mountOpenVikingSkills(ctx) {
  return ctx.plugin(skillFilesystem, {
    providerName: "openviking",            // must not collide with `filesystem`
    includeDefaultRoots: false,
    bundledSkillDir: SKILLS_DIR,           // rank 600 root
  });
}
```

`Config` fields: `providerName`, `includeDefaultRoots`, `dshHome`, `agentsHome`, `customSkillDirs`, `watch*`, `bundledSkillDir`. Default roots and ranks: project `.dsh/skills` (100) → `.agents/skills` (200) → custom (300) → user `~/.dsh/skills` (400) → `~/.agents/skills` (500) → bundled (600). A skill is `<name>/SKILL.md` (nested `**/SKILL.md` is not discovered) or a flat `<name>.md` with YAML frontmatter: required `name`/`description`, optional `whenToUse`, `metadata`, `disable-model-invocation`, `user-invocable`.

**MCP** — one plugin instance per server (`mcp.mjs`):

```js
import * as mcpClient from "@deepseek-ai/dsh-mcp-client";
return ctx.plugin(mcpClient, {
  transport: "stdio", serverName: "openviking",
  command: process.execPath, args: [PROXY_PATH], env: { ELECTRON_RUN_AS_NODE: "1" },
  toolCallTimeoutMs: 60_000,
});
```

`Config = StdioConfig | StreamableHttpConfig`: `transport: 'stdio' | 'streamable-http'`, required `serverName` (`[A-Za-z0-9_-]{1,32}`, unique per registration scope), `command/args/env/cwd` or `url/headers`, `toolCallTimeoutMs` (60 000), `failOnStartupError` (false), `reconnect.{enabled,initialDelayMs,maxDelayMs,maxAttempts}`. Tools appear as `mcp__<serverName>__<rawName>`, re-sync on server announcement, and are stable across restarts. MCP resources/prompts are unsupported. They can equally be mounted from a profile patch row rather than code.

**Inventory** — `@deepseek-ai/dsh-host-plugin-inventory` exposes a read-only `pluginInventory/list` Remote (entry id, specifier, effective enablement, fiber phase, per-preset compositions). `@deepseek-ai/dsh-plugin-package-inventory-deepseek` owns a `dsh_plugin_packages` request field (`enabled: true`). Neither requires plugin-side work.

## 7. Lifecycle, hooks, and events

From `dsh-agent/lib/types/runtime-types.d.ts` and `dsh-session/lib/types/index.d.ts`:

| Event | Mode | Payload |
|---|---|---|
| `agent/session-start` | emit | `{ agent, source }` — seed with `agent.inject()` |
| `agent/pre-step` | **waterfall** | `{ agent, messages, turn, step, signal }, next → PreStepDecision` (`reject \| enter`) |
| `agent/request` | waterfall | `next → LlmCallConfig` |
| `agent/request-error` | waterfall | `next → RequestErrorAction` (`{kind:'retry'}`) |
| `agent/assistant-stream` | emit | `{ agent, frame }` (live start/chunk/end) |
| `agent/turn-stopping` | **serial** | `{ agent, turn, signal }` — may `agent.steer()` |
| `agent/created` / `agent/disposed` / `agent/status` / `agent/inbox/*` / `agent/error` | emit | — |
| `session/created` / `session/disposed` / `session/event(session,event)` / `session/flush` | emit | durable log |
| `tools/pre-execute` \| `tools/execute` \| `tools/post-execute` \| `tools/result` \| `tools/change` | wf/wf/wf/emit/emit | pipeline |

**Proactive chat injection** — three distinct APIs on `Agent`:
- `inject(message)` queues model-facing context for the next pre-step **without waking** an idle agent;
- `followup(message)` wakes a new turn;
- `steer(message)` targets the nearest step.

Build the message with `createUserMessage({ content: [{type:'text',text}], source: { kind:'plugin', plugin:'<name>', form:'notice' } })` (`ContextForm` = `instructions|catalog|snapshot|notice|relay|recall`). Guard against disposed agents.

**Finding agents:** `inject: ['agents']` → `ctx.agents.list()`, `.roots()`, `.get(sessionId)`.

**Timers:** `ctx.timeout(cb, ms)` / `ctx.interval(cb, ms)` / `ctx.throttle` / `ctx.debounce`, each returning a disposer that rides the fiber (`ctx.setInterval`/`setTimeout` are deprecated aliases). Use `ctx.effect(() => () => clearInterval(t))` for ordered teardown.

**Background jobs:** `ctx.jobs.start({ kind, label, owner: exec.agent, run }) → JobId` from `@deepseek-ai/dsh-jobs` (impl `dsh-jobs-local`, in-process, not restart-durable).

**Durable UI state:** `ctx.sessionProjections.register({ key, stateSchema, init, apply, wire, stateVersion })` — a pure synchronous fold over committed events; the framework owns subscription and change notification.

**"Background disk watcher that warns in chat" recipe:** a normal plugin with `inject: ['agents']`; `ctx.interval` polls disk (its disposer rides the fiber); on a threshold, for each `ctx.agents.list()` choose `agent.followup(warning)` when `agent.status === 'idle'` (wakes a turn) and `agent.inject(warning)` when `running` (attach to the next step without stealing the turn). Attribute the message with `source: { kind: 'plugin', plugin: 'disk-watch', form: 'notice' }` so it is durable, replayable, and visible to compaction. This mirrors the shipped `/loop` and scheduled-task patterns in `docs/cookbook/extension-cookbook.md:108,127`.

## 8. Packaging and distribution

- **Local directory install works:** `dsh plugin --profile demo add ./hello-plugin` → pnpm records `"dsh-hello-plugin": "link:/path/to/hello-plugin"` and appends the name to `dsh.profile.bundles`. A linked checkout keeps **its own** `node_modules`, so dsh peers must be resolvable there (declare them in both peer and dev dependencies).
- **npm:** `dsh plugin --profile web add your-package`, with `lib/` built at publish time (`publishConfig.access: "public"` on scoped packages). **Tarball:** `pnpm pack`, then `add ./hello-plugin-0.1.0.tgz`. Both avoid build permissions.
- **GitHub:** `add github:you/hello-plugin` fetches sources; your `prepare` script must build self-contained, and the user must allowlist `allowBuilds` in the profile's `pnpm-workspace.yaml`. Treat that as permission to run the package's code at install time; pin a commit.
- **Verify without booting:** `dsh --profile demo --dump-config` shows the bundle as its own `# == …` layer.
- **Documented troubleshooting** (OpenViking README): a profile-local copy of a core `@deepseek-ai/*` package can shadow the host copy and break the whole skill loader; diagnose with `dsh plugin --profile <p> why <pkg>` and inspect `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml`; keep `autoInstallPeers: false` and `nodeLinker: hoisted`; never patch `node_modules`.
- The `desktop` profile name is reserved by the Electron app and rejected by the CLI.

**Not discoverable in the installed files:** any npm registry publish workflow beyond `pnpm publish` conventions, and any third-party client-plugin build preset (the tsdown `clientBundle` preset is repo-internal).

---

## Minimal hello-world plugin (registers one tool)

```
hello-plugin/
├── package.json
├── cordis.patch.yml
└── index.mjs
```

**`package.json`**

```json
{
  "name": "dsh-hello-plugin",
  "version": "0.1.0",
  "description": "Minimal DSH bundle: registers one model-facing tool",
  "type": "module",
  "main": "index.mjs",
  "exports": { ".": "./index.mjs" },
  "files": ["index.mjs", "cordis.patch.yml"],
  "dsh": { "bundle": { "patch": "./cordis.patch.yml" } },
  "dependencies": { "@deepseek-ai/schemastery": "3.18.2" },
  "peerDependencies": { "@deepseek-ai/dsh-tools": ">=0.1.0-rc.6 <0.2.0 || ^0.1.5-rc.1" },
  "devDependencies": { "@deepseek-ai/dsh-tools": "0.1.0-rc.6" },
  "engines": { "node": "^22.19.0 || >=24" },
  "publishConfig": { "access": "public" },
  "license": "MIT"
}
```

**`cordis.patch.yml`**

```yaml
- insert:
    - id: hello-plugin
      name: 'dsh-hello-plugin'
```

**`index.mjs`** (plain JS needs no build step; a TS package would compile to this)

```js
import { defineTool } from '@deepseek-ai/dsh-tools'
import Schema from '@deepseek-ai/schemastery'

export const name = 'hello-plugin'          // named exports, never `export default apply`
export const inject = ['tools']

export const Config = Schema.object({
  greeting: Schema.string().default('Hello'),
})

export function apply(ctx, config) {
  ctx.tools.register(defineTool({
    name: 'greet',
    description: 'Greet someone by name.',
    parameters: {
      name: { type: 'string', required: true, description: 'The name to greet' },
    },
    output: {
      schema: { type: 'string' },
      render: (_args, value) => [{ type: 'text', text: value }],
    },
    async execute(args) {
      return `${config.greeting}, ${args.name}!`
    },
  }))
}
```

Install and verify:

```sh
dsh plugin --profile demo add ./hello-plugin
dsh --profile demo --dump-config     # expect a "# == dsh-hello-plugin" layer
dsh --profile demo
```
