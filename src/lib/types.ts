export type Risk = 'safe' | 'review' | 'keep';
export type AutoMode = 'approve' | 'auto';

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
  /** Totals are an estimate pending background calibration. */
  estimated?: boolean;

  // ---- client-assembled fields ----
  /** Direct children, attached as they stream in. */
  children?: Node[];
  /** Aggregated AI finding this node belongs to. */
  insightId?: string;
  risk?: Risk;
  note?: string;
  ext?: string;
}

export interface Finding {
  id: string;
  title: string;
  /** One-line AI conclusion: why this is here and what happens on clean. */
  reason: string;
  size: number;
  path: string;
  risk: Risk;
  confidence: number; // 0..1
  /** learned from the user's repeated decisions */
  learned?: boolean;
  /** added ad-hoc by the user via the context menu */
  manual?: boolean;
}

export interface Routine {
  id: string;
  title: string;
  cadence: string;
  avgSize: number;
  autoMode: AutoMode;
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
