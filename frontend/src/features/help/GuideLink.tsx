/** A control, card or panel opens its version-matched guide beside the work. */
import { useTranslation } from 'react-i18next';

import type { SettingsTarget } from '@/api/settings';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { GUIDANCE, type GuideId } from '@/help/guidance';

export function GuideLink({ guide, target }: { guide: GuideId; named?: true; target?: SettingsTarget }) {
  const { t } = useTranslation();
  const def = GUIDANCE.find((one) => one.id === guide);
  if (def === undefined) return null;
  return <ArticleGuideLink article={guide} title={t(def.title)} target={target} />;
}
