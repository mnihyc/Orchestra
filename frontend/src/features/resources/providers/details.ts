/** Provider details follow the API's member states and source declarations, independently of the credential kind. */
import type { Provider } from '@/generated/api-types';

/** The vendor's named window, qualified only when it supplies a different model-family name. */
export function windowName(window: { label: string; model_family?: string | null }): string {
  return window.model_family === null || window.model_family === undefined || window.model_family === '' || window.model_family === window.label ? window.label : `${window.label} (${window.model_family})`;
}

/** The card's fixed reading order; every key is an ordinary facts member. */
export const FACT_MEMBERS = ['identity', 'credential', 'credential_verified', 'plan', 'windows', 'limit_reached', 'ordinary_usage_allowed', 'balances', 'spend', 'extra_usage', 'available_resets'] as const;
export type DetailMember = (typeof FACT_MEMBERS)[number];
export type DetailState = NonNullable<Provider['facts_state']>[DetailMember | 'models'];
export type Balance = Provider['facts']['balances'][number];
export type Spend = Provider['facts']['spend'][number];

/** Only the source's explicit member state decides whether a block is supported, unread or empty. */
export function detailState(provider: Provider, member: DetailMember | 'models'): DetailState {
  return provider.facts_state[member];
}

/** Collections accept the contract's explicit empty and nullable forms without manufacturing a zero amount. */
export function reportedList<T>(value: readonly T[] | null | undefined): readonly T[] { return value ?? []; }

export function factsSource(provider: Provider) { return provider.sources.facts; }
export function modelsSource(provider: Provider) { return provider.sources.models; }
export function spendsResets(provider: Provider): boolean { return factsSource(provider).descriptor?.facts?.spends_resets === true; }

/** No authentication is a declared endpoint option; an absent credential alone never says it is unnecessary. */
export function endpointsNeedNoKey(provider: Pick<Provider, 'upstreams'>): boolean {
  return provider.upstreams.length > 0 && provider.upstreams.every(({ options }) =>
    options !== null && typeof options === 'object' && !Array.isArray(options) && 'auth' in options && options.auth === 'none');
}

/** Decimal text stays text so that a source's precision is not rounded through a JavaScript number. */
export function amountText(value: string | null | undefined, unit: string, unknown: string): string {
  return value === null || value === undefined ? unknown : `${value} ${unit}`;
}
