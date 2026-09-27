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
