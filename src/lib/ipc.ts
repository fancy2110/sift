import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type { FileNode, VolumeInfo } from './types';

// ---- commands --------------------------------------------------------------

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

export interface DeleteResultItem {
  path: string;
  ok: boolean;
  error: string | null;
}

export function moveToTrash(paths: string[]): Promise<DeleteResultItem[]> {
  return invoke('move_to_trash', { paths });
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
}

export interface ScanDoneEvent {
  cancelled: boolean;
  root: string;
}

export function onDiscovered(cb: (node: FileNode) => void): Promise<UnlistenFn> {
  return listen<FileNode>('scan://discovered', (e) => cb(e.payload));
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

export interface DeletedEvent {
  id: string;
  path: string;
}

export function onFsDeleted(cb: (e: DeletedEvent) => void): Promise<UnlistenFn> {
  return listen<DeletedEvent>('fs://deleted', (e) => cb(e.payload));
}
