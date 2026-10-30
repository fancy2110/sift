import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { isTauri } from './runtime';
import type {
  AiConfig,
  AiRoutineDraft,
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
//
// The mock is dev-only simulation (~900 lines). It is imported lazily so the
// production initial chunk — which never routes here, because
// `__TAURI_INTERNALS__` is always present in the webview — does not pay its
// parse/compile cost during cold start.

let mockBusPromise: Promise<import('./mock/events').EventBus> | null = null;
let mockBackendPromise: Promise<import('./mock/backend').MockBackend> | null = null;

function ensureBus(): Promise<import('./mock/events').EventBus> {
  mockBusPromise ??= import('./mock/events').then(({ createEventBus }) => createEventBus());
  return mockBusPromise;
}

function ensureMock(): Promise<import('./mock/backend').MockBackend> {
  mockBackendPromise ??= ensureBus().then((bus) =>
    import('./mock/backend').then(({ createMockBackend }) => createMockBackend(bus)),
  );
  return mockBackendPromise;
}

function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) return invoke<T>(command, args);
  return ensureMock().then((mock) => mock.invoke<T>(command, args));
}

async function subscribe<T>(event: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  if (isTauri()) {
    return listen<T>(event, (e) => cb(e.payload));
  }
  const bus = await ensureBus();
  return bus.listen<T>(event, cb) as unknown as UnlistenFn;
}

// ---- commands: volumes & scanning ------------------------------------------

export function listVolumes(): Promise<VolumeInfo[]> {
  return call<VolumeInfo[]>('list_volumes');
}

/** The user's home directory, or null when the platform cannot tell. */
export function homeDir(): Promise<string | null> {
  return call<string | null>('home_dir');
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

export interface CleanupRule {
  id: string;
  name: string;
  dir: string;
  pattern: string;
  olderThanDays: number;
  enabled: boolean;
}

export interface RulePreview {
  matched: number;
  bytes: number;
  truncated: boolean;
  examples: string[];
}

export interface RuleRun {
  matched: number;
  removed: number;
  failed: number;
  bytes: number;
  truncated: boolean;
  results: DeleteResultItem[];
}

export interface DeleteResultItem {
  path: string;
  ok: boolean;
  error: string | null;
  /** True for items never attempted because the user cancelled the batch. */
  cancelled?: boolean;
}

export function watchFs(root: string, recursive: boolean): Promise<void> {
  return call('watch_fs', { root, recursive });
}

export function unwatchFs(): Promise<void> {
  return call('unwatch_fs');
}

// ---- commands: analysis, persistence & monitoring ---------------------------

/** Free-form question to the configured model (assistant tab). */
export function aiPrompt(prompt: string): Promise<string> {
  return call<string>('ai_prompt', { prompt });
}

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

/** Remember (or clear) "stop recommending this path" after a queue removal. */
export function setFindingDismissed(path: string, dismissed: boolean): Promise<void> {
  return call('set_finding_dismissed', { path, dismissed });
}

export function listCleanupRules(): Promise<CleanupRule[]> {
  return call('list_cleanup_rules');
}

export function saveCleanupRule(rule: CleanupRule): Promise<CleanupRule> {
  return call('save_cleanup_rule', { rule });
}

export function deleteCleanupRule(id: string): Promise<boolean> {
  return call('delete_cleanup_rule', { id });
}

export function previewCleanupRule(rule: CleanupRule): Promise<RulePreview> {
  return call('preview_cleanup_rule', { rule });
}

export function runCleanupRule(id: string): Promise<RuleRun> {
  return call('run_cleanup_rule', { id });
}

export function cleanPaths(paths: string[]): Promise<DeleteResultItem[]> {
  return call('clean_paths', { paths });
}

/** Ask a running interactive cleanup to stop after its current item. */
export function cancelCleanup(): Promise<boolean> {
  return call<boolean>('cancel_cleanup');
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

export function markPath(path: string, sizeHint?: number): Promise<import('./types').Finding> {
  return call('mark_path', { path, bytes: sizeHint ?? null });
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

/// Keep in sync with `LOCALES` in i18n.svelte.ts and `set_language` in Rust.
export type LanguageTag = 'zh' | 'zh-TW' | 'en' | 'ja' | 'ko' | 'hi' | 'id' | 'ru';

const LANGUAGE_TAGS: readonly LanguageTag[] = [
  'zh',
  'zh-TW',
  'en',
  'ja',
  'ko',
  'hi',
  'id',
  'ru',
];

export function getLanguage(): Promise<LanguageTag> {
  return call<string>('get_language').then((tag) =>
    LANGUAGE_TAGS.includes(tag as LanguageTag) ? (tag as LanguageTag) : 'zh'
  );
}

export function setLanguage(language: LanguageTag): Promise<void> {
  return call('set_language', { language });
}

// ---- AI provider configuration ----------------------------------------------

export function getAiConfig(): Promise<AiConfig> {
  return call<AiConfig>('get_ai_config');
}

export interface AiProfileDraft {
  id: string | null;
  name: string;
  provider: string;
  endpoint: string;
  model: string;
}

/**
 * Create or update one provider configuration. `token` semantics:
 * `undefined` keeps the stored secret, `''` clears it, a value replaces it.
 */
export function saveAiProfile(draft: AiProfileDraft, token?: string): Promise<AiConfig> {
  return call<AiConfig>('save_ai_profile', {
    profile: { ...draft, token: token === undefined ? null : token }
  });
}

/** Ask the model for a routine draft from a sentence. */
export function aiRoutine(prompt: string): Promise<AiRoutineDraft> {
  return call<AiRoutineDraft>('ai_routine', { prompt });
}

/** Save a confirmed draft (manual or AI); always starts in approve mode. */
export function saveRoutineDraft(draft: {
  title: string;
  cadence: string;
  paths: string[];
}): Promise<void> {
  return call<void>('save_routine_draft', {
    title: draft.title,
    cadence: draft.cadence,
    paths: draft.paths
  });
}

export function selectAiProfile(id: string): Promise<AiConfig> {
  return call<AiConfig>('select_ai_profile', { id });
}

export function deleteAiProfile(id: string): Promise<AiConfig> {
  return call<AiConfig>('delete_ai_profile', { id });
}

export function setAiEnabled(enabled: boolean): Promise<AiConfig> {
  return call<AiConfig>('set_ai_enabled', { enabled });
}

export function setAiLanguage(language: string): Promise<AiConfig> {
  return call<AiConfig>('set_ai_language', { language });
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

/** Emitted after the daily scheduled analysis pass (nothing is deleted). */
export function onScheduledReady(
  cb: (e: { pending: number; ruleMatches: number }) => void
): Promise<UnlistenFn> {
  return subscribe('scheduled://ready', cb);
}

export function onMonitorEvent(cb: (e: unknown) => void): Promise<UnlistenFn> {
  return subscribe<unknown>('monitor://event', cb);
}
