/** What was observed of a thread's work, as chips: Uncommitted, Not pushed, Last turn failed, each with when it was seen, and what could not be seen said as unknown.
 *
 * These are observations the server made and dates, never a review state a person sets: there is no
 * "Mark reviewed". A flag that is not known is never drawn as clean, an observation that may be old says
 * so, and the chips are words, not buttons. Cards keep concrete work or turn failures; detailed views
 * explain missing environment observations.
 */
import { useTranslation } from 'react-i18next';
import { Fragment } from 'react';

import type { ThreadAttention } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { RelativeTime } from '@/ui/RelativeTime';

import { workChips, workReason } from './words';

/** A thread row's attention as the server sent it, where a list row carries it; anything else is no observation. */
export function attentionOf(value: unknown): ThreadAttention | null {
  if (typeof value !== 'object' || value === null || !('attention' in value)) return null;
  const attention: unknown = value.attention;
  if (typeof attention !== 'object' || attention === null) return null;
  const record = attention as Record<string, unknown>;
  const flag = (key: string): boolean | null => {
    const seen = record[key];
    return typeof seen === 'boolean' ? seen : null;
  };
  return {
    uncommitted: flag('uncommitted'),
    unpushed: flag('unpushed'),
    failed: record.failed === true,
    as_of: typeof record.as_of === 'string' ? record.as_of : null,
    stale: record.stale === true,
    reason: typeof record.reason === 'string' ? record.reason : null,
  };
}

const TONES = {
  uncommitted: 'border-warning/40 bg-warning-subtle text-text',
  unpushed: 'border-warning/40 bg-warning-subtle text-text',
  failed: 'border-danger/30 bg-danger-subtle text-text',
  unknown: 'border-border text-text-muted',
} as const;

/** The observed facts of one thread's work; nothing when there is nothing to say. */
export function WorkChips({ attention, compact = false, context = 'detail' }: { attention: ThreadAttention | null | undefined; compact?: boolean; context?: 'detail' | 'card' }) {
  const { t } = useTranslation();
  // Cards retain concrete work and turn failures; missing environment observations belong in its detailed view.
  const chips = workChips(attention).filter((chip) => context !== 'card' || chip.kind !== 'unknown');
  const staleAfter = chips.reduce((last, chip, index) => chip.stale ? index : last, -1);
  if (attention === null || attention === undefined || chips.length === 0) return null;
  const hasWork = chips.some((chip) => chip.kind !== 'failed');
  const reason = hasWork ? workReason(t, attention.reason) : null;
  return (
    <span className="inline-flex flex-wrap items-center gap-1" role="group" aria-label={t('source.work.label')} data-work-chips>
      {chips.map((chip, index) => <Fragment key={chip.kind}>
        <span className={cx('inline-flex items-center rounded-full border px-2 text-xs', TONES[chip.kind])} data-work={chip.kind}>
          {chip.kind === 'unknown' ? t('source.work.unknown') : t(`source.work.${chip.kind}`)}
        </span>
        {index === staleAfter && <span className="text-xs text-text-muted">{t('source.work.stale')}</span>}
      </Fragment>)}
      {!compact && hasWork && attention.as_of !== null && (
        <span className="text-xs text-text-muted">
          {t('source.work.asOf')} <RelativeTime at={attention.as_of} />
        </span>
      )}
      {!compact && reason !== null && <span className="text-xs text-text-muted">{reason}</span>}
    </span>
  );
}
