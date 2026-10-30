/** Templates list. Loader, permission handling and search schema are pending.
 *
 * The templates a person may use or manage, of both kinds. What is searched for and the kind are in the
 * address (`q`, `kind`), so that a link reproduces them; whatever an address leaves out is the default.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { oneOf, readText, TEMPLATE_KINDS, type TemplatesSearch } from '@/features/resources/search';
import { TemplateList } from '@/features/resources/templates/TemplateList';

/** What the address says, with everything that is not one of the things it may say left out. */
export function readSearch(search: Record<string, unknown>): TemplatesSearch {
  const q = readText(search.q);
  const kind = oneOf(TEMPLATE_KINDS, search.kind);
  return { ...(q === undefined ? {} : { q }), ...(kind === undefined ? {} : { kind }) };
}

export const Route = createFileRoute('/resources/templates/')({
  staticData: { screen: 'page' },
  validateSearch: readSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <TemplateList search={search} onSearch={(patch) => void navigate({ search: (before) => readSearch({ ...before, ...patch }), replace: true })} />;
}
