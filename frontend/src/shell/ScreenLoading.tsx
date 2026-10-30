/** A screen's loading state occupies its main area, without replacing the surrounding shell. */
import { useTranslation } from 'react-i18next';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';

export function ScreenLoading() {
  const { t } = useTranslation();
  return <div role="status" aria-label={t('common.loading')} data-screen-loading className="relative min-h-0 p-4 sm:p-6">
    <SkeletonGroup className="flex flex-col gap-4"><Skeleton className="h-8 w-1/3" /><SkeletonRows rows={6} /></SkeletonGroup>
  </div>;
}
