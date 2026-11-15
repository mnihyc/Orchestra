/** The current native start uses only its own admitted timing receipt and the existing line's clock. */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import type { SettingSource, Thread, Turn } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime, formatSpan } from '@/lib/time';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

function currentWait(thread: Thread, turn: Turn | null) {
  const receipt = turn?.start_wait;
  return thread.state === 'starting' && turn?.id === thread.current_turn_id && turn.state === 'starting'
    && receipt !== undefined && receipt !== null && receipt.closed_at === null ? receipt : null;
}

export function startWaitWords(thread: Thread, turn: Turn | null, t: TFunction, format: ReturnType<typeof useFormatContext>): string | null {
  const current = currentWait(thread, turn);
  if (current === null || current.deadline === null) return null;
  return `${t(current.stage === 'acceptance' ? 'startWaitFacts.acceptance' : current.stage === 'start' ? 'startWaitFacts.start' : 'startWaitFacts.events')} ${formatDateTime(current.deadline, format)}`;
}

export function StartWaitFacts({ thread, turn, fallback }: { thread: Thread; turn: Turn | null; fallback: ReactNode }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const current = currentWait(thread, turn);
  if (current === null && thread.starting_step !== 'turn') return fallback;
  const scope = current?.admitted_default_source ?? null;
  const source: SettingSource | null = scope === 'default' || scope === 'system' ? { scope, node: null }
    : scope === 'thread' ? { scope, node: { id: thread.id, kind: 'thread', name: thread.name, redacted: false } } : null;
  const key = current?.stage === 'start' ? 'threads.start_confirmation_seconds' : current?.stage === 'events' ? 'threads.start_event_grace_seconds' : 'threads.start_acceptance_seconds';
  return <span data-start-wait={current?.stage ?? 'unreported'}>
    <FactDisclosure fragment={current === null || current.deadline === null ? fallback : <>{t(current.stage === 'acceptance' ? 'startWaitFacts.acceptance' : current.stage === 'start' ? 'startWaitFacts.start' : 'startWaitFacts.events')} <RelativeTime at={current.deadline} /></>}
      groups={[{ key: 'start-wait', kind: 'recorded_outcome', facts: [
        { key: 'began', label: t('startWaitFacts.began'), value: current === null ? undefined : <RelativeTime at={current.began_at} /> },
        { key: 'deadline', label: t('startWaitFacts.deadline'), value: current === null ? undefined : current.deadline === null ? t('startWaitFacts.beyondCalendar') : <RelativeTime at={current.deadline} /> },
        { key: 'allowance', label: t('startWaitFacts.allowance'), value: current === null ? undefined : current.after_millis !== null && Number.isSafeInteger(current.after_millis) ? formatSpan(current.after_millis, format, 'long') : t('startWaitFacts.finite') },
        { key: 'source', label: t('startWaitFacts.source'), value: scope === null ? undefined : source === null ? t('settings.compact.hiddenSource')
          : <SourceSettingLink source={source} def={{ key, class: 'parameter' }} target={{ kind: 'node', id: thread.id }} fallback={t('factDisclosure.notReported')} defaultToSystem /> },
      ] }]} />
  </span>;
}
