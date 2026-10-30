/** Hosts list. Loader, permission handling and search schema are pending.
 *
 * The hosts a person may use or manage, the system's among them. What is searched for and whether the host
 * takes new environments are in the address (`q`, `state`), so that a link reproduces them; whatever an
 * address leaves out is the default.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { HostList } from '@/features/resources/hosts/HostList';
import { HOST_STATES, oneOf, readText, type HostsSearch } from '@/features/resources/search';

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): HostsSearch {
  const q = readText(search.q);
  const state = oneOf(HOST_STATES, search.state);
  return { ...(q === undefined ? {} : { q }), ...(state === undefined ? {} : { state }) };
}

export const Route = createFileRoute('/resources/hosts/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <HostList search={search} onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })} />;
}
