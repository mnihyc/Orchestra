/** A retained thing inspected read-only through its existing stored GET reader. */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { InspectionPage } from '@/features/trace/Inspection';
export const Route = createFileRoute('/inspect/$kind/$id/')({ staticData: { screen: 'page' }, component: Screen });
function Screen() { const { kind, id } = usePageParams(Route); return <InspectionPage kind={kind} id={id} />; }
