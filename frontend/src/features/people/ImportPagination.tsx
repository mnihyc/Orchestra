/** Import lists replace one bounded page at a time; the beginning stays reachable. */
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export function ImportPagination({ first, more, loading = false, error, onFirst, onMore }: {
  first: boolean; more: boolean; loading?: boolean; error?: unknown; onFirst: () => void; onMore: () => void;
}) {
  const { t } = useTranslation();
  if (!first && !more && !error) return null;
  return <div className="flex flex-col gap-2">
    {error !== null && error !== undefined && <ErrorAlert error={error} />}
    <div className="flex flex-wrap justify-between gap-2">
      <Button size="sm" disabled={!first || loading} onClick={onFirst}>{t('importWizard.paging.beginning')}</Button>
      <Button size="sm" pending={loading} disabled={!more && !error} onClick={onMore}>{t(error ? 'common.tryAgain' : 'importWizard.paging.next')}</Button>
    </div>
  </div>;
}
