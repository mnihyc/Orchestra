/** Shared words for the actual reported Live index state. */
import type { TFunction } from 'i18next';
import type { EnvironmentIndexReason } from '@/generated/api-types';
import { record } from '@/features/resources/UnitCommands';

/** The Files panel's reason for a live tree, with an honest fallback for an older connector. */
export function liveIndexReasonKey(reason: EnvironmentIndexReason | null | undefined): 'largeTree.reasons.chosen' | 'largeTree.reasons.entries' | 'largeTree.reasons.watches' | 'largeTree.reasons.scan_time' | 'largeTree.reasons.unknown' {
  switch (reason) {
    case 'chosen': return 'largeTree.reasons.chosen';
    case 'entries': return 'largeTree.reasons.entries';
    case 'watches': return 'largeTree.reasons.watches';
    case 'scan_time': return 'largeTree.reasons.scan_time';
    default: return 'largeTree.reasons.unknown';
  }
}

/** How far the machine is in looking at a whole tree, while it is: the files in all and the share done in whole percent. */
export function indexingProgress(facts: unknown): { total: number; percent: number } | null {
  const { done, total } = record(record(facts).indexing);
  if (typeof done !== 'number' || typeof total !== 'number' || !Number.isSafeInteger(total) || total <= 0) return null;
  return { total, percent: Math.min(100, Math.max(0, Math.floor((done * 100) / total))) };
}

/** An older machine may omit the bound; an unknown bound is never replaced by a guessed default. */
export function liveIndexReason(t: TFunction, reason: EnvironmentIndexReason | null | undefined, facts: unknown): string {
  const limit = record(facts).index_entry_limit;
  return reason === 'entries' && typeof limit === 'number' && Number.isSafeInteger(limit) && limit >= 0
    ? t('largeTree.reasons.entriesWithLimit', { count: limit })
    : t(liveIndexReasonKey(reason));
}
