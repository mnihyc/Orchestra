/** One job's retained coordination facts, shared by live progress and its result. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { Job, JobWaitTime } from '@/generated/api-types';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { errorText } from '@/ui/errorText';
import { useFormatContext } from '@/lib/appearance';

export function JobFacts({ job }: { job: Job }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const wait = job.lifecycle_wait;
  if (wait === null || wait === undefined) {
    if (job.kind !== 'environment_remove') return null;
  }
  const moment = (value: JobWaitTime | undefined): ReactNode => value === undefined || value.state === 'unreported' ? null
    : value.state === 'not_applicable' ? t('placeFacts.notApplicable') : <RelativeTime at={value.at} />;
  const code = wait?.reason_code;
  const reason = code === null || code === undefined ? wait?.reason
    : errorText({ code, message: wait?.reason ?? '' }, t, format).message;
  return <FactDisclosure key={job.id} groups={[{ key: 'lifecycle', kind: 'recorded_outcome', facts: [
    { key: 'phase', label: t('placeFacts.stage'), value: wait === null || wait === undefined ? null : wait.phase === 'cleanup' ? t('placeFacts.cleanup') : t(`change.phase.${wait.phase}.short`, { defaultValue: t('factDisclosure.notReported') }) },
    { key: 'attempt', label: t('placeFacts.lastAttempt'), value: moment(wait?.last_attempt_at) },
    { key: 'retry', label: t('placeFacts.nextEligible'), value: moment(wait?.retry_eligible_at) },
    { key: 'lease', label: t('placeFacts.leaseRecovery'), value: moment(wait?.lease_recoverable_at) },
    ...(wait?.retry_eligible_at.state === 'not_applicable' && !reason ? [] : [{ key: 'reason', label: t('placeFacts.reason'), value: reason }]),
    { key: 'observed', label: t('placeFacts.observed'), value: wait?.observed_at === null || wait?.observed_at === undefined ? null : <RelativeTime at={wait.observed_at} /> },
  ] }]} />;
}
