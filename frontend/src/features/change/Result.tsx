/** Done: what was checked, what was left out and what is being removed. Or where it stopped, that everything was undone, and Try again.
 *
 * The end of a job, in the same view whichever way it ended. A job that succeeded says so, and what the checks found;
 * what could not be copied; and that the old side is being removed, which nobody has to wait for. A job that failed is
 * drawn in danger style: the phase it failed in, the reason in words, and that everything it had made was removed and
 * the thread is as it was, with what "as it was" does not include. One that was stopped says that too. Where a turn
 * that was told to end in time did not, there is a way to stop it and go on.
 */
import type { TFunction } from 'i18next';
import { CircleCheck, Undo2 } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { checksOf } from '@/features/change/Progress';
import { i18n } from '@/i18n';
import type { Job } from '@/generated/api-types';
import { errorText } from '@/ui/errorText';
import { StateLine } from '@/ui/StateLine';

/** The reason a job failed, in words of the page; the server's own sentence stands where there are none for its code. */
export function reasonWords(job: Job, t: TFunction, minutes: number): string {
  const error = job.error;
  if (error === null) return job.state === 'failed' ? t('ui.error.operationFailed') : '';
  const own = `change.failure.${error.code}`;
  const fallback = errorText(error, t, { locale: i18n.resolvedLanguage ?? 'en' }).message;
  return t(own, { defaultValue: fallback, minutes, message: error.code === 'binding_incompatible' ? error.message : fallback });
}

export interface ResultProps {
  job: Job;
  /** The sentence for what was done, in the past: "This thread now runs on Team Pro." */
  done: string;
  /** What the failure leaves out of "as it was": "The turn that was running was stopped, as you asked." */
  stoppedTurn: boolean;
  /** Minutes the wait for a turn was given, for the sentence of a wait that ran out. */
  waited: number;
  /** What is "as it was": "This thread", "Main", "Team Pro". */
  subject: string;
  children?: ReactNode;
}

export function Result({ job, done, stoppedTurn, waited, subject, children }: ResultProps) {
  const { t } = useTranslation();
  const checks = checksOf(job);
  if (job.state === 'succeeded') {
    return (
      <div className="flex flex-col gap-3" data-job-result="succeeded">
        <p className="flex items-start gap-2 text-base text-text">
          <CircleCheck className="mt-0.5 size-4 shrink-0 text-success" aria-hidden /> <span>{done}</span>
        </p>
        {checks.length > 0 && (
          <p className="text-sm text-text-muted">
            {t('change.result.checked')} {checks.map((check) => t(`change.check.${check.replaceAll(' ', '_')}`, { defaultValue: check })).join(t('startFlow.separator'))}
          </p>
        )}
        {children}
      </div>
    );
  }
  if (job.state === 'cancelled') {
    return (
      <div className="flex flex-col gap-3" data-job-result="cancelled">
        <p className="flex items-start gap-2 text-lg font-semibold text-text">
          <Undo2 className="mt-1 shrink-0 text-text-muted" aria-hidden /> {t('change.result.cancelled')}
        </p>
        <p className="text-base text-text">{t('change.result.undone', { subject })}</p>
        {stoppedTurn && <p className="text-sm text-text-muted">{t('change.result.turnStopped')}</p>}
      </div>
    );
  }
  const error = job.error;
  return (
    <div className="flex flex-col gap-3" data-job-result="failed">
      <StateLine role="alert" tone="danger" message={<>
        <span data-failure-code={error?.code}>{reasonWords(job, t, waited)}</span>{' '}
        <span>{error?.undone === false ? t('change.result.notUndone') : t('change.result.undone', { subject })}</span>
        {stoppedTurn && error?.undone !== false && <span>{' '}{t('change.result.turnStopped')}</span>}
      </>} />
    </div>
  );
}
