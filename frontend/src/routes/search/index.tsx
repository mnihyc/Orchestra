/** Search and its address state.
 *
 * The page that finds what was said, in conversations with agents, in chats and in documents. What it
 * looks for and what it is kept to are in the address (`q`, `scope`, `kinds`, `author`, `from`, `to`), read
 * leniently: a value that is not what it should be is left out.
 */
import { usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';

import { mergeParams, readParams, type SearchParams } from '@/features/search/params';
import { SearchPage } from '@/features/search/Search';

export const Route = createFileRoute('/search/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): SearchParams => readParams(search),
  component: SearchRoute,
});

function SearchRoute() {
  const params = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  return <SearchPage params={params} onParams={(next) => void navigate({ search: (before) => mergeParams(before, params, next), replace: true })} />;
}
