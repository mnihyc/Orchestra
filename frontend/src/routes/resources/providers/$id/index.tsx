/** Providers detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One provider and its tabs. The tab that is open is in the address (`tab`); an address that names a
 * tab the reader has none of shows the overview.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { providerTab, type ProviderTab } from '@/features/resources/tabs';
import { ProviderScreen } from '@/features/resources/providers/Provider';
import { readSearch,patchSearch,type UsageSearch } from '@/features/usage/search';

export const Route = createFileRoute('/resources/providers/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): UsageSearch & { tab?: ProviderTab } => {
    const tab = providerTab(search.tab);
    return {...readSearch(search),...(tab === undefined ? {} : {tab})};
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const search=usePageSearch(Route);
  const {tab}=search;
  const navigate = usePageNavigate(Route);
  return <ProviderScreen providerId={id} tab={tab ?? 'overview'} usageSearch={search}
    onUsageSearch={(patch)=>void navigate({search:(before)=>({...patchSearch(before,patch),...(before.tab===undefined ? {} : {tab:before.tab})}),replace:true})}
    onTab={(next) => { void navigate({ search: {...search,tab: next}, replace: true }); }} />;
}
