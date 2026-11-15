/** The captured occurrence note stays separate from its input's actual delivery state. */
import { useTranslation } from 'react-i18next';
import type { InputScheduleRun } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useErrorText } from '@/ui/errorText';

export function ScheduleRunOutcome({ run }: { run: InputScheduleRun }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const describe = useErrorText();
  const outcome = run.outcome;
  let note: string | null = null;
  if (outcome !== undefined && outcome !== null) {
    switch (outcome.reason_code) {
      case 'accepted':
        if (outcome.coalesced_count > 0) note = t('scheduleOccurrence.coalesced', { count: outcome.coalesced_count, n: formatNumber(outcome.coalesced_count, format.locale) });
        break;
      case 'previous_input_pending': note = t('scheduleOccurrence.previousPending'); break;
      case 'nonexistent_local_time': note = t('scheduleOccurrence.gap', { zone: outcome.zone }); break;
      case 'schedule_ended': note = t('scheduleOccurrence.ended'); break;
      case 'admission_failed': break;
    }
  }
  const state = t(`agentSchedules.outcomes.${run.state}`);
  return <><span>{note === null ? state : `${state} · ${note}`}</span>
    {run.state === 'failed' && run.reason !== null && <span>{describe(run.reason).message}</span>}</>;
}
