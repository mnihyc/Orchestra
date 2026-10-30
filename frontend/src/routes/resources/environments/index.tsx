/** Environments list. Loader, permission handling and search schema are pending.
 *
 * The environments a person may use or manage. What is searched for, where they run and how they stand are
 * in the address (`q`, `kind`, `status`), so that a link reproduces them; whatever an address leaves out is
 * the default.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { ENVIRONMENT_KINDS, ENVIRONMENT_STATUSES, EnvironmentList, type EnvironmentsSearch } from '@/features/resources/environments/EnvironmentList';
import { oneOf, readText } from '@/features/resources/search';

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): EnvironmentsSearch {
  const q = readText(search.q);
  const kind = oneOf(ENVIRONMENT_KINDS, search.kind);
  const status = oneOf(ENVIRONMENT_STATUSES, search.status);
  return { ...(q === undefined ? {} : { q }), ...(kind === undefined ? {} : { kind }), ...(status === undefined ? {} : { status }) };
}

export const Route = createFileRoute('/resources/environments/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <EnvironmentList search={search} onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })} />;
}
