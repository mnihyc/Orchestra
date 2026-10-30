/** Agents list. Loader, permission handling and search schema are pending.
 *
 * The agent profiles a person may use or manage. What is searched for, whether they are on and the harness
 * are in the address (`q`, `state`, `harness`), so that a link reproduces them.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { AGENT_STATES, AgentList, type AgentsSearch } from '@/features/resources/agents/AgentList';
import { oneOf, readText } from '@/features/resources/search';

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): AgentsSearch {
  const q = readText(search.q);
  const state = oneOf(AGENT_STATES, search.state);
  const harness = readText(search.harness);
  return { ...(q === undefined ? {} : { q }), ...(state === undefined ? {} : { state }), ...(harness === undefined ? {} : { harness }) };
}

export const Route = createFileRoute('/resources/agents/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <AgentList search={search} onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })} />;
}
