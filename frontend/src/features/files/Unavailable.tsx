/** What the place of a file shows when its content cannot be had: why, and what can still be done.
 *
 * A file that is not there, one the workspace's rules leave out, one too large to bring and one never
 * opened while its environment is away are all expected, so they are told as what they are and not as
 * a page that failed.
 */
import { FileX } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import type { FetchUnavailable } from '@/generated/api-types';
import { EmptyState } from '@/ui/EmptyState';

import { unavailableKey } from './content';

export function Unavailable({ reason }: { reason: FetchUnavailable }) {
  const { t } = useTranslation();
  return (
    <EmptyState
      compact
      className="h-full"
      icon={<FileX aria-hidden />}
      title={t(unavailableKey(reason))}
      description={reason === 'offline_not_cached' ? t('files.unavailable.offlineHint') : undefined}
    />
  );
}
