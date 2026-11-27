/** One page summary follows the current thread read selection, including retained history; the person can fold it to its
 * title, and that choice is kept for them. A summary with nothing in it yet is one closed row, as on a topic's page. */
import { useTranslation } from 'react-i18next';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadState, useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';
import { ResourceSummary } from './ResourceSummary';
import { SummaryFoldRow } from './SummaryBox';

export function ThreadSummaryInspector({ threadId, enabled = true }: { threadId: string; enabled?: boolean }) {
  const { t } = useTranslation();
  const { summaryHidden, setSummaryHidden } = useTimelinePreferences();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  const state = threadReadState(view);
  const reason = state.readiness === 'loading' ? t('summaryBox.reading')
    : state.readiness === 'ready' ? t('summaryBox.historical') : t('summaryBox.viewUnavailable');
  // Its title shows or puts it away; a summary put away is not read until it is shown again.
  const fold = { open: !summaryHidden, onOpenChange: (open: boolean) => setSummaryHidden(!open) };
  // A summary already shown stays while the conversation is read again; the line takes its place only for a first
  // opening, a failure with nothing kept, or an older version.
  return <div className="border-b border-border px-3 py-2" data-summary-inspector data-after-content>
    {state.readOnly && !state.kept ? <StateLine compact title={reason} className="text-text-muted"
      role={state.readiness === 'loading' ? 'status' : undefined}
      action={state.readiness === 'failed' ? <Button size="sm" variant="ghost" onClick={() => reloadThread(read)}>{t('common.tryAgain')}</Button> : undefined} />
      : summaryHidden ? <SummaryFoldRow fold={fold} />
        : <ResourceSummary target={{ kind: 'thread', id: threadId }} reference={`/thread/${threadId}`} enabled={enabled} fold={fold} foldWhenEmpty />}
  </div>;
}
