/** Whether the directory typed for a new thread is in its environment already or is made when the thread starts.
 *
 * Read from the environment itself once typing rests. A failed check states its safe cause and can be retried;
 * it never turns an inaccessible or offline directory into a claim that a new one will be made.
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { statFile } from '@/api/workspace';
import { IconButton } from '@/ui/IconButton';
import { useErrorText } from '@/ui/errorText';

/** How long typing rests before the environment is asked: one request a pause, not one a key. */
const SETTLE_MS = 400;

export function DirectorySign({ environmentId, directory }: { environmentId: string | null; directory: string }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const [settled, setSettled] = useState(directory);
  useEffect(() => {
    const timer = window.setTimeout(() => setSettled(directory), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [directory]);
  const asked = environmentId !== null && settled !== '';
  const found = useQuery({
    queryKey: ['start', 'directory', environmentId, settled],
    queryFn: ({ signal }) => statFile({ environmentId: environmentId ?? '', threadId: null }, settled, signal),
    enabled: asked,
    retry: false,
  });
  if (!asked || settled !== directory) return null;
  if (found.isPending) return <span role="status" className="text-sm text-text-muted" data-directory-sign="checking">{t('start.where.directoryChecking')}</span>;
  if (found.isError && found.error instanceof ApiError && found.error.status === 404) return <span className="text-text-muted" data-directory-sign="new">{t('start.where.directoryNew')}</span>;
  if (found.isError) {
    const cause = found.error instanceof ApiError && found.error.is('source.offline') ? t('start.where.directoryOffline')
      : found.error instanceof ApiError && found.error.status === 403 ? t('start.where.directoryNoAccess') : describe(found.error).message;
    return <span role="status" className="inline-flex min-w-0 flex-wrap items-center gap-x-2 text-sm text-text-muted" data-directory-sign="unchecked">
      <span>{t('start.where.directoryNotChecked')}</span><span aria-hidden>·</span><span>{cause}</span>
      <IconButton action="refresh" size="sm" label={t('common.tryAgain')} pending={found.isFetching} onClick={() => { void found.refetch(); }} />
    </span>;
  }
  return found.data.type === 'dir'
    ? <span className="text-success-text" data-directory-sign="exists">{t('start.where.directoryExists')}</span>
    : <span className="text-danger-text" data-directory-sign="file">{t('start.where.directoryIsFile')}</span>;
}
