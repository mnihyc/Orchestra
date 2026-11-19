/** Org settings. Loader, permission handling and search schema are pending.
 *
 * The settings of an org at any level: a root org and a department are the same page, and the section
 * in view is in the address. What each section needs of its reader is said inside it.
 */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute, redirect } from '@tanstack/react-router';

import { traceSearch } from '@/features/trace/params';
import { OrgSettings } from '@/features/settings/org/OrgSettings';
import { canonicalOrgAddress } from '@/features/navigation/orgAddress';

export const Route = createFileRoute('/org/$slug/settings/$section/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string,unknown>) => ({...traceSearch(search),...(typeof search.setting === 'string' && search.setting !== '' ? {setting:search.setting} : {})}),
  loader: async ({ params, location }) => {
    const address = await canonicalOrgAddress(params.slug);
    if (address !== undefined) throw redirect({ to: '/org/$slug/settings/$section', params: { slug: address, section: params.section }, search: true, hash: location.hash, replace: true });
  },
  component: Screen,
});

function Screen() {
  const { slug, section } = usePageParams(Route);
  return <OrgSettings slug={slug} section={section} />;
}
