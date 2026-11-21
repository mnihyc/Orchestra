/** Release facts, prepared public addresses and the existing operator commands. */
import { useTranslation } from 'react-i18next';
import { Installation } from '@/features/admin/Installation';
import { PublicAddressSettings } from '@/features/admin/PublicAddress';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';

export function System() {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-6">
    <Installation />
    <PublicAddressSettings />
    <ArticleGuideLink article="updates" title={t('componentUpdates.guideTitle')} />
  </div>;
}
