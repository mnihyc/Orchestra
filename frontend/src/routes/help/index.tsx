/** The help view at full width. Loader, permission handling and search schema are pending.
 *
 * The help as a page of its own: for the screen the reader came from, the articles, the keys. It is the
 * same help as the view beside a screen; here there is the whole width, and an article has an address.
 */
import { createFileRoute } from '@tanstack/react-router';
import { CircleQuestionMark } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { HelpContent } from '@/features/help/HelpView';
import { helpSearch } from '@/features/help/navigation';
import { useScreenHeading } from '@/shell/screen';

export const Route = createFileRoute('/help/')({
  staticData: { screen: 'page' },
  validateSearch: helpSearch,
  component: Screen,
});

function Screen() {
  const { t } = useTranslation();
  useScreenHeading({ title: t('screens.help.title'), icon: CircleQuestionMark, views: { ready: true, help: false } });
  return (
    <div className="mx-auto h-full w-full max-w-(--container-prose)">
      <HelpContent page />
    </div>
  );
}
