/** A person's trail kept to one node rather than a wider place. */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { ScopeTrail } from '@/features/trace/ScopeTrail';
import { traceSearch } from '@/features/trace/params';
export const Route = createFileRoute('/trace/$nodeId/')({ staticData: { screen: 'page' }, validateSearch: traceSearch, component: Screen });
function Screen() { const { nodeId } = usePageParams(Route); return <ScopeTrail id={nodeId} />; }
