/** Templates and their settings lead to the same first-template guide as Help and the finder. */
import { useTranslation } from 'react-i18next';
import { CircleHelp } from 'lucide-react';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { TEMPLATE_GUIDE } from '@/help/templates';

export function TemplateGuideLink() {
  const { t } = useTranslation();
  return <ArticleGuideLink article={TEMPLATE_GUIDE.id} title={t(TEMPLATE_GUIDE.title)} icon={<CircleHelp aria-hidden />} />;
}
