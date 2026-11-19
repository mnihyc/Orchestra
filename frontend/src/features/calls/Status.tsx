/** The same failure, action, duration and attempt in thread and workflow status places. */
import { Trans, useTranslation } from 'react-i18next';
import type { ReactNode } from 'react';
import type { CallRecovery } from '@/generated/api-types';
import { recoveryCause } from './History';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan, useRunningClock } from '@/lib/time';
import { CallModel } from './Model';
import { cx } from '@/ui/cx';

export function CallRecoveryStatus({ recovery, service = false }: { recovery: CallRecovery; service?: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useRunningClock(recovery.state === 'scheduled');
  const continuing = !service && recovery.continuing === true;
  const administrative = recovery.detail !== null && ['provider_switched_off', 'model_switched_off', 'request_cancelled'].includes(recovery.detail);
  const kind = t(`calls.kinds.${recovery.failure.reason === 'access' ? 'access' : recovery.failure.code}`, { defaultValue: t('calls.kinds.any') });
  let status: ReactNode;
  if (recovery.detail === 'repeat_limit') status = t('calls.repeatStopped', { kind, count: recovery.count ?? 0, occurrences: recovery.count === null ? '–' : formatNumber(recovery.count, format.locale), window: recovery.window_secs === null ? '–' : formatSpan(recovery.window_secs * 1_000, format, 'short') });
  else if (recovery.detail === 'returning') status = t('calls.returning');
  else if (recovery.state === 'stopped' || recovery.state === 'waiting') status = recovery.detail === null ? t('calls.needsAttention', { kind }) : t(`calls.details.${recovery.detail}`, { defaultValue: t('calls.needsAttention', { kind }) });
  else if (recovery.state === 'switching') status = t('calls.switching');
  else if (recovery.step?.type === 'failover') {
    const waiting = recovery.state === 'scheduled' && recovery.due_at !== null && Date.parse(recovery.due_at) > now;
    status = <Trans i18nKey={waiting ? 'calls.failingOverIn' : 'calls.failingOver'} values={{ time: recovery.due_at === null ? '–' : formatSpan(Math.max(0, Date.parse(recovery.due_at) - now), format, 'short') }} components={{ model: <CallModel providerId={recovery.step.candidate.provider_id} model={recovery.step.candidate.model} /> }} />;
  }
  else if (recovery.state === 'running') status = t(continuing ? 'calls.continuing' : 'calls.retrying', { kind: recoveryCause(recovery.failure.reason === 'access' ? 'access' : recovery.failure.code, t) });
  else if (recovery.due_at !== null) status = t(recovery.step?.type === 'wait' ? 'calls.waitUntil' : continuing ? 'calls.continueIn' : 'calls.retryIn', { kind, time: formatSpan(Math.max(0, Date.parse(recovery.due_at) - now), format, 'short') });
  else status = t('calls.needsAttention', { kind });
  const times = !administrative && (recovery.step?.type === 'retry' || recovery.step?.type === 'continue') ? recovery.step.times : null;
  const skipped=recovery.state!=='waiting' && recovery.state!=='stopped' && recovery.detail!==null && ['candidate_not_allowed','candidate_parameters','embedding_identity'].includes(recovery.detail) ? t(`calls.details.${recovery.detail}`, { defaultValue: t('calls.needsAttention', { kind }) }) : null;
  return <span className={cx('min-w-0 wrap-anywhere tabular-nums', !administrative && (recovery.state === 'waiting' || recovery.state === 'stopped') && 'text-warning-text')} data-call-recovery={recovery.state}>
    {status}{times !== null && ` · ${t('calls.attempt', { attempt: formatNumber(recovery.attempt, format.locale), total: formatNumber(times, format.locale) })}`}
    {skipped!==null && ` · ${skipped}`}
  </span>;
}
