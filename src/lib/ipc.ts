import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
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

export function startScan(root: string, focus: string): Promise<void> {
  return invoke('start_scan', { root, focus });
}

export function cancelScan(): Promise<void> {
  return invoke('cancel_scan');
}

export function setScanFocus(focus: string): Promise<void> {
  return invoke('set_scan_focus', { focus });
}

export function scanRunning(): Promise<boolean> {
  return invoke<boolean>('scan_running');
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

// ---- streamed events --------------------------------------------------------

export interface BatchUpdate {
  discovered: Node[];
  sized: { id: string; size: number; pending: boolean }[];
  progress?: { files: number; dirs: number; trackedNodes: number };
}

export interface ScanDoneEvent {
  cancelled: boolean;
  root: string;
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

export function onFsDeleted(cb: (e: DeletedEvent) => void): Promise<UnlistenFn> {
  return listen<DeletedEvent>('fs://deleted', (e) => cb(e.payload));
}

export function onMonitorEvent(cb: (e: unknown) => void): Promise<UnlistenFn> {
  return listen('monitor://event', (e) => cb(e.payload));
}
