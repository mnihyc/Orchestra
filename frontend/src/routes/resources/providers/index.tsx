/** Providers list. Loader, permission handling and search schema are pending.
 *
 * The providers a person owns, may use or manages, and for an admin of the org in view all of those in
 * it. What is searched for, the view and the status are in the address (`q`, `view`, `status`), so that a
 * link reproduces them; whatever an address leaves out is the default.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { ProviderList } from '@/features/resources/providers/ProviderList';
import { oneOf, PROVIDER_STATUSES, PROVIDER_VIEWS, type ProvidersSearch, readText } from '@/features/resources/search';

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): ProvidersSearch {
  const q = readText(search.q);
  const view = oneOf(PROVIDER_VIEWS, search.view);
  const status = oneOf(PROVIDER_STATUSES, search.status);
  return {
    ...(q === undefined ? {} : { q }),
    ...(view === undefined ? {} : { view }),
    ...(status === undefined ? {} : { status }),
  };
}

export const Route = createFileRoute('/resources/providers/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return (
    <ProviderList
      search={search}
      onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })}
    />
  );
}
