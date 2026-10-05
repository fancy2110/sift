import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  AiConfig,
  AnalysisSummary,
  MonitorStatus,
  Node,
  Place,
  Routine,
  RoutineSuggestion,
  StoreWarning,
  VolumeInfo
} from './types';

// ---- commands: volumes & scanning ------------------------------------------

export function listVolumes(): Promise<VolumeInfo[]> {
  return invoke<VolumeInfo[]>('list_volumes');
}

export function startScan(root: string, focus: string): Promise<number> {
  return invoke<number>('start_scan', { root, focus });
}

export function cancelScan(): Promise<void> {
  return invoke('cancel_scan');
}

export function setScanFocus(focus: string): Promise<void> {
  return invoke('set_scan_focus', { focus });
}

export function scanRunning(): Promise<number | null> {
  return invoke<number | null>('scan_running');
}

/**
 * Live snapshot of a folder's immediate entries. The front end keeps only
 * directories from the scan; file entries are fetched on demand when the
 * explorer opens a folder.
 */
export function listDirFiles(path: string): Promise<Node[]> {
  return invoke<Node[]>('list_dir_files', { path });
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
  return invoke('resolve_permission', { id, granted, grantedPath: grantedPath ?? null });
}

// ---- commands: deletion & filesystem awareness -----------------------------

export interface DeleteResultItem {
  path: string;
  ok: boolean;
  error: string | null;
}

export function watchFs(root: string, recursive: boolean): Promise<void> {
  return invoke('watch_fs', { root, recursive });
}

export function unwatchFs(): Promise<void> {
  return invoke('unwatch_fs');
}

// ---- commands: analysis, persistence & monitoring ---------------------------

export function analyzeCurrent(): Promise<AnalysisSummary> {
  return invoke<AnalysisSummary>('analyze_current');
}

export function listCleanable(): Promise<import('./types').Finding[]> {
  return invoke('list_cleanable');
}

export function setCleanableApproval(id: string, approved: boolean): Promise<boolean> {
  return invoke('set_cleanable_approval', { id, approved });
}

export function approveStructural(): Promise<number> {
  return invoke<number>('approve_structural');
}

export function forgetCleanable(id: string): Promise<boolean> {
  return invoke('forget_cleanable', { id });
}

export function cleanPaths(paths: string[]): Promise<DeleteResultItem[]> {
  return invoke('clean_paths', { paths });
}

export function monitorStatus(): Promise<MonitorStatus> {
  return invoke<MonitorStatus>('monitor_status');
}

export function startMonitor(): Promise<void> {
  return invoke('start_monitor');
}

export function stopMonitor(): Promise<void> {
  return invoke('stop_monitor');
}

export function setAutoCleanMode(mode: 'off' | 'notify' | 'auto'): Promise<void> {
  return invoke('set_auto_clean_mode', { mode });
}

export function setScheduledCleanup(enabled: boolean, hour: number): Promise<void> {
  return invoke('set_scheduled_cleanup', { enabled, hour });
}

export function markPath(path: string): Promise<import('./types').Finding> {
  return invoke('mark_path', { path });
}

export function takeStoreWarnings(): Promise<StoreWarning[]> {
  return invoke<StoreWarning[]>('take_store_warnings');
}

export function storeLocation(): Promise<string> {
  return invoke<string>('store_location');
}

export function routineSuggestions(): Promise<RoutineSuggestion[]> {
  return invoke<RoutineSuggestion[]>('routine_suggestions');
}

// ---- quick places ----------------------------------------------------------

export function listPlaces(): Promise<Place[]> {
  return invoke<Place[]>('list_places');
}

// ---- cleanup history -------------------------------------------------------

export function listHistory(): Promise<import('./types').HistoryEntry[]> {
  return invoke('list_history');
}

// ---- saved routines --------------------------------------------------------

export function listRoutines(): Promise<Routine[]> {
  return invoke<Routine[]>('list_routines');
}

export function acceptRoutineSuggestion(name: string, kind: string): Promise<void> {
  return invoke('accept_routine_suggestion', { name, kind });
}

export function dismissRoutineSuggestion(name: string, kind: string): Promise<void> {
  return invoke('dismiss_routine_suggestion', { name, kind });
}

export function deleteRoutine(id: string): Promise<boolean> {
  return invoke<boolean>('delete_routine', { id });
}

export function toggleRoutineMode(id: string): Promise<boolean> {
  return invoke<boolean>('toggle_routine_mode', { id });
}

export function runRoutine(id: string): Promise<DeleteResultItem[]> {
  return invoke<DeleteResultItem[]>('run_routine', { id });
}

// ---- interface language ----------------------------------------------------

export function getLanguage(): Promise<'zh' | 'en'> {
  return invoke<string>('get_language').then((tag) => (tag === 'en' ? 'en' : 'zh'));
}

export function setLanguage(language: 'zh' | 'en'): Promise<void> {
  return invoke('set_language', { language });
}

// ---- AI provider configuration ----------------------------------------------

export function getAiConfig(): Promise<AiConfig> {
  return invoke<AiConfig>('get_ai_config');
}

/**
 * Persist the AI provider settings. `token` semantics:
 * `undefined` keeps the stored secret, `''` clears it, a value replaces it.
 */
export function saveAiConfig(
  config: Omit<AiConfig, 'hasToken' | 'batchSize'>,
  token?: string,
): Promise<AiConfig> {
  return invoke<AiConfig>('save_ai_config', {
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
  return listen<BatchUpdate>('scan://batch', (e) => cb(e.payload));
}

export function onScanDone(cb: (e: ScanDoneEvent) => void): Promise<UnlistenFn> {
  return listen<ScanDoneEvent>('scan://done', (e) => cb(e.payload));
}

export function onScanRefreshed(cb: (e: RefreshedEvent) => void): Promise<UnlistenFn> {
  return listen<RefreshedEvent>('scan://refreshed', (e) => cb(e.payload));
}

export function onFsDeleted(cb: (e: DeletedEvent) => void): Promise<UnlistenFn> {
  return listen<DeletedEvent>('fs://deleted', (e) => cb(e.payload));
}

export function onMonitorEvent(cb: (e: unknown) => void): Promise<UnlistenFn> {
  return listen('monitor://event', (e) => cb(e.payload));
}
