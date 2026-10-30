/** Mounts detail. Loader, permission handling and search schema are pending.
 *
 * The screen itself belongs to a later part of the work. Its route is here from the start, inside the
 * workbench, so that its address works; until the screen is built it says what it will show.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { MountScreen } from '@/features/resources/mounts/Mount';
import { MOUNT_TABS, type MountTab } from '@/features/resources/tabs';

export const Route = createFileRoute('/resources/mounts/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: MountTab } => {
    const tab = MOUNT_TABS.find((candidate) => candidate === search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const navigate = usePageNavigate(Route);
  return <MountScreen id={usePageParams(Route).id} tab={usePageSearch(Route).tab ?? 'overview'} onTab={(tab) => { void navigate({ search: { tab }, replace: true }); }} />;
}
