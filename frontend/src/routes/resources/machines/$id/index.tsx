/** Machines detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One machine: what it is and reports, what is registered from it and the limits it keeps. The tab
 * that is open is in the address (`tab`).
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { Machine, type MachineTab } from '@/features/resources/machines/Machine';
import { oneOf } from '@/features/resources/search';

const TABS: readonly MachineTab[] = ['overview', 'registered', 'sessions', 'programs', 'storage', 'policy', 'certificate', 'installation', 'access'];

export const Route = createFileRoute('/resources/machines/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: MachineTab } => {
    const tab = oneOf(TABS, search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const { tab } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <Machine machineId={id} tab={tab ?? 'overview'} onTab={(next) => { void navigate({ search: { tab: next }, replace: true }); }} />;
}
