/** Read-only release facts and operator commands; the API never receives power over its host. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { systemQuery } from '@/api/admin';
import { Installation } from '@/features/admin/Installation';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { Facts } from '@/features/resources/Facts';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { ConfigurationFacts } from '@/features/admin/ConfigurationFacts';

export function System() {
  const { t } = useTranslation();
  const system = useQuery(systemQuery);
  return <div className="flex flex-col gap-6">
    {system.isPending ? <SkeletonRows read={system} rows={1} /> : system.isError ? <ErrorState error={system.error} onRetry={() => void system.refetch()} /> :
      <Facts rows={[{name:t('componentUpdates.runningServer'),value:<span className="inline-flex min-w-0 flex-wrap items-center gap-1">{system.data.version}<ConfigurationFacts observation={system.data.effective_configuration}/></span>}]} />}
    <Installation />
    <ArticleGuideLink article="updates" title={t('componentUpdates.guideTitle')} />
  </div>;
}
