/**
 * The in-process mock backend used in browser mode (`pnpm dev` opened outside
 * the Tauri shell).
 *
 * It answers every Tauri command the real backend exposes and simulates a
 * scan by emitting the same events (`scan://batch`, `scan://done`) through the
 * shared event bus, so the whole UI — home progress, explorer drill, smart
 * findings — is exercisable without a native build.
 *
 * This is development-only infrastructure: `src/lib/ipc.ts` never routes here
 * when `window.__TAURI_INTERNALS__` is present.
 */

import type { EventBus } from './events';
import type {
  AiConfig,
  AnalysisSummary,
  Finding,
  HistoryEntry,
  MonitorStatus,
  Node,
  Routine,
  RoutineSuggestion,
  VolumeInfo
} from '../types';

// ---- units -----------------------------------------------------------------

const MB = 1024 * 1024;
const GB = 1024 * MB;
const TB = 1024 * GB;
const DAY_MS = 86_400_000;

const NOW = Date.now();

// ---- static fixtures --------------------------------------------------------

const VOLUMES: VolumeInfo[] = [
  {
    id: 'vol-root',
    name: 'Macintosh HD',
    mountPoint: '/',
    totalBytes: TB,
    availableBytes: 250 * GB,
    isRemovable: false,
    fileSystem: 'apfs'
  },
  {
    id: 'vol-backup',
    name: 'BACKUP',
    mountPoint: '/Volumes/BACKUP',
    totalBytes: 128 * GB,
    availableBytes: 92 * GB,
    isRemovable: true,
    fileSystem: 'exfat'
  }
];

interface MockDir {
  id: string;
  parentId: string | null;
  name: string;
  path: string;
  size: number;
  mtimeMs: number;
}

interface MockFile {
  name: string;
  size: number;
  mtimeMs: number;
}

/** Mock directory tree, parent-before-child. Sizes are rolled-up bytes. */
const DIRS: Array<[string, number]> = [
  ['/', 774 * GB],
  ['/Users', 740 * GB],
  ['/Users/demo', 700 * GB],
  ['/Users/demo/Downloads', 86 * GB],
  ['/Users/demo/Desktop', 12 * GB],
  ['/Users/demo/Documents', 20 * GB],
  ['/Users/demo/Movies', 120 * GB],
  ['/Users/demo/Library', 140 * GB],
  ['/Users/demo/Library/Caches', 90 * GB],
  ['/Users/demo/Library/Logs', 10 * GB],
  ['/Users/demo/projects', 80 * GB],
  ['/Users/demo/projects/web', 30 * GB],
  ['/Users/demo/projects/web/node_modules', 18 * GB],
  ['/Users/demo/projects/api', 40 * GB],
  ['/Users/demo/projects/api/build', 26 * GB],
  ['/Users/demo/.Trash', 24 * GB],
  ['/Applications', 46 * GB],
  ['/Library', 18 * GB]
];

/** Files per directory, used both by `list_dir_files` and file findings. */
const FILES: Record<string, MockFile[]> = {
  '/Users/demo/Downloads': [
    { name: 'Xcode_16.dmg', size: 8 * GB, mtimeMs: NOW - 120 * DAY_MS },
    { name: 'old-backup.zip', size: 22 * GB, mtimeMs: NOW - 200 * DAY_MS },
    { name: 'lecture-recording.mov', size: 55 * GB, mtimeMs: NOW - 400 * DAY_MS }
  ],
  '/Users/demo/Desktop': [{ name: 'screenshot-2024.png', size: 6 * MB, mtimeMs: NOW - 20 * DAY_MS }],
  '/Users/demo/Documents': [
    { name: 'dataset.zip', size: 18 * GB, mtimeMs: NOW - 210 * DAY_MS },
    { name: 'annual-report.pdf', size: 3 * MB, mtimeMs: NOW - 40 * DAY_MS }
  ],
  '/Users/demo/Movies': [
    { name: 'vacation-2019.mov', size: 96 * GB, mtimeMs: NOW - 600 * DAY_MS }
  ],
  '/Users/demo/Library/Caches': [
    { name: 'app-cache.dat', size: 60 * GB, mtimeMs: NOW - 2 * DAY_MS }
  ],
  '/Users/demo/Library/Logs': [
    { name: 'system.log', size: 900 * MB, mtimeMs: NOW - 3 * DAY_MS },
    { name: 'crash-report.crash', size: 700 * MB, mtimeMs: NOW - 14 * DAY_MS },
    { name: 'small.log', size: 2 * MB, mtimeMs: NOW }
  ],
  '/Users/demo/.Trash': [
    { name: 'junk.dmg', size: 6 * GB, mtimeMs: NOW - 90 * DAY_MS },
    { name: 'broken.zip', size: 11 * GB, mtimeMs: NOW - 130 * DAY_MS }
  ],
  '/Users/demo/projects/api/build': [
    { name: 'output.o', size: 20 * GB, mtimeMs: NOW - 10 * DAY_MS }
  ]
};

// ---- id derivation ----------------------------------------------------------

/** Stable id for an absolute path, same shape as the real NodeKey strings. */
function idFor(path: string): string {
  return `n-${path.replace(/[^a-zA-Z0-9]+/g, '_').replace(/^_+|_+$/g, '')}`;
}

function dirRecords(root: string, deleted: Set<string>): MockDir[] {
  // `${root}/` for root '/' would be '//', matching nothing; handle it plainly.
  const within =
    root === '/'
      ? DIRS
      : DIRS.filter(([path]) => path === root || path.startsWith(`${root}/`));
  const live = within.filter(([path]) => {
    for (const removed of deleted) {
      if (path === removed || path.startsWith(`${removed}/`)) return false;
    }
    return true;
  });
  return within.map(([path, size]) => {
    if (path === root) {
      return { id: idFor(path), parentId: null, name: basename(path), path, size, mtimeMs: NOW - DAY_MS };
    }
    const parent = path.slice(0, path.lastIndexOf('/')) || '/';
    return {
      id: idFor(path),
      parentId: idFor(parent),
      name: basename(path),
      path,
      size,
      mtimeMs: NOW - DAY_MS
    };
  });
}

function basename(path: string): string {
  if (path === '/') return '/';
  return path.slice(path.lastIndexOf('/') + 1);
}

function extOf(name: string): string | undefined {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : undefined;
}

// ---- finding construction ---------------------------------------------------

interface DirRule {
  rule: string;
  safety: Finding['safety'];
  reasonKey: string;
  reasonParams: string[];
  kind: string;
  impactKey: string;
  cleanupMethod: 'trashItem' | 'emptyTrash';
  confidence: number;
}

/** Mirror of the Rust `for_dir` conclusions for the mock tree. */
function dirRuleFor(dir: MockDir): DirRule | null {
  const inLibrary = dir.path.includes('/Library/');
  switch (dir.name) {
    case 'node_modules':
      return {
        rule: 'dir.node_modules', safety: 'safe', confidence: 0.9,
        reasonKey: 'reason.rebuildableCache', reasonParams: ['npm'],
        kind: 'rebuildableCache', impactKey: 'cleanup.impact.depsReinstalled',
        cleanupMethod: 'trashItem'
      };
    case 'Caches':
      if (!inLibrary) return null;
      return {
        rule: 'dir.app_cache', safety: 'safe', confidence: 0.9,
        reasonKey: 'reason.cacheDirectory', reasonParams: ['application'],
        kind: 'cacheDirectory', impactKey: 'cleanup.impact.cacheRegenerated',
        cleanupMethod: 'trashItem'
      };
    case 'Logs':
      if (!inLibrary) return null;
      return {
        rule: 'dir.app_logs', safety: 'safe', confidence: 0.9,
        reasonKey: 'reason.log', reasonParams: ['application'],
        kind: 'log', impactKey: 'cleanup.impact.logRegenerated',
        cleanupMethod: 'trashItem'
      };
    case 'build':
      return {
        rule: 'dir.rust_gradle_build', safety: 'review', confidence: 0.5,
        reasonKey: 'reason.rebuildableCache', reasonParams: ['build'],
        kind: 'rebuildableCache', impactKey: 'cleanup.impact.buildArtifactsRegenerated',
        cleanupMethod: 'trashItem'
      };
    case '.Trash':
      return {
        rule: 'dir.trash', safety: 'safe', confidence: 0.9,
        reasonKey: 'reason.trash', reasonParams: [],
        kind: 'trash', impactKey: 'cleanup.impact.trashEmptied',
        cleanupMethod: 'emptyTrash'
      };
    default:
      return null;
  }
}

function makeFinding(
  node: { id: string; name: string; path: string; isDir: boolean; size: number },
  rule: DirRule
): Finding {
  return {
    id: node.id,
    name: node.name,
    path: node.path,
    displayPath: node.path.replace('/Users/demo', '~'),
    size: node.size,
    isDir: node.isDir,
    safety: rule.safety,
    confidence: rule.confidence,
    reason: rule.reasonKey,
    reasonKind: 'key',
    reasonParams: rule.reasonParams,
    source: `rule:${rule.rule}`,
    kind: rule.kind,
    knownCleanable: rule.safety === 'safe',
    approvedForAuto: false,
    cleanupCommand: null,
    cleanupMethod: rule.cleanupMethod,
    impact: rule.impactKey,
    impactKind: 'key',
    impactParams: []
  };
}

/** File findings, mirroring the Rust `for_file` rules. */
function fileFindings(dirPath: string, files: MockFile[]): Finding[] {
  const out: Finding[] = [];
  for (const file of files) {
    const ext = extOf(file.name);
    let rule: DirRule | null = null;
    if (['dmg', 'pkg', 'iso', 'msi', 'exe', 'appimage', 'deb', 'rpm'].includes(ext ?? '')) {
      rule = {
        rule: 'file.installer', safety: 'review', confidence: 0.5,
        reasonKey: 'reason.packageInstaller', reasonParams: [ext ?? ''],
        kind: 'packageInstaller', impactKey: 'cleanup.impact.installerGone',
        cleanupMethod: 'trashItem'
      };
    } else if (['zip', 'tar', 'gz', 'tgz', 'xz', 'bz2', '7z', 'rar'].includes(ext ?? '')) {
      rule = {
        rule: 'file.archive', safety: 'review', confidence: 0.5,
        reasonKey: 'reason.archive', reasonParams: [ext ?? ''],
        kind: 'archive', impactKey: 'cleanup.impact.archiveInTrash',
        cleanupMethod: 'trashItem'
      };
    } else if (['log', 'crash', 'ips', 'dmp', 'diag'].includes(ext ?? '') && file.size >= 64 * MB) {
      rule = {
        rule: 'file.large_log', safety: 'safe', confidence: 0.9,
        reasonKey: 'reason.log', reasonParams: [ext === 'log' ? 'application' : 'crash'],
        kind: 'log', impactKey: 'cleanup.impact.logRegenerated',
        cleanupMethod: 'trashItem'
      };
    } else if (file.size >= 256 * MB && NOW - file.mtimeMs >= 180 * DAY_MS) {
      const days = Math.floor((NOW - file.mtimeMs) / DAY_MS);
      rule = {
        rule: 'file.stale_large', safety: 'review', confidence: 0.5,
        reasonKey: 'reason.staleLargeFile', reasonParams: [String(days)],
        kind: 'staleLargeFile', impactKey: 'cleanup.impact.fileInTrash',
        cleanupMethod: 'trashItem'
      };
    }
    if (!rule || file.size < 8 * MB) continue;
    const path = `${dirPath}/${file.name}`;
    out.push(
      makeFinding(
        { id: idFor(path), name: file.name, path, isDir: false, size: file.size },
        rule
      )
    );
  }
  return out;
}

// ---- backend ---------------------------------------------------------------

export interface MockBackend {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>;
}

export function createMockBackend(bus: EventBus): MockBackend {
  /** Mutable, in-memory state of the simulated machine. */
  const state = {
    language: 'zh' as 'zh' | 'en',
    scanning: false,
    cancelled: false,
    /** Monotonic scan generation; 0 before the first start (P0-7). */
    epoch: 0,
    rootPath: '/',
    scanDirs: [] as MockDir[],
    findings: [] as Finding[],
    /** Persisted definitively-cleanable list, keyed by finding id. */
    cleanable: new Map<string, Finding>(),
    history: [
      {
        id: 'h-2', atMs: NOW - 30 * DAY_MS, bytes: 30 * GB, items: 2,
        automatic: true, titles: ['system.log', 'crash-report.crash']
      },
      {
        id: 'h-1', atMs: NOW - 3 * DAY_MS, bytes: 12 * GB, items: 4,
        automatic: false, titles: ['Xcode_15.dmg', 'old-build']
      }
    ] as HistoryEntry[],
    suggestions: [
      {
        name: 'node_modules',
        kind: 'rebuildableCache',
        occurrences: 6,
        distinctDays: 5,
        averageBytes: 4 * GB,
        totalBytes: 24 * GB,
        destructive: true,
        reason: 'routine.reason.repeatedRemoval',
        cadence: 'routine.cadence.weekly'
      }
    ] as RoutineSuggestion[],
    routines: [] as Routine[],
    monitorRunning: false,
    autoMode: 'off' as 'off' | 'notify' | 'auto',
    scheduled: false,
    scheduledHour: 2,
    timers: [] as Array<ReturnType<typeof setTimeout>>,
    ai: {
      enabled: false,
      provider: 'openai',
      endpoint: 'https://api.openai.com/v1',
      model: 'gpt-4o',
      language: 'zh' as 'zh' | 'en',
      hasToken: false
    },
    /** Absolute paths "moved to trash"; hidden from later scans. */
    deletedPaths: new Set<string>(),
    /** Directories closed (sized) in the current scan. */
    sizedDirs: 0,
    /** Start timestamp of the current scan, for the simulated rate. */
    scanStartMs: Date.now()
  };

  const clearTimers = () => {
    for (const timer of state.timers) clearTimeout(timer);
    state.timers = [];
  };

  /** Turn a mock dir into the Node shape streamed by the real scanner. */
  const dirNode = (dir: MockDir, pending: boolean): Node => ({
    id: dir.id,
    parentId: dir.parentId,
    name: dir.name,
    path: dir.path,
    isDir: true,
    size: pending ? 0 : dir.size,
    modifiedMs: dir.mtimeMs,
    deletable: true,
    pending,
    status: pending ? 'awaiting' : 'ok',
    ext: undefined
  });

  const fileNode = (dirPath: string, file: MockFile): Node => {
    const path = `${dirPath}/${file.name}`;
    return {
      id: idFor(path),
      parentId: idFor(dirPath),
      name: file.name,
      path,
      isDir: false,
      size: file.size,
      modifiedMs: file.mtimeMs,
      deletable: true,
      pending: false,
      status: 'ok',
      ext: extOf(file.name)
    };
  };

  const emitBatch = (dirs: MockDir[], options: { pending: boolean; final: boolean }) => {
    if (state.cancelled) return;
    // Sized batches close their directories; coverage grows monotonically with
    // the fraction of directories measured so the percent ring tracks progress
    // instead of sitting at 0% until the final batch.
    if (!options.pending) {
      state.sizedDirs += dirs.length;
    }
    const ratio = state.scanDirs.length > 0 ? state.sizedDirs / state.scanDirs.length : 0;
    const elapsedSec = Math.max(0.001, (Date.now() - state.scanStartMs) / 1000);
    const bytes = Math.round(774 * GB * ratio);
    bus.emit('scan://batch', {
      epoch: state.epoch,
      discovered: dirs.map((dir) => dirNode(dir, options.pending)),
      sized: dirs.map((dir) => ({
        id: dir.id,
        size: dir.size,
        pending: options.pending,
        status: (options.pending ? 'awaiting' : 'ok') as Node['status']
      })),
      permissions: [],
      progress: {
        files: Math.round(84_213 * ratio),
        dirs: state.sizedDirs,
        bytes,
        totalBytes: 774 * GB,
        percent: options.final ? 100 : Math.min(99, Math.floor(ratio * 100)),
        rateBytesPerSec: Math.round(bytes / elapsedSec),
        awaiting: 0,
        trackedNodes: state.scanDirs.length
      }
    });
  };

  const startScan = (root: string): number => {
    clearTimers();
    state.cancelled = false;
    state.scanning = true;
    state.epoch += 1;
    const epoch = state.epoch;
    state.rootPath = root;
    state.scanDirs = dirRecords(root, state.deletedPaths);
    state.sizedDirs = 0;
    state.scanStartMs = Date.now();
    const half = Math.ceil(state.scanDirs.length / 2);
    const firstHalf = state.scanDirs.slice(0, half);
    const secondHalf = state.scanDirs.slice(half);

    state.timers = [
      setTimeout(() => emitBatch(firstHalf, { pending: true, final: false }), 120),
      setTimeout(() => {
        emitBatch(firstHalf, { pending: false, final: false });
        emitBatch(secondHalf, { pending: true, final: false });
      }, 300),
      setTimeout(() => emitBatch(secondHalf, { pending: false, final: true }), 480),
      setTimeout(() => {
        if (state.cancelled || state.epoch !== epoch) return;
        state.scanning = false;
        bus.emit('scan://done', { epoch, cancelled: false, root });
      }, 600)
    ];
    return epoch;
  };

  /** Re-derive every finding from the current scan tree. */
  const computeFindings = () => {
    const findings: Finding[] = [];
    for (const dir of state.scanDirs) {
      const rule = dirRuleFor(dir);
      if (rule && dir.size >= 8 * MB) {
        findings.push(makeFinding({ ...dir, isDir: true }, rule));
      }
    }
    for (const dir of state.scanDirs) {
      for (const finding of fileFindings(dir.path, FILES[dir.path] ?? [])) {
        findings.push(finding);
      }
    }
    findings.sort((a, b) => b.size - a.size || a.id.localeCompare(b.id));
    state.findings = findings;

    // Fold Safe entries into the persisted cleanable list; drop stale ones.
    const safeIds = new Set(findings.filter((f) => f.safety === 'safe').map((f) => f.id));
    for (const finding of findings) {
      if (finding.safety !== 'safe') continue;
      const remembered = state.cleanable.get(finding.id);
      state.cleanable.set(finding.id, {
        ...finding,
        approvedForAuto: remembered?.approvedForAuto ?? false
      });
    }
    for (const id of [...state.cleanable.keys()]) {
      if (!safeIds.has(id)) state.cleanable.delete(id);
    }
  };

  const invoke = <T>(command: string, args: Record<string, unknown> = {}): Promise<T> =>
    new Promise<T>((resolve, reject) => {
      switch (command) {
        // ---- volumes & scanning ----
        case 'list_volumes':
          resolve(VOLUMES as unknown as T);
          break;
        case 'start_scan':
          resolve(startScan(String(args.root ?? '/')) as unknown as T);
          break;
        case 'cancel_scan': {
          if (!state.scanning) {
            resolve(undefined as T);
            break;
          }
          const epoch = state.epoch;
          clearTimers();
          state.cancelled = true;
          state.scanning = false;
          bus.emit('scan://done', { epoch, cancelled: true, root: state.rootPath });
          resolve(undefined as T);
          break;
        }
        case 'set_scan_focus':
          resolve(undefined as T);
          break;
        case 'scan_running':
          resolve((state.scanning ? state.epoch : null) as unknown as T);
          break;
        case 'list_dir_files': {
          const path = String(args.path ?? '');
          resolve((FILES[path] ?? []).map((file) => fileNode(path, file)) as unknown as T);
          break;
        }
        case 'resolve_permission':
        case 'watch_fs':
        case 'unwatch_fs':
          resolve(undefined as T);
          break;

        // ---- analysis & cleanable ----
        case 'analyze_current': {
          computeFindings();
          const sourceCounts: Record<string, number> = {};
          for (const finding of state.findings) {
            sourceCounts[finding.source] = (sourceCounts[finding.source] ?? 0) + 1;
          }
          const summary: AnalysisSummary = {
            findings: state.findings,
            reclaimableBytes: state.findings
              .filter((f) => f.safety !== 'keep')
              .reduce((sum, f) => sum + f.size, 0),
            safeBytes: state.findings
              .filter((f) => f.safety === 'safe')
              .reduce((sum, f) => sum + f.size, 0),
            knownCleanableBytes: [...state.cleanable.values()].reduce(
              (sum, f) => sum + f.size,
              0
            ),
            sourceCounts,
            // The simulated adjudicator sits behind the router, same as the
            // real default pipeline, so the UI's consent logic sees a remote.
            usedRemote: true,
            remoteNeedsConsent: false
          };
          resolve(summary as unknown as T);
          break;
        }
        case 'list_cleanable':
          resolve([...state.cleanable.values()] as unknown as T);
          break;
        case 'set_cleanable_approval': {
          const entry = state.cleanable.get(String(args.id ?? ''));
          if (!entry) {
            resolve(false as unknown as T);
            break;
          }
          entry.approvedForAuto = Boolean(args.approved);
          resolve(true as unknown as T);
          break;
        }
        case 'approve_structural': {
          let count = 0;
          for (const entry of state.cleanable.values()) {
            if (!entry.approvedForAuto) {
              entry.approvedForAuto = true;
              count += 1;
            }
          }
          resolve(count as unknown as T);
          break;
        }
        case 'forget_cleanable': {
          resolve(state.cleanable.delete(String(args.id ?? '')) as unknown as T);
          break;
        }
        case 'clean_paths': {
          const paths = (args.paths as string[]) ?? [];
          for (const path of paths) {
            state.deletedPaths.add(path);
            const slash = path.lastIndexOf('/');
            const dir = path.slice(0, slash);
            const name = path.slice(slash + 1);
            const rows = FILES[dir];
            if (rows) FILES[dir] = rows.filter((file) => file.name !== name);
            state.scanDirs = state.scanDirs.filter(
              (d) => d.path !== path && !d.path.startsWith(`${path}/`)
            );
          }
          resolve(
            paths.map((path) => ({ path, ok: true, error: null })) as unknown as T
          );
          break;
        }

        // ---- monitor ----
        case 'monitor_status': {
          const cleanable = [...state.cleanable.values()];
          const eligible = cleanable.filter((f) => f.approvedForAuto);
          const status: MonitorStatus = {
            running: state.monitorRunning,
            enabled: true,
            autoMode: state.autoMode,
            cleanableItems: cleanable.length,
            autoEligibleItems: eligible.length,
            autoEligibleBytes: eligible.reduce((sum, f) => sum + f.size, 0),
            lastLevel: 'ok',
            lastAvailableBytes: VOLUMES[0].availableBytes,
            lastTotalBytes: VOLUMES[0].totalBytes,
            scheduledCleanupEnabled: state.scheduled,
            scheduledHour: state.scheduledHour
          };
          resolve(status as unknown as T);
          break;
        }
        case 'start_monitor':
          state.monitorRunning = true;
          resolve(undefined as T);
          break;
        case 'stop_monitor':
          state.monitorRunning = false;
          resolve(undefined as T);
          break;
        case 'set_auto_clean_mode':
          state.autoMode = String(args.mode) as MonitorStatus['autoMode'];
          resolve(undefined as T);
          break;
        case 'set_scheduled_cleanup':
          state.scheduled = Boolean(args.enabled);
          state.scheduledHour = Number(args.hour ?? 2);
          resolve(undefined as T);
          break;

        // ---- history, routines ----
        case 'list_history':
          resolve([...state.history].reverse() as unknown as T);
          break;
        case 'list_routines':
          resolve([...state.routines] as unknown as T);
          break;
        case 'routine_suggestions':
          resolve([...state.suggestions] as unknown as T);
          break;
        case 'accept_routine_suggestion': {
          const name = String(args.name ?? '');
          const kind = String(args.kind ?? '');
          const index = state.suggestions.findIndex(
            (s) => s.name === name && s.kind === kind
          );
          if (index < 0) {
            reject(new Error('err.routineSuggestionGone'));
            break;
          }
          const suggestion = state.suggestions[index];
          state.suggestions.splice(index, 1);
          const routine: Routine = {
            id: `routine-${suggestion.kind}-${suggestion.name}`,
            title: suggestion.name,
            kind: suggestion.kind,
            cadence: suggestion.cadence,
            averageBytes: suggestion.averageBytes,
            mode: 'approve',
            paths: []
          };
          state.routines.push(routine);
          resolve(undefined as T);
          break;
        }
        case 'save_routine': {
          const title = String(args.title ?? '');
          const kind = String(args.kind ?? '');
          const averageBytes = Number(args.averageBytes ?? 0);
          const path = args.path ? String(args.path) : null;
          const routine: Routine = path
            ? {
                id: `custom-${idFor(path)}`,
                title,
                kind: 'customPath',
                cadence: 'cadence.weekly',
                averageBytes,
                mode: 'approve',
                paths: [path]
              }
            : {
                id: `routine-${kind}-${title}`,
                title,
                kind,
                cadence: 'cadence.weekly',
                averageBytes,
                mode: 'approve',
                paths: []
              };
          state.routines = state.routines.filter((r) => r.id !== routine.id);
          state.routines.push(routine);
          resolve(undefined as T);
          break;
        }
        case 'dismiss_routine_suggestion': {
          const name = String(args.name ?? '');
          const kind = String(args.kind ?? '');
          state.suggestions = state.suggestions.filter(
            (s) => !(s.name === name && s.kind === kind)
          );
          resolve(undefined as T);
          break;
        }
        case 'delete_routine': {
          const id = String(args.id ?? '');
          const before = state.routines.length;
          state.routines = state.routines.filter((r) => r.id !== id);
          resolve((state.routines.length < before) as unknown as T);
          break;
        }
        case 'toggle_routine_mode': {
          const routine = state.routines.find((r) => r.id === String(args.id ?? ''));
          if (!routine) {
            resolve(false as unknown as T);
            break;
          }
          routine.mode = routine.mode === 'auto' ? 'approve' : 'auto';
          resolve(true as unknown as T);
          break;
        }
        case 'run_routine': {
          resolve(
            [{ path: state.rootPath, ok: true, error: null }] as unknown as T
          );
          break;
        }
        case 'mark_path': {
          const path = String(args.path ?? '');
          const name = basename(path);
          const finding: Finding = {
            id: idFor(path),
            name,
            path,
            displayPath: path.replace('/Users/demo', '~'),
            size: typeof args.bytes === 'number' ? args.bytes : 4 * GB,
            isDir: false,
            safety: 'review',
            confidence: 0.5,
            reason: 'reason.userMarked',
            reasonKind: 'key',
            reasonParams: [],
            source: 'user',
            kind: 'userMarked',
            knownCleanable: false,
            approvedForAuto: false,
            cleanupCommand: null,
            cleanupMethod: 'trashItem',
            impact: 'cleanup.impact.markedInTrash',
            impactKind: 'key',
            impactParams: []
          };
          resolve(finding as unknown as T);
          break;
        }
        case 'take_store_warnings':
          resolve([] as unknown as T);
          break;
        case 'store_location':
          resolve('~/Library/Application Support/sift/store.json' as unknown as T);
          break;

        // ---- language & AI config ----
        case 'get_language':
          resolve(state.language as unknown as T);
          break;
        case 'set_language':
          state.language = String(args.language) === 'en' ? 'en' : 'zh';
          resolve(undefined as T);
          break;
        case 'get_ai_config': {
          const config: AiConfig = {
            enabled: state.ai.enabled,
            provider: state.ai.provider,
            endpoint: state.ai.endpoint,
            model: state.ai.model,
            language: state.ai.language,
            batchSize: 40,
            hasToken: state.ai.hasToken
          };
          resolve(config as unknown as T);
          break;
        }
        case 'save_ai_config': {
          const config = (args.config as {
            enabled: boolean;
            provider: string;
            endpoint: string;
            model: string;
            language: string;
            token: string | null;
          }) ?? {
            enabled: false, provider: '', endpoint: '', model: '', language: '', token: null
          };
          state.ai.enabled = config.enabled;
          state.ai.provider = config.provider;
          state.ai.endpoint = config.endpoint;
          state.ai.model = config.model;
          state.ai.language = config.language === 'en' ? 'en' : 'zh';
          state.ai.hasToken = Boolean(config.token);
          resolve(
            {
              enabled: state.ai.enabled,
              provider: state.ai.provider,
              endpoint: state.ai.endpoint,
              model: state.ai.model,
              language: state.ai.language,
              batchSize: 40,
              hasToken: state.ai.hasToken
            } as unknown as T
          );
          break;
        }
        default:
          reject(new Error(`mock backend: unknown command "${command}"`));
      }
    });

  return { invoke };
}
