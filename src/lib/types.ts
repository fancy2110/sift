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

  // ---- client-assembled fields ----
  /** AI finding id this node belongs to. */
  insightId?: string;
  risk?: Risk;
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
export interface AiConfig {
  enabled: boolean;
  provider: string;
  endpoint: string;
  model: string;
  language: string;
  batchSize: number;
  /** Whether a token is stored in the Keychain / environment. */
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
