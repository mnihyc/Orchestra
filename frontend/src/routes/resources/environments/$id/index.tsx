/** Environments detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One environment and its tabs. The tab that is open is in the address (`tab`); an address that names a
 * tab the reader has none of shows the overview.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { EnvironmentScreen } from '@/features/resources/environments/Environment';
import { ENVIRONMENT_TABS, type EnvironmentTab } from '@/features/resources/tabs';
import { oneOf } from '@/features/resources/search';

export const Route = createFileRoute('/resources/environments/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: EnvironmentTab; port?: number; transport?: 'tcp' | 'udp'; setting?: string } => {
    const tab = oneOf(ENVIRONMENT_TABS, search.tab);
    const port = Number(search.port);
    return { ...(tab === undefined ? {} : { tab }), ...(typeof search.setting === 'string' && search.setting !== '' ? { setting: search.setting } : {}), ...(Number.isInteger(port) && port >= 1 && port <= 65535 ? { port } : {}), ...(search.transport === 'udp' || search.transport === 'tcp' ? { transport: search.transport } : {}) };
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const { tab, port, setting } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <EnvironmentScreen environmentId={id} initialPort={port} wantedKey={setting} tab={tab ?? 'overview'} onTab={(next) => { void navigate({ search: { tab: next }, replace: true }); }} />;
}
