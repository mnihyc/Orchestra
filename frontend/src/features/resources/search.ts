/** What the address of a list of resources says: the words searched for and the one filter, read so that whatever is not one of the things it may say is left out.
 *
 * Each list's filter and the values it may take are here, not in the list's screen: its route reads them before the
 * screen's code is fetched.
 */
import { matches as matchesQuery } from '@/features/finder/ranking';
import type { ProviderListScope, ProviderStatus, UnitKind } from '@/generated/api-types';

/** The one of `allowed` that `value` is, or nothing. */
export function oneOf<Value extends string>(allowed: readonly Value[], value: unknown): Value | undefined {
  return allowed.find((candidate) => candidate === value);
}

/** What was searched for, as the address gives it: text, and never an empty one. */
export function readText(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

/**
 * Whether every word of `query` is somewhere in one of the `fields`, without regard to case. A search
 * for two words finds what has both, wherever each lies.
 */
export function matches(query: string, ...fields: readonly (string | null | undefined)[]): boolean {
  return matchesQuery(query, ...fields);
}

/** Whose providers the list shows. */
export type ProviderView = ProviderListScope;

export const PROVIDER_VIEWS: readonly ProviderView[] = ['mine', 'managed', 'usable', 'viewable', 'all'];
export const PROVIDER_STATUSES: readonly ProviderStatus[] = ['ready', 'needs_setup', 'exhausted', 'offline', 'error', 'disabled'];

export interface ProvidersSearch {
  q?: string;
  view?: ProviderView;
  status?: ProviderStatus;
}

export const TEMPLATE_KINDS: readonly UnitKind[] = ['environment', 'provider'];

export interface TemplatesSearch {
  q?: string;
  kind?: UnitKind;
}

/** Whether a host takes new environments. */
export type HostState = 'open' | 'closed';

export const HOST_STATES: readonly HostState[] = ['open', 'closed'];

export interface HostsSearch {
  q?: string;
  state?: HostState;
}
