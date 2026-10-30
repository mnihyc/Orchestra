/** Opens a provider/user/interface guide from a real control without leaving the work. */
import { useTranslation } from 'react-i18next';

import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { PROVIDER_GUIDES, type ProviderGuideId } from '@/help/providers';

export function ProviderGuideLink({ guide }: { guide: ProviderGuideId; named?: true }) {
  const { t } = useTranslation();
  const def = PROVIDER_GUIDES.find((one) => one.id === guide);
  if (def === undefined) return null;
  return <ArticleGuideLink article={guide} title={t(def.title)} />;
}
