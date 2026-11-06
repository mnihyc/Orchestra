/** The retained navigation title and current status reference may name one verified Environment. */
import type { DuplicateAllowance } from '../../e2e/layout-rules-dom';

export interface EnvironmentReferenceProof {
  origin: string;
  environment: { id: string; name: string };
  tab: { id: string; url: string; text: string };
  status: { url: string; label: string; text: string };
}

export function environmentReferenceAllowance(proof: EnvironmentReferenceProof): DuplicateAllowance | undefined {
  const { environment, tab, status } = proof;
  if (tab.id === '' || status.label === '' || tab.text !== environment.name || status.text !== environment.name) return undefined;
  try {
    const expected = `/resources/environments/${encodeURIComponent(environment.id)}`;
    if (![tab.url, status.url].every((href) => { const url = new URL(href, proof.origin); return url.origin === proof.origin && url.pathname === expected; })) return undefined;
  } catch { return undefined; }
  return {
    text: environment.name,
    selectors: [`[role="tab"][id=${JSON.stringify(tab.id)}] > [data-view-tab-title]`, `[data-status-place="bar"] [data-status-item="environment"] > button[aria-label=${JSON.stringify(status.label)}] > span.truncate:not([data-resource-hint])`],
    reason: `The retained navigation tab and qualified current status reference both target Environment ${environment.id}; their native page and detail-card links were verified.`,
  };
}
