const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

export function formatSize(bytes: number, digits = 1): string {
  const { value, unit } = formatSizeParts(bytes, digits);
  return `${value} ${unit}`;
}

export function formatSizeParts(
  bytes: number,
  digits = 1
): { value: string; unit: string } {
  if (bytes <= 0) return { value: '0', unit: 'B' };
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), UNITS.length - 1);
  const v = bytes / 1024 ** i;
  const d = i === 0 ? 0 : digits;
  return { value: v.toFixed(d), unit: UNITS[i] };
}

export function formatPercent(ratio: number): string {
  return `${(ratio * 100).toFixed(ratio >= 0.1 ? 0 : 1)}%`;
}
