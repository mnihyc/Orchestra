/** The complete visible hierarchy, available to every signed-in person in the main area. */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { EverythingScreen } from '@/features/everything/Everything';
import { addressFromView, everythingSearch, viewFromAddress } from '@/features/everything/view';

export const Route = createFileRoute('/everything/')({
  staticData: { screen: 'page' },
  validateSearch: everythingSearch,
  component: Screen,
});

function Screen() {
  const search = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <EverythingScreen scope={search.scope ?? 'mine'} parent={search.parent ?? null} q={search.q ?? ''}
    {...(search.participating === undefined ? {} : { participating: search.participating })} view={viewFromAddress(search)}
    onView={(view) => void navigate({ search: (before) => ({ ...before, ...addressFromView(view) }), replace: true })}
    onParticipation={(participating) => void navigate({ search: (before) => ({ ...before, participating: participating || undefined }), replace: true })}
    onScope={(scope) => void navigate({ search: (before) => ({ ...before, scope }), replace: true })}
    onQuery={(q) => void navigate({ search: (before) => ({ ...before, q: q === '' ? undefined : q }), replace: true })} />;
}
