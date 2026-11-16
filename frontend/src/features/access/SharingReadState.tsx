/** Sharing reads keep a nameless current state without lending a cached audience. */
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { hasReadFailure, type RecoverableRead } from '@/api/reads';
import { ReadFeedback } from '@/ui/ReadFeedback';

export function SharingReadState({ reads }: { reads: readonly RecoverableRead[] }) {
  const { t } = useTranslation();
  const failures = reads.filter(hasReadFailure);
  const privateRead = failures.some((read) => read.error instanceof ApiError && [401, 403, 404, 410].includes(read.error.status));
  return <ReadFeedback reads={reads} pending
    title={t(privateRead ? 'implicitAvailability.sharingUnavailable' : 'implicitAvailability.sharingFailed')}
    pendingMessage={t('implicitAvailability.sharingChecking')} />;
}
