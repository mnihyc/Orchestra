/** Hosts detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One host and its tabs. The tab that is open is in the address (`tab`); an address that names a tab the
 * reader has none of shows the overview.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { HOST_TABS, type HostTab } from '@/features/resources/tabs';
import { HostScreen } from '@/features/resources/hosts/Host';
import { oneOf } from '@/features/resources/search';

export const Route = createFileRoute('/resources/hosts/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: HostTab } => {
    const tab = oneOf(HOST_TABS, search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const { tab } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <HostScreen hostId={id} tab={tab ?? 'overview'} onTab={(next) => { void navigate({ search: { tab: next }, replace: true }); }} />;
}
