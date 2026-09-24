import type { FileNode, Insight, Routine, ScanLocation, Volume } from './types';

const GB = 1024 ** 3;

export const volumes: Volume[] = [
  { id: 'loc-disk', name: 'Macintosh HD', capacity: 1000 * GB, used: 812 * GB },
  { id: 'loc-backup-disk', name: 'Backup Plus', capacity: 2000 * GB, used: 1400 * GB, external: true }
];

export const scanLocations: ScanLocation[] = [
  // physical disks
  { id: 'loc-disk', name: 'Macintosh HD', path: '/', icon: 'drive', group: 'disk', diskId: 'loc-disk' },
  {
    id: 'loc-backup-disk',
    name: 'Backup Plus',
    path: '/Volumes/Backup Plus',
    icon: 'externalDrive',
    group: 'disk',
    diskId: 'loc-backup-disk'
  },
  // quick places on the internal disk
  { id: 'loc-user', name: '用户文件夹', path: '~', icon: 'home', group: 'places', diskId: 'loc-disk', subtreePath: ['用户'] },
  { id: 'loc-downloads', name: '下载', path: '~/Downloads', icon: 'download', group: 'places', diskId: 'loc-disk', subtreePath: ['用户', '下载'] },
  { id: 'loc-desktop', name: '桌面', path: '~/Desktop', icon: 'desktop', group: 'places', diskId: 'loc-disk', subtreePath: ['用户', '桌面'] },
  { id: 'loc-movies', name: '影片', path: '~/Movies', icon: 'film', group: 'places', diskId: 'loc-disk', subtreePath: ['用户', '影片'] },
  { id: 'loc-trash', name: '废纸篓', path: '~/.Trash', icon: 'trash', group: 'places', diskId: 'loc-disk', subtreePath: ['废纸篓'] }
];

export const insights: Insight[] = [
  {
    id: 'ins-derived-data',
    title: 'Xcode DerivedData 构建缓存',
    reason: '可重新生成的索引与中间产物，其中 3 个项目 90 天未打开，对应 14.6 GB 优先清理。',
    size: 18.4 * GB,
    path: '~/Library/Developer/Xcode/DerivedData',
    risk: 'safe',
    confidence: 0.98,
    locId: 'loc-user'
  },
  {
    id: 'ins-package-caches',
    title: 'Homebrew / npm / Cargo 缓存',
    reason: '包管理器下载缓存，依赖已在项目中，可随时重新拉取（保留近 30 天）。',
    size: 7.6 * GB,
    path: '~/Library/Caches',
    risk: 'safe',
    confidence: 0.96,
    locId: 'loc-user'
  },
  {
    id: 'ins-download-dmg',
    title: '下载目录中的安装镜像',
    reason: '12 个镜像对应 App 已安装；你连续 12 周扫描后都会删除，已沉淀为习惯。',
    size: 6.2 * GB,
    path: '~/Downloads',
    risk: 'safe',
    confidence: 0.95,
    learned: true,
    locId: 'loc-downloads'
  },
  {
    id: 'ins-trash',
    title: '废纸篓',
    reason: '87% 的项目已放入废纸篓超过 30 天，包含 2.9 GB 旧视频导出。',
    size: 3.1 * GB,
    path: '~/.Trash',
    risk: 'safe',
    confidence: 0.99,
    locId: 'loc-trash'
  },
  {
    id: 'ins-dup-video',
    title: '重复的视频导出',
    reason: '与保留版本逐字节一致（SHA-256），建议每组仅保留最早一份。',
    size: 12.7 * GB,
    path: '~/Movies/Exports',
    risk: 'review',
    confidence:0.91,
    locId: 'loc-movies'
  },
  {
    id: 'ins-ios-backup',
    title: '旧 iPad 本地备份',
    reason: '设备已改用 iCloud，本地备份停留 14 个月前，此后未再接入这台 Mac。',
    size: 9.8 * GB,
    path: '~/Library/Application Support/MobileSync/Backup',
    risk: 'review',
    confidence: 0.88,
    locId: 'loc-user'
  },
  {
    id: 'ins-stale-archive',
    title: '长期未打开的项目归档',
    reason: '11 个项目超 18 个月未打开，其中 2 个 GitHub 有完整远端，建议先确认再删。',
    size: 24.0 * GB,
    path: '~/Desktop/_archive',
    risk: 'review',
    confidence: 0.78,
    locId: 'loc-desktop'
  },
  {
    id: 'ins-vm-image',
    title: 'Windows 11 ARM 虚拟机',
    reason: '近 90 天启动过 4 次 Parallels 且无其他备份，建议保留、不列入清理。',
    size: 62.0 * GB,
    path: '~/Parallels/Windows 11.pvm',
    risk: 'keep',
    confidence: 0.92,
    locId: 'loc-user'
  },

  // ---- external backup disk findings ----
  {
    id: 'ins-bkp-tm',
    title: '过期的 Time Machine 旧备份',
    reason: '12 个月以前的历史快照，关键数据已迁移至 iCloud，保留近期快照即可。',
    size: 180 * GB,
    path: '/Volumes/Backup Plus/Backups.backupdb',
    risk: 'review',
    confidence: 0.82,
    locId: 'loc-backup-disk'
  },
  {
    id: 'ins-bkp-archive',
    title: '旧项目归档（2019–2022）',
    reason: '220 GB 归档中 70% 的项目在 GitHub 有完整远端，确认后可只保留无远端的部分。',
    size: 220 * GB,
    path: '/Volumes/Backup Plus/Old Projects',
    risk: 'review',
    confidence: 0.74,
    locId: 'loc-backup-disk'
  },
  {
    id: 'ins-bkp-installers',
    title: '安装镜像与归档压缩包',
    reason: '46 GB 旧镜像 / 压缩包，对应软件均已更新多个大版本，可从官网重新获取。',
    size: 46 * GB,
    path: '/Volumes/Backup Plus/Installers',
    risk: 'safe',
    confidence: 0.93,
    locId: 'loc-backup-disk'
  },
  {
    id: 'ins-bkp-dup',
    title: '重复的照片 / 视频导出',
    reason: '与内盘照片图库内容重复的导出副本，逐字节比对一致，建议仅保留图库原件。',
    size: 84 * GB,
    path: '/Volumes/Backup Plus/Photo Exports',
    risk: 'review',
    confidence: 0.86,
    locId: 'loc-backup-disk'
  }
];

export const routines: Routine[] = [
  {
    id: 'rt-downloads',
    title: '下载文件夹整理',
    cadence: '每周五 18:00',
    avgSize: 5.6 * GB,
    autoMode: 'auto'
  },
  {
    id: 'rt-cache',
    title: '开发缓存清理',
    cadence: '每周一 09:00',
    avgSize: 11.8 * GB,
    autoMode: 'approve'
  },
  {
    id: 'rt-space-guard',
    title: '空间守卫',
    cadence: '可用空间低于 15% 时',
    avgSize: 8.4 * GB,
    autoMode: 'auto'
  }
];

const internalTree: FileNode = {
  name: 'Macintosh HD',
  size: 812 * GB,
  children: [
    {
      name: '用户',
      size: 598 * GB,
      children: [
        {
          name: '开发工具',
          size: 45 * GB,
          children: [
            { name: 'DerivedData', size: 18.4 * GB, note: '可重建缓存', insightId: 'ins-derived-data', risk: 'safe' },
            { name: 'iOS 设备支持文件', size: 12 * GB },
            { name: '归档 Archives', size: 8.6 * GB },
            { name: '模拟器数据', size: 6 * GB }
          ]
        },
        { name: '照片图库', size: 52 * GB },
        { name: '虚拟机', size: 62 * GB, note: '近期在用，建议保留', insightId: 'ins-vm-image', risk: 'keep' },
        {
          name: '影片',
          size: 34 * GB,
          children: [
            { name: '重复的视频导出', size: 12.7 * GB, note: '逐字节重复', insightId: 'ins-dup-video', risk: 'review' },
            { name: '其他影片', size: 21.3 * GB }
          ]
        },
        { name: '下载', size: 31 * GB, note: '安装镜像可清理', insightId: 'ins-download-dmg', risk: 'safe',
          children: [
            { name: '安装镜像', size: 6.2 * GB },
            { name: '文档与压缩包', size: 14 * GB },
            { name: '其他', size: 10.8 * GB }
          ] },
        { name: '桌面', size: 26 * GB, note: '旧归档待确认', insightId: 'ins-stale-archive', risk: 'review',
          children: [
            { name: '_archive 归档', size: 24 * GB },
            { name: '散落文件', size: 2 * GB }
          ] },
        { name: '文稿', size: 44 * GB },
        { name: '音乐', size: 14 * GB },
        {
          name: '应用支持数据',
          size: 48 * GB,
          children: [
            { name: 'MobileSync 备份', size: 10 * GB, note: '旧 iPad 备份', insightId: 'ins-ios-backup', risk: 'review' },
            { name: '应用容器', size: 23 * GB },
            { name: '其他', size: 15 * GB }
          ]
        },
        {
          name: '缓存',
          size: 22 * GB,
          children: [
            { name: '应用缓存', size: 9.2 * GB },
            { name: '包管理器缓存', size: 7.6 * GB, note: '可安全清理', insightId: 'ins-package-caches', risk: 'safe' },
            { name: '其他', size: 5.2 * GB }
          ]
        },
        { name: '其他用户目录', size: 20 * GB }
      ]
    },
    { name: '应用程序', size: 62 * GB, deletable: false },
    { name: '系统', size: 28 * GB, deletable: false },
    { name: '系统数据', size: 106 * GB, deletable: false },
    { name: '资源库', size: 15 * GB, deletable: false },
    { name: '废纸篓', size: 3.1 * GB, note: '87% 已放 30 天以上', insightId: 'ins-trash', risk: 'safe',
      children: [
        { name: '旧视频导出', size: 2.9 * GB },
        { name: '其他', size: 0.2 * GB }
      ] }
  ]
};

const backupTree: FileNode = {
  name: 'Backup Plus',
  size: 1400 * GB,
  children: [
    {
      name: '旧 Time Machine 备份',
      size: 180 * GB,
      note: '过期历史快照',
      insightId: 'ins-bkp-tm',
      risk: 'review'
    },
    {
      name: '旧项目归档',
      size: 220 * GB,
      note: '多数有远端',
      insightId: 'ins-bkp-archive',
      risk: 'review'
    },
    {
      name: '安装镜像与压缩包',
      size: 46 * GB,
      note: '版本已过时',
      insightId: 'ins-bkp-installers',
      risk: 'safe'
    },
    {
      name: '重复照片导出',
      size: 84 * GB,
      note: '与图库重复',
      insightId: 'ins-bkp-dup',
      risk: 'review'
    },
    { name: '照片与视频资料库', size: 520 * GB },
    { name: '文档与其他备份', size: 350 * GB }
  ]
};

/** Per-disk file trees, keyed by volume id. */
export const fileTrees: Record<string, FileNode> = {
  'loc-disk': internalTree,
  'loc-backup-disk': backupTree
};

export const scanStages = [
  '读取文件系统元数据',
  '计算目录大小',
  '识别缓存 / 安装包 / 残留',
  '比对重复文件',
  '结合使用习惯生成结论'
];

export const demoSizes = { GB };
