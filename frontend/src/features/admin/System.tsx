/** Release facts, prepared public addresses and the existing operator commands. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { systemQuery } from '@/api/admin';
import { Installation } from '@/features/admin/Installation';
import { PublicAddressSettings } from '@/features/admin/PublicAddress';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { Facts } from '@/features/resources/Facts';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function System() {
  const { t } = useTranslation();
  const system = useQuery(systemQuery);
  return <div className="flex flex-col gap-6">
    {system.isPending ? <SkeletonRows read={system} rows={1} /> : system.isError ? <ErrorState error={system.error} onRetry={() => void system.refetch()} /> :
      <Facts rows={[{name:t('componentUpdates.runningServer'),value:system.data.version}]} />}
    <PublicAddressSettings />
    <Installation />
    <ArticleGuideLink article="updates" title={t('componentUpdates.guideTitle')} />
  </div>;
}
