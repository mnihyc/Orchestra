/** Read-only release facts and operator commands; the API never receives power over its host. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { systemQuery } from '@/api/admin';
import { Installation } from '@/features/admin/Installation';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { Facts } from '@/features/resources/Facts';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function System() {
  const { t } = useTranslation();
  const system = useQuery(systemQuery);
  return <div className="flex flex-col gap-6">
    {system.isPending ? <SkeletonRows rows={1} /> : system.isError ? <ErrorState error={system.error} onRetry={() => void system.refetch()} /> :
      <Facts rows={[{name:t('componentUpdates.runningServer'),value:system.data.version}]} />}
    <Installation />
    <ArticleGuideLink article="updates" title={t('componentUpdates.guideTitle')} />
  </div>;
}
