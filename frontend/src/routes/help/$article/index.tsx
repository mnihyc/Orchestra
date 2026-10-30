/** One article of the help, with a heading as its anchor. Loader, permission handling and search schema are pending.
 *
 * An article of the user guide as a page: its address is `/help/<article>`, and the heading after the hash is
 * brought into view. An address that names no article says so, with the way back to the list.
 */
import { useViewLocation } from '@/features/workbench/pageContext';
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { BookOpen } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { HelpContent } from '@/features/help/HelpView';
import { useLocalizedGuide as useGuide } from '@/help/localized';
import { useScreenHeading } from '@/shell/screen';

export const Route = createFileRoute('/help/$article/')({
  staticData: { screen: 'page' },
  component: Screen,
});

function Screen() {
  const { t } = useTranslation();
  const { article } = usePageParams(Route);
  const hash = useViewLocation({ select: (location) => location.hash });
  const guide = useGuide();
  const title = guide?.articles.find((candidate) => candidate.id === article)?.title;
  useScreenHeading({ title: title ?? t('screens.helpArticle.title'), icon: BookOpen, trail: [{ label: t('screens.sections.help'), to: '/help' }], views: { ready: true, help: false } });
  return (
    <div className="mx-auto h-full w-full max-w-(--container-prose)">
      <HelpContent page article={article} heading={hash === '' ? null : decodeURIComponent(hash)} />
    </div>
  );
}
