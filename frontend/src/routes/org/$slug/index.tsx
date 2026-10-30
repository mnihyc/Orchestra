/** Org home, for a root org by its short name and for an org inside one by its address. Loader, permission handling and search schema are pending.
 *
 * The screen is the org's home: its overview, its members and its Activity, the section in the
 * address (`tab`). An org inside another is reached by its address, and also by its id, which is what
 * the tree of the left bar knows of it; the id is replaced by the address before anything is shown.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { useRememberedView } from '@/lib/viewChoices';
import { createFileRoute, redirect } from '@tanstack/react-router';

import { canonicalOrgAddress } from '@/features/navigation/orgAddress';
import { OrgHome, type OrgTab } from '@/features/org/Home';

interface OrgSearch {
  tab?: OrgTab;
}

const TABS: readonly OrgTab[] = ['overview', 'members', 'activity'];

export const Route = createFileRoute('/org/$slug/')({
  staticData: { screen: 'org' },
  validateSearch: (search: Record<string, unknown>): OrgSearch =>
    TABS.includes(search.tab as OrgTab) ? { tab: search.tab as OrgTab } : {},
  loader: async ({ params, location }) => {
    const address = await canonicalOrgAddress(params.slug);
    // The address that was asked for may say more than the section: the chat panel, which chat, which message. It all goes with it.
    if (address !== undefined) throw redirect({ to: '/org/$slug', params: { slug: address }, search: true, hash: location.hash, replace: true });
  },
  component: OrgRoute,
});

function OrgRoute() {
  const { slug } = usePageParams(Route);
  const { tab } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const [picked, choose] = useRememberedView('org', 'tab', tab, 'overview', TABS);
  return (
    <OrgHome
      slug={slug}
      tab={picked}
      onTab={(next) => { choose(next); void navigate({ search: { tab: next }, replace: true }); }}
    />
  );
}
