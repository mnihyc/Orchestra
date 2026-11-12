/** Sharing reads keep a nameless current state without lending a cached audience. */
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';

interface SharingRead {
  isError: boolean;
  isFetching: boolean;
  error: unknown;
  refetch: () => unknown;
}

export function SharingReadState({ reads }: { reads: readonly SharingRead[] }) {
  const { t } = useTranslation();
  const failures = reads.filter((read) => read.isError);
  const privateRead = failures.some((read) => read.error instanceof ApiError && [401, 403, 404, 410].includes(read.error.status));
  return <StateLine compact role="status" className="text-text-muted"
    message={t(privateRead ? 'implicitAvailability.sharingUnavailable' : failures.length > 0 ? 'implicitAvailability.sharingFailed' : 'implicitAvailability.sharingChecking')}
    action={!privateRead && failures.length > 0 && <Button size="sm" variant="ghost" pending={failures.some((read) => read.isFetching)} onClick={() => { for (const read of failures) void read.refetch(); }}>{t('common.tryAgain')}</Button>} />;
}
