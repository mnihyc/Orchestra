/** The captured occurrence note stays separate from its input's actual delivery state. */
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { InputScheduleRun } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useErrorText } from '@/ui/errorText';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { timingFacts } from './TimingFacts';

export function scheduleRunWords(run: InputScheduleRun, t: TFunction, locale: string): string {
  const outcome = run.outcome;
  let note: string | null = null;
  if (outcome !== undefined && outcome !== null) {
    switch (outcome.reason_code) {
      case 'accepted':
        if (outcome.coalesced_count > 0) note = t('scheduleOccurrence.coalesced', { count: outcome.coalesced_count, n: formatNumber(outcome.coalesced_count, locale) });
        break;
      case 'previous_input_pending': note = t('scheduleOccurrence.previousPending'); break;
      case 'nonexistent_local_time': note = t('scheduleOccurrence.gap', { zone: outcome.zone }); break;
      case 'schedule_ended': note = t('scheduleOccurrence.ended'); break;
      case 'admission_failed': break;
    }
  }
  const state = t(`agentSchedules.outcomes.${run.state}`);
  return note === null ? state : `${state} · ${note}`;
}

export function ScheduleRunOutcome({ run }: { run: InputScheduleRun }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const describe = useErrorText();
  const outcome = run.outcome;
  return <><FactDisclosure fragment={scheduleRunWords(run, t, format.locale)} groups={[{ key: 'occurrence', kind: 'recorded_outcome', facts: [
    { key: 'scheduled', label: t('scheduleTimingFacts.scheduled'), value: outcome?.scheduled_for === undefined || outcome.scheduled_for === null ? undefined : <RelativeTime at={outcome.scheduled_for} /> },
    { key: 'coalesced', label: t('scheduleTimingFacts.coalesced'), value: outcome === undefined || outcome === null ? undefined : formatNumber(outcome.coalesced_count, format.locale) },
    ...timingFacts(outcome?.timing_policy, outcome?.zone, t),
  ] }]} />
    {run.state === 'failed' && run.reason !== null && <span>{describe(run.reason).message}</span>}</>;
}
