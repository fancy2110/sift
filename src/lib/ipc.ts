import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isTauri } from './runtime';
import { createEventBus } from './mock/events';
import { createMockBackend } from './mock/backend';
import type {
  AiConfig,
  AnalysisSummary,
  MonitorStatus,
  Node,
  Routine,
  RoutineSuggestion,
  StoreWarning,
  VolumeInfo
} from './types';

// ---- transport selection ----------------------------------------------------
//
// Every command goes through `call`, every streamed event through `subscribe`.
// Inside the Tauri webview they use the native bridge; in a plain browser
// (e.g. opening the Vite URL during `pnpm dev`) they fall back to an
// in-process mock backend sharing the exact same command/event shapes, so the
// whole UI stays exercisable without a native build.

let mockBus: ReturnType<typeof createEventBus> | null = null;
let mockBackend: ReturnType<typeof createMockBackend> | null = null;

function ensureBus(): ReturnType<typeof createEventBus> {
  mockBus ??= createEventBus();
  return mockBus;
}

function ensureMock() {
  mockBackend ??= createMockBackend(ensureBus());
  return mockBackend;
}

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) return invoke<T>(command, args);
  return ensureMock().invoke<T>(command, args);
}

function subscribe<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (isTauri()) {
    return listen<T>(event, (e) => cb(e.payload));
  }
  return Promise.resolve(ensureBus().listen<T>(event, cb) as unknown as UnlistenFn);
}

// ---- commands: volumes & scanning ------------------------------------------

export function listVolumes(): Promise<VolumeInfo[]> {
  return call<VolumeInfo[]>('list_volumes');
}

export function startScan(root: string, focus: string): Promise<number> {
  // Returns the scan epoch (P0-7); the mock transport reports the same shape.
  return call<number>('start_scan', { root, focus });
}

export function cancelScan(): Promise<void> {
  return call('cancel_scan');
}

export function setScanFocus(focus: string): Promise<void> {
  return call('set_scan_focus', { focus });
}

export function scanRunning(): Promise<number | null> {
  // Current scan epoch, or null when idle (P0-7).
  return call<number | null>('scan_running');
}

/**
 * Live snapshot of a folder's immediate entries. The front end keeps only
 * directories from the scan; file entries are fetched on demand when the
 * explorer opens a folder.
 */
export function listDirFiles(path: string): Promise<Node[]> {
  return call<Node[]>('list_dir_files', { path });
}

/**
 * Resolve a directory parked pending macOS authorization.
 * `granted` re-walks it; `false` closes it as denied/unknown. When the user
 * selected an ancestor folder in the panel, pass it as `grantedPath`.
 */
export function resolvePermission(
  id: string,
  granted: boolean,
  grantedPath?: string,
): Promise<void> {
  return call('resolve_permission', { id, granted, grantedPath: grantedPath ?? null });
}

// ---- commands: deletion & filesystem awareness -----------------------------

export interface DeleteResultItem {
  path: string;
  ok: boolean;
  error: string | null;
}

export function watchFs(root: string, recursive: boolean): Promise<void> {
  return call('watch_fs', { root, recursive });
}

export function unwatchFs(): Promise<void> {
  return call('unwatch_fs');
}

// ---- commands: analysis, persistence & monitoring ---------------------------

export function analyzeCurrent(): Promise<AnalysisSummary> {
  return call<AnalysisSummary>('analyze_current');
}

export function listCleanable(): Promise<import('./types').Finding[]> {
  return call('list_cleanable');
}

export function setCleanableApproval(id: string, approved: boolean): Promise<boolean> {
  return call('set_cleanable_approval', { id, approved });
}

export function approveStructural(): Promise<number> {
  return call<number>('approve_structural');
}

export function forgetCleanable(id: string): Promise<boolean> {
  return call('forget_cleanable', { id });
}

export function cleanPaths(paths: string[]): Promise<DeleteResultItem[]> {
  return call('clean_paths', { paths });
}

export function monitorStatus(): Promise<MonitorStatus> {
  return call<MonitorStatus>('monitor_status');
}

export function startMonitor(): Promise<void> {
  return call('start_monitor');
}

export function stopMonitor(): Promise<void> {
  return call('stop_monitor');
}

export function setAutoCleanMode(mode: 'off' | 'notify' | 'auto'): Promise<void> {
  return call('set_auto_clean_mode', { mode });
}

export function setScheduledCleanup(enabled: boolean, hour: number): Promise<void> {
  return call('set_scheduled_cleanup', { enabled, hour });
}

export function markPath(path: string): Promise<import('./types').Finding> {
  return call('mark_path', { path });
}

export function takeStoreWarnings(): Promise<StoreWarning[]> {
  return call<StoreWarning[]>('take_store_warnings');
}

export function storeLocation(): Promise<string> {
  return call<string>('store_location');
}

export function routineSuggestions(): Promise<RoutineSuggestion[]> {
  return call<RoutineSuggestion[]>('routine_suggestions');
}

// ---- cleanup history -------------------------------------------------------

export function listHistory(): Promise<import('./types').HistoryEntry[]> {
  return call('list_history');
}

// ---- saved routines --------------------------------------------------------

export function listRoutines(): Promise<Routine[]> {
  return call<Routine[]>('list_routines');
}

export function acceptRoutineSuggestion(name: string, kind: string): Promise<void> {
  return call('accept_routine_suggestion', { name, kind });
}

export function saveRoutine(
  title: string,
  kind: string,
  averageBytes: number,
  path?: string
): Promise<void> {
  return call('save_routine', { title, kind, averageBytes, path: path ?? null });
}

export function dismissRoutineSuggestion(name: string, kind: string): Promise<void> {
  return call('dismiss_routine_suggestion', { name, kind });
}

export function deleteRoutine(id: string): Promise<boolean> {
  return call<boolean>('delete_routine', { id });
}

export function toggleRoutineMode(id: string): Promise<boolean> {
  return call<boolean>('toggle_routine_mode', { id });
}

export function runRoutine(id: string): Promise<DeleteResultItem[]> {
  return call<DeleteResultItem[]>('run_routine', { id });
}

// ---- interface language ----------------------------------------------------

export function getLanguage(): Promise<'zh' | 'en'> {
  return call<string>('get_language').then((tag) => (tag === 'en' ? 'en' : 'zh'));
}

export function setLanguage(language: 'zh' | 'en'): Promise<void> {
  return call('set_language', { language });
}

// ---- AI provider configuration ----------------------------------------------

export function getAiConfig(): Promise<AiConfig> {
  return call<AiConfig>('get_ai_config');
}

/**
 * Persist the AI provider settings. `token` semantics:
 * `undefined` keeps the stored secret, `''` clears it, a value replaces it.
 */
export function saveAiConfig(
  config: Omit<AiConfig, 'hasToken' | 'batchSize'>,
  token?: string,
): Promise<AiConfig> {
  return call<AiConfig>('save_ai_config', {
    config: {
      enabled: config.enabled,
      provider: config.provider,
      endpoint: config.endpoint,
      model: config.model,
      language: config.language,
      token: token === undefined ? null : token
    }
  });
}

// ---- streamed events --------------------------------------------------------

export interface ProgressInfo {
  files: number;
  dirs: number;
  bytes: number;
  totalBytes: number;
  percent: number;
  rateBytesPerSec: number;
  awaiting: number;
  trackedNodes: number;
}

export interface PermissionRequest {
  id: string;
  path: string;
  name: string;
  /** true — TCC denial, grantable via the panel; false — needs an admin. */
  tcc: boolean;
}

export interface BatchUpdate {
  epoch: number;
  discovered: Node[];
  sized: {
    id: string;
    size: number | null;
    pending: boolean;
    status: Node['status'];
  }[];
  permissions: PermissionRequest[];
  progress?: ProgressInfo;
}

export interface ScanDoneEvent {
  epoch: number;
  cancelled: boolean;
  root: string;
}

export interface RefreshedEvent {
  epoch: number;
  path: string;
}

export interface DeletedEvent {
  id: string;
  path: string;
}

export function onScanBatch(cb: (e: BatchUpdate) => void): Promise<UnlistenFn> {
  return subscribe<BatchUpdate>('scan://batch', cb);
}

export function onScanDone(cb: (e: ScanDoneEvent) => void): Promise<UnlistenFn> {
  return subscribe<ScanDoneEvent>('scan://done', cb);
}

export function onScanRefreshed(cb: (e: RefreshedEvent) => void): Promise<UnlistenFn> {
  return subscribe<RefreshedEvent>('scan://refreshed', cb);
}

export function onFsDeleted(cb: (e: DeletedEvent) => void): Promise<UnlistenFn> {
  return subscribe<DeletedEvent>('fs://deleted', cb);
}

export function onMonitorEvent(cb: (e: unknown) => void): Promise<UnlistenFn> {
  return subscribe<unknown>('monitor://event', cb);
}
