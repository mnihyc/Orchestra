/** The Documents tab of a topic: the documents written beside it, and with a switch those beside the places below it.
 *
 * The list is the shared one of the documents feature: the same rows, the same New document and Import
 * Markdown, the same "Recently deleted" at the foot, whatever the place it is of.
 */
import { DocumentList, type ListedNode } from '@/features/documents/DocumentList';

export function TopicDocuments({ topic }: { topic: ListedNode }) {
  return <DocumentList node={topic} below />;
}
