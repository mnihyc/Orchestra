/** Actual tier facts, shared by usage rows, filters and performance choices. */
import type { TFunction } from 'i18next';
import { offeredChoice } from '@/i18n/choices';

export interface TierFacts {
  tier?: string | null;
  tier_unknown?: boolean;
}
export interface TierFilter {
  tier?: string | undefined;
  tier_unknown?: boolean | undefined;
}
interface RequestTierFacts extends TierFacts {
  timing?: {
    wire_protocol?: string | null;
    requested_service_tier?: string | null;
    reported_service_tier?: { state: string };
  };
}

export function tierLabel(row: TierFacts, t: TFunction): string {
  if (row.tier_unknown === true) return t('usage.key.actualTierNotReported');
  return row.tier === null || row.tier === undefined ? t('usage.key.standard') : offeredChoice(t, 'serviceTier', row.tier)?.name ?? row.tier;
}

/** This structured UI identity is never a vendor tier or a tier query parameter. */
export function tierId(row: TierFacts): string {
  return JSON.stringify([row.tier ?? null, row.tier_unknown === true]);
}

export function tierFilter(row: TierFacts): TierFilter {
  return row.tier_unknown === true
    ? { tier: undefined, tier_unknown: true }
    : { tier: row.tier ?? 'none' };
}

/** Historical nulls remain Standard. A declared unavailable actual tier has no facet. */
export function tierAvailable(row: RequestTierFacts): boolean {
  const requested = row.timing?.requested_service_tier;
  const state = row.timing?.reported_service_tier?.state;
  if (state === 'unread' || row.timing?.wire_protocol != null && state === 'unsupported') return false;
  return !(row.tier == null && requested != null && requested !== 'default' && requested !== 'standard' && state !== 'told' && state !== 'empty');
}

/** Mirrors the optional tier filters; unfiltered logs still retain unsupported requests. */
export function matchesTier(row: RequestTierFacts, filter: TierFilter): boolean {
  const available = tierAvailable(row);
  if (filter.tier_unknown !== undefined && (!available || (row.tier_unknown === true) !== filter.tier_unknown)) return false;
  if (filter.tier === undefined) return true;
  if (!available) return false;
  return filter.tier === 'none'
    ? row.tier == null && row.tier_unknown !== true
    : row.tier === filter.tier;
}
