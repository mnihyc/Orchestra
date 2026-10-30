/** A refused answer keeps the request open and explains why beside its choices. */
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { ErrorAlert } from '@/ui/ErrorState';

export function DecisionError({ error }: { error: unknown }) {
  const { t } = useTranslation();
  const raw = error instanceof ApiError
    ? error.extra('request.invalid')?.errors ?? error.extra('request.value_refused')?.errors ?? []
    : [];
  const safe = error instanceof ApiError ? error.faults() : [];
  const reasons = raw.map((fault, index) => {
    if (fault.message === 'that decision does not fit what the harness asked') return t('approvals.answerMismatch');
    if (fault.message.startsWith('the request offers no option ')) return t('approvals.answerOptionGone');
    return safe[index]?.message ?? '';
  }).filter((reason) => reason.trim() !== '');
  return <ErrorAlert error={error} message={reasons.length === 0 ? undefined : [...new Set(reasons)].join(' ')} />;
}
