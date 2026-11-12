export type Risk = 'safe' | 'review' | 'keep';

/** Measurement state of a directory node. */
export type NodeStatus = 'ok' | 'estimated' | 'denied' | 'awaiting';

/** A live filesystem node streamed in by the backend scanner. */
export interface Node {
  id: string;
  parentId: string | null;
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modifiedMs: number | null;
  deletable: boolean;
  /** Directory whose subtree scan hasn't finished. */
  pending: boolean;
  status: NodeStatus;
  ext?: string;
}

/** One analyzed candidate, exactly the backend FindingDto shape. */
export interface Finding {
  id: string;
  name: string;
  path: string;
  displayPath: string;
  size: number;
  isDir: boolean;
  safety: Risk;
  confidence: number;
  reason: string;
  reasonKind: 'key' | 'text';
  reasonParams: string[];
  source: string;
  kind: string;
  knownCleanable: boolean;
  approvedForAuto: boolean;
  cleanupCommand?: string | null;
  cleanupMethod?: string;
  impact: string;
  impactKind: 'key' | 'text';
  impactParams: string[];
}

export interface AnalysisSummary {
  findings: Finding[];
  reclaimableBytes: number;
  safeBytes: number;
  knownCleanableBytes: number;
  sourceCounts: Record<string, number>;
  usedRemote: boolean;
  remoteNeedsConsent: boolean;
}

/** AI provider configuration from the backend (never includes the token). */
export interface AiProfile {
  id: string;
  name: string;
  /** ark | deepseek | openai | anthropic | custom */
  provider: string;
  endpoint: string;
  model: string;
  /** Whether this profile has a token (environment or stored). */
  hasToken: boolean;
}

export interface AiConfig {
  enabled: boolean;
  activeProfile: string;
  profiles: AiProfile[];
  language: string;
  batchSize: number;
  /** Extra instructions used when analysing directories. */
  analysisPrompt: string;
  /** Whether the active profile has a token. */
  hasToken: boolean;
}

export interface RoutineSuggestion {
  name: string;
  kind: string;
  occurrences: number;
  distinctDays: number;
  averageBytes: number;
  totalBytes: number;
  destructive: boolean;
  reason: string;
  cadence: string;
}

/** A routine proposed by the model; saved only after the user confirms. */
export interface AiRoutineDraft {
  title: string;
  /** cadence.daily | cadence.weekly | cadence.monthly */
  cadence: string;
  paths: string[];
  reason: string;
  /** Optional file-name regex; empty means every file under the paths. */
  pattern: string;
  /** Only meaningful with a pattern; 0 = any age. */
  olderThanDays: number;
}

/** One finished cleanup session from the backend timeline. */
export interface HistoryEntry {
  id: string;
  atMs: number;
  bytes: number;
  items: number;
  automatic: boolean;
  titles: string[];
}

/** One saved routine. */
export interface Routine {
  id: string;
  title: string;
  kind: string;
  cadence: string;
  averageBytes: number;
  mode: 'auto' | 'approve';
  /** Explicit targets of a path-based routine; empty for a kind-based one. */
  paths: string[];
  /** Disabled routines stay listed but never run. */
  enabled: boolean;
}

export interface MonitorStatus {
  running: boolean;
  enabled: boolean;
  autoMode: 'off' | 'notify' | 'auto';
  cleanableItems: number;
  autoEligibleItems: number;
  autoEligibleBytes: number;
  lastLevel: string;
  lastAvailableBytes: number;
  lastTotalBytes: number;
  scheduledCleanupEnabled: boolean;
  scheduledHour: number;
  scheduledAutoExecute: boolean;
  protectedPaths: string[];
}

export interface VolumeInfo {
  id: string;
  name: string;
  mountPoint: string;
  totalBytes: number;
  availableBytes: number;
  isRemovable: boolean;
  fileSystem: string;
}

/** A load problem reported by the local store. */
export interface StoreWarning {
  key: string;
  what: string;
  reason: ResetReason;
  backup: string | null;
}

export type ResetReason =
  | { kind: 'io'; message: string }
  | { kind: 'jsonParse'; message: string }
  | { kind: 'schemaMismatch'; found: number; expected: number };

// ---- environment inventory -------------------------------------------------

export interface AppFact {
  key: string;
  name: string;
  version: string | null;
  bundleId: string | null;
  installPath: string;
  lastUsedDays: number | null;
  artifactCount: number;
}

export interface PmFact {
  id: string;
  version: string | null;
  detected: boolean;
  roots: string[];
  projects: string[];
  capabilities: string[];
}

export interface ProjectFact {
  root: string;
  pm: string;
  manifest: string;
  lockfile: string | null;
  pmVersion: string | null;
  dirty: boolean;
}

export interface MaintenanceHint {
  id: string;
  subject: string;
  action: string;
  status: 'proposed' | 'validated' | 'rejected' | 'stale';
  source: 'catalog' | 'rule' | 'model' | 'user';
  confidence: number;
  reasonKey: string;
  impactKey: string;
  modelReason?: string;
  requiresAdmin: boolean;
  pmVersion: string | null;
  createdAtMs: number;
  lastSeenMs: number;
}

export interface EnvironmentSummary {
  updatedAtMs: number;
  apps: AppFact[];
  packageManagers: PmFact[];
  projects: ProjectFact[];
  hints: MaintenanceHint[];
}

export interface UninstallArtifact {
  path: string;
  kind: 'bundle' | 'support' | 'cache' | 'preferences' | 'container' | 'groupContainer' | 'savedState' | 'log' | 'launchAgent' | 'other';
  scope: 'user' | 'system' | 'shared' | 'unknown';
  disposition: 'autoSafe' | 'review' | 'keep' | 'forbidden';
  exists: boolean;
  sizeBytes: number | null;
  requiresAdmin: boolean;
  sharedWith?: string[];
  source: string;
}

export interface AppUninstallPlan {
  appKey: string;
  appName: string;
  artifacts: UninstallArtifact[];
  totalBytes: number;
  warnings: string[];
}

export interface UninstallManifestEntry {
  original: string;
  stored: string;
  isDir: boolean;
}

export interface UninstallManifest {
  schemaVersion: number;
  id: string;
  appKey: string;
  appName: string;
  createdAtMs: number;
  quarantineRoot: string;
  entries: UninstallManifestEntry[];
}

export interface RestoreReport {
  restored: string[];
  skipped: string[];
}

export type PmDestructiveness = 'safe' | 'review' | 'danger';

export interface PmActionPlan {
  pmId: string;
  action: string;
  program: string;
  args: string[];
  scope: string;
  requiresAdmin: boolean;
  destructiveness: PmDestructiveness;
  reasonKey: string;
  impactKey: string;
}

export interface PmRunReport {
  pmId: string;
  action: string;
  success: boolean;
  exitCode: number | null;
  durationMs: number;
  stdout: string;
  stderr: string;
}
