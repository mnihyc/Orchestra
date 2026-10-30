/** Templates detail. The selected section belongs to this URL, not another item’s saved tab.
 *
 * One template and its tabs. The tab that is open is in the address (`tab`); an address that names a tab
 * the reader has none of shows the overview.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { oneOf } from '@/features/resources/search';
import { TEMPLATE_TABS, type TemplateTab } from '@/features/resources/tabs';
import { TemplateScreen } from '@/features/resources/templates/Template';

export const Route = createFileRoute('/resources/templates/$id/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): { tab?: TemplateTab } => {
    const tab = oneOf(TEMPLATE_TABS, search.tab);
    return tab === undefined ? {} : { tab };
  },
  component: Screen,
});

function Screen() {
  const { id } = usePageParams(Route);
  const { tab } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <TemplateScreen templateId={id} tab={tab ?? 'overview'} onTab={(next) => { void navigate({ search: { tab: next }, replace: true }); }} />;
}
