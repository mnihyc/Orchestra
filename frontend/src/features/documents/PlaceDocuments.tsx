/** The Documents view beside an org or project, and for a thread the list of its topic.
 *
 * The list is the shared one of the documents feature (`DocumentList`), the one a topic's own tab draws: the same rows, the same
 * New document, the same switch for what lies below. A topic uses only its page tab. The view follows the screen: it is of the node the
 * screen says it is of, unless the tab was put beside one thing on purpose.
 */
import { useQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { useViewShown } from '@/features/chat/shown';
import { DocumentList, type ListedNode } from '@/features/documents/DocumentList';
import type { ViewProps } from '@/features/workbench/views';
import { useCurrentHeading } from '@/shell/screen';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function PlaceDocuments({ tab, area }: ViewProps) {
  const { t } = useTranslation();
  const heading = useCurrentHeading();
  const shown = useViewShown(tab, area);
  const nodeId = tab.of?.node ?? heading?.nodeId ?? null;
  const own = useQuery({ ...nodeQuery(nodeId ?? ''), enabled: shown && nodeId !== null });
  // A thread has no documents of its own: those beside it are listed under its topic.
  const topicId =
    own.data?.kind === 'thread' ? ([...own.data.path].reverse().find((one) => one.kind === 'topic')?.id ?? null) : null;
  const topic = useQuery({ ...nodeQuery(topicId ?? ''), enabled: shown && topicId !== null });
  if (!shown) return null;

  const none = (
    <EmptyState
      compact
      className="h-full"
      icon={<FileText aria-hidden />}
      title={t('documents.panel.none')}
      description={t('documents.panel.noneAbout')}
    />
  );
  if (nodeId === null) return none;
  const failed = own.isError ? own : topic.isError ? topic : null;
  if (failed !== null) {
    return <ErrorState compact className="h-full" error={failed.error} retrying={failed.isRefetching} onRetry={() => void failed.refetch()} />;
  }
  if (own.data === undefined) return <SkeletonRows rows={4} className="p-4" />;
  let listed: ListedNode | undefined = own.data;
  if (own.data.kind === 'thread') {
    if (topicId === null) return none;
    listed = topic.data;
  }
  if (listed === undefined) return <SkeletonRows rows={4} className="p-4" />;
  return (
    <div className="h-full min-h-0 overflow-y-auto p-3" data-testid="documents-view">
      <DocumentList key={listed.id} node={listed} below />
    </div>
  );
}
