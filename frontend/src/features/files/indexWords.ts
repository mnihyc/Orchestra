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

/** An older machine may omit the bound; an unknown bound is never replaced by a guessed default. */
export function liveIndexReason(t: TFunction, reason: EnvironmentIndexReason | null | undefined, facts: unknown): string {
  const limit = record(facts).index_entry_limit;
  return reason === 'entries' && typeof limit === 'number' && Number.isSafeInteger(limit) && limit >= 0
    ? t('largeTree.reasons.entriesWithLimit', { count: limit })
    : t(liveIndexReasonKey(reason));
}
