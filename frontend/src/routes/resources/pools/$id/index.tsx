/** One ordinarily readable pool and its native member and access controls. */
import { createFileRoute } from '@tanstack/react-router';
import { usePageParams } from '@/features/workbench/pageContext';
import { PoolScreen } from '@/features/resources/pools/Pool';
export const Route = createFileRoute('/resources/pools/$id/')({ staticData: { screen: 'page' }, component: Screen });
function Screen() { const { id } = usePageParams(Route); return <PoolScreen id={id} />; }
