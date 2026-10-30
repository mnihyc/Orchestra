/** Machines list. Loader, permission handling and search schema are pending.
 *
 * The machines a person has connected, with the means to add one. What is searched for and the one
 * filter are in the address (`q`, `status`), so that a link reproduces them; whatever an address leaves
 * out is the default.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { MachineList, type MachineFilter, type MachinesSearch } from '@/features/resources/machines/MachineList';
import { oneOf, readText } from '@/features/resources/search';

const FILTERS: readonly MachineFilter[] = ['online', 'offline', 'revoked'];

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): MachinesSearch {
  const q = readText(search.q);
  const status = oneOf(FILTERS, search.status);
  return { ...(q === undefined ? {} : { q }), ...(status === undefined ? {} : { status }) };
}

export const Route = createFileRoute('/resources/machines/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return (
    <MachineList
      search={search}
      onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })}
    />
  );
}
