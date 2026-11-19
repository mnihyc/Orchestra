/** Whether the directory typed for a new thread is in its environment already or is made when the thread starts.
 *
 * Read from the environment itself once typing rests, in one word beside the field; nothing is said while it is
 * asked, for a new environment, or where the environment cannot answer (offline, or its files are not the reader's
 * to see).
 */
import { useQuery } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { statFile } from '@/api/workspace';

/** How long typing rests before the environment is asked: one request a pause, not one a key. */
const SETTLE_MS = 400;

export function DirectorySign({ environmentId, directory }: { environmentId: string | null; directory: string }) {
  const { t } = useTranslation();
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
    staleTime: 10_000,
  });
  if (!asked || settled !== directory || found.isPending) return null;
  if (found.data !== undefined) {
    return found.data.type === 'dir'
      ? <span className="text-success-text" data-directory-sign="exists">{t('start.where.directoryExists')}</span>
      : <span className="text-danger-text" data-directory-sign="file">{t('start.where.directoryIsFile')}</span>;
  }
  return found.error instanceof ApiError && found.error.status === 404
    ? <span className="text-text-muted" data-directory-sign="new">{t('start.where.directoryNew')}</span>
    : null;
}
