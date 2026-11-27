/** Pools retain their native inventory search in the address. */
import { createFileRoute } from '@tanstack/react-router';
import { usePageNavigate, usePageSearch } from '@/features/workbench/pageContext';
import { PoolList } from '@/features/resources/pools/PoolList';
import { readText } from '@/features/resources/search';
export const Route = createFileRoute('/resources/pools/')({ staticData: { screen: 'page' }, validateSearch: (search: Record<string, unknown>): { q?: string } => {
  const q = readText(search.q); return q === undefined ? {} : { q };
}, component: Screen });
function Screen() { const search = usePageSearch(Route); const navigate = usePageNavigate(Route);
  return <PoolList search={search} onSearch={(patch) => void navigate({ search: (before) => ({ ...before, ...patch }), replace: true })} />; }
