import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { Node, VolumeInfo } from './types';

// ---- commands --------------------------------------------------------------

export function listVolumes(): Promise<VolumeInfo[]> {
  return invoke<VolumeInfo[]>('list_volumes');
}

export function homeDir(): Promise<string | null> {
  return invoke<string | null>('home_dir');
}

export function startScan(root: string, focus: string, label: string): Promise<void> {
  return invoke('start_scan', { root, focus, label });
}

export function cancelScan(): Promise<void> {
  return invoke('cancel_scan');
}

export function setScanFocus(focus: string): Promise<void> {
  return invoke('set_scan_focus', { focus });
}

export interface DeleteResultItem {
  path: string;
  ok: boolean;
  error: string | null;
  dryRun: boolean;
}

/// Preview what cleanup would do; never touches the filesystem.
export function previewDelete(paths: string[]): Promise<DeleteResultItem[]> {
  return invoke('preview_delete', { paths });
}

/// Real cleanup only when `execute` is true.
export function moveToTrash(paths: string[], execute = false): Promise<DeleteResultItem[]> {
  return invoke('move_to_trash', { paths, execute });
}

export function watchFs(root: string): Promise<void> {
  return invoke('watch_fs', { root });
}

// ---- streamed events -------------------------------------------------------

export interface SizedEvent {
  id: string;
  size: number;
  pending: boolean;
}

export interface ProgressEvent {
  files: number;
  dirs: number;
  trackedNodes: number;
  bytes: number;
  denied: number;
  queueDepth: number;
  elapsedSecs: number;
  entriesPerSec: number;
  bytesPerSec: number;
  coverage: number;
}

export interface ScanDoneEvent {
  cancelled: boolean;
  root: string;
}

export function onDiscovered(cb: (node: Node) => void): Promise<UnlistenFn> {
  return listen<Node>('scan://discovered', (e) => cb(e.payload));
}

export function onSized(cb: (e: SizedEvent) => void): Promise<UnlistenFn> {
  return listen<SizedEvent>('scan://sized', (e) => cb(e.payload));
}

export function onProgress(cb: (e: ProgressEvent) => void): Promise<UnlistenFn> {
  return listen<ProgressEvent>('scan://progress', (e) => cb(e.payload));
}

export function onScanDone(cb: (e: ScanDoneEvent) => void): Promise<UnlistenFn> {
  return listen<ScanDoneEvent>('scan://done', (e) => cb(e.payload));
}

export interface CalibratedEvent {
  id: string;
  size: number;
  files: number;
}

export function onCalibrated(cb: (e: CalibratedEvent) => void): Promise<UnlistenFn> {
  return listen<CalibratedEvent>('scan://calibrated', (e) => cb(e.payload));
}

export function onCalibrationStart(cb: (e: { message: string }) => void): Promise<UnlistenFn> {
  return listen<{ message: string }>('scan://calibration-start', (e) => cb(e.payload));
}

export function onCalibrationDone(cb: () => void): Promise<UnlistenFn> {
  return listen('scan://calibration-done', () => cb());
}

export interface DeletedEvent {
  id: string;
  path: string;
}

export function onFsDeleted(cb: (e: DeletedEvent) => void): Promise<UnlistenFn> {
  return listen<DeletedEvent>('fs://deleted', (e) => cb(e.payload));
}
