/** Shared words for the actual reported Live index state. */
import type { TFunction } from 'i18next';
import type { EnvironmentIndexReason } from '@/generated/api-types';
import { record } from '@/features/resources/UnitCommands';

/** The Files panel's reason for a live tree, with an honest fallback for an older connector. */
export function liveIndexReasonKey(reason: EnvironmentIndexReason | null | undefined): 'largeTree.reasons.chosen' | 'largeTree.reasons.entries' | 'largeTree.reasons.watches' | 'largeTree.reasons.scan_time' | 'largeTree.reasons.unreadable' | 'largeTree.reasons.unknown' {
  switch (reason) {
    case 'chosen': return 'largeTree.reasons.chosen';
    case 'entries': return 'largeTree.reasons.entries';
    case 'watches': return 'largeTree.reasons.watches';
    case 'scan_time': return 'largeTree.reasons.scan_time';
    case 'unreadable': return 'largeTree.reasons.unreadable';
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

/** A failed exclusion source is a coverage problem, never an empty rule set. */
export function exclusionProblem(t: TFunction, facts: unknown): string | null {
  const problem = record(record(facts).exclusion_problem);
  if (typeof problem.path !== 'string' || problem.path === '') return null;
  const where = typeof problem.line === 'number' && Number.isSafeInteger(problem.line) && problem.line > 0
    ? t('files.exclusions.line', { path: problem.path, line: problem.line }) : problem.path;
  let reason: string;
  switch (problem.reason) {
    case 'linked': reason = t('files.exclusions.linked'); break;
    case 'not_file': reason = t('files.exclusions.notFile'); break;
    case 'too_large': reason = t('files.exclusions.tooLarge'); break;
    case 'invalid_encoding': reason = t('files.exclusions.encoding'); break;
    case 'invalid_pattern': reason = t('files.exclusions.pattern'); break;
    case 'changed': reason = t('files.exclusions.changed'); break;
    default: reason = t('files.exclusions.unreadable');
  }
  return t('files.exclusions.problem', { where, reason });
}
