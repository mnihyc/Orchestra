/** The existing inspector shares the current page's native paragraph and read selection. */
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadState, useThreadView } from '@/realtime/stores/thread';
import { ResourceSummary } from './ResourceSummary';

export function ThreadSummaryInspector({ threadId }: { threadId: string }) {
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  if (threadReadState(view).readOnly) return null;
  return <div className="border-b border-border px-3 py-2" data-summary-inspector>
    <ResourceSummary target={{ kind: 'thread', id: threadId }} reference={`/thread/${threadId}`} />
  </div>;
}
