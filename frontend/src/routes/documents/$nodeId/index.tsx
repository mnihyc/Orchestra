/** The document area beside one readable place; its folders do not contain threads. */
import { usePageParams } from '@/features/workbench/pageContext';
import { createFileRoute } from '@tanstack/react-router';
import { DocumentDirectoryPage } from '@/features/documents/DirectoryBrowser';

export const Route = createFileRoute('/documents/$nodeId/')({ staticData: { screen: 'page' }, component: Screen });
function Screen() {
  const { nodeId } = usePageParams(Route);
  return <DocumentDirectoryPage node={nodeId} at={{ kind: 'node_docs', id: nodeId }} />;
}
