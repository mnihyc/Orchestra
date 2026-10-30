/** Agents detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One agent profile. The tab that is open is in the address (`tab`).
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { AgentScreen } from '@/features/resources/agents/Profile';
import { AGENT_TABS, type AgentTab } from '@/features/resources/tabs';
import { oneOf } from '@/features/resources/search';

export const Route = createFileRoute('/resources/agents/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: AgentTab } => {
    const tab = oneOf(AGENT_TABS, search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const { tab } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <AgentScreen agentId={id} tab={tab ?? 'overview'} onTab={(next) => { void navigate({ search: { tab: next }, replace: true }); }} />;
}
