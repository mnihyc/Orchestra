/** The caller's own usage, in full, with span, measure, split and filters as search parameters. Loader, permission handling and search schema are pending.
 *
 * The screen is one with three doors (`scope`: nothing for one's own, `node:<id>` for a place and
 * everything below it, `provider:<id>` for what ran on a provider), so that a link to any of them is
 * the same link. What the address leaves out is the default: the thirty days, in credits, split by model.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { Report } from '@/features/usage/Report';
import { parseScope, patchSearch, readSearch } from '@/features/usage/search';

export const Route = createFileRoute('/usage/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return (
    <Report
      scope={parseScope(search.scope)}
      search={search}
      onSearch={(patch) => void navigate({ search: (before) => patchSearch(before, patch), replace: true })}
    />
  );
}
