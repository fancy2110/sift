import type { Finding } from './types';
import { t } from './i18n.svelte';

/** Human-readable title for a finding's candidate family. */
export function kindTitle(finding: Finding): string {
  return t(`kind.${finding.kind}`, undefined, finding.name);
}

/** Render a finding's reason key (or model text) into readable copy. */
export function reasonText(finding: Finding): string {
  if (finding.reasonKind === 'text') return finding.reason;
  return t(finding.reason, finding.reasonParams, finding.reason);
}

/** Render a finding's deletion-impact key (or model text) into readable copy. */
export function impactText(finding: Finding): string {
  if (finding.impactKind === 'text') return finding.impact;
  return t(finding.impact, finding.impactParams, finding.impact);
}

/** Short label for how a finding is cleaned. */
export function cleanupMethodText(finding: Finding): string {
  return t(`cleanup.method.${finding.cleanupMethod ?? 'trashItem'}`, undefined, finding.cleanupMethod ?? '');
}

/** The toolchain command for a finding, with its required working directory. */
export function cleanupCommandText(finding: Finding): string | null {
  return finding.cleanupCommand ?? null;
}
