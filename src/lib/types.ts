export type Risk = 'safe' | 'review' | 'keep';
export type AutoMode = 'approve' | 'auto';

export interface Insight {
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
  /** added ad-hoc by the user via the tile context menu */
  manual?: boolean;
  /** scope this insight belongs to */
  locId: string;
}

export interface Routine {
  id: string;
  title: string;
  cadence: string;
  avgSize: number;
  autoMode: AutoMode;
}

export interface FileNode {
  name: string;
  size: number;
  note?: string;
  children?: FileNode[];
  /** Links this node to an AI insight when it represents an actionable item. */
  insightId?: string;
  /** AI verdict for this node; drives the judgment overlay on the map. */
  risk?: Risk;
  /** File extension (leaf files only). */
  ext?: string;
  /** Last-modified epoch ms (leaf files only). */
  lastModified?: number;
  /** Whether the current user is allowed to delete this item. */
  deletable?: boolean;
}

export type LocationIcon =
  | 'drive'
  | 'externalDrive'
  | 'home'
  | 'download'
  | 'desktop'
  | 'film'
  | 'trash'
  | 'folder';

export interface ScanLocation {
  id: string;
  name: string;
  /** Absolute-ish display path. */
  path: string;
  icon: LocationIcon;
  /** Menu grouping: physical disks vs quick folders. */
  group: 'disk' | 'places';
  /** Owning volume id; drives capacity readout and tree lookup. */
  diskId: string;
  /** Path of the subtree inside the disk tree; undefined = whole tree. */
  subtreePath?: string[];
  /** User-picked folders are generated, not seeded. */
  custom?: boolean;
}

export interface Volume {
  /** Matches the disk-location id. */
  id: string;
  name: string;
  capacity: number;
  used: number;
  external?: boolean;
}
