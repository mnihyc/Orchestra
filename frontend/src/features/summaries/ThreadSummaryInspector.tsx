/** The existing inspector shares the current page's native paragraph and read selection. */
import { useTranslation } from 'react-i18next';
import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadState, useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';
import { ResourceSummary } from './ResourceSummary';

export function ThreadSummaryInspector({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  const state = threadReadState(view);
  const reason = state.readiness === 'loading' ? t('summaryBox.reading')
    : state.readiness === 'ready' ? t('summaryBox.historical') : t('summaryBox.viewUnavailable');
  return <div className="border-b border-border px-3 py-2" data-summary-inspector>
    {state.readOnly ? <StateLine compact title={reason} className="text-text-muted"
      role={state.readiness === 'loading' ? 'status' : undefined}
      action={state.readiness === 'failed' ? <Button size="sm" variant="ghost" onClick={() => reloadThread(read)}>{t('common.tryAgain')}</Button> : undefined} />
      : <ResourceSummary target={{ kind: 'thread', id: threadId }} reference={`/thread/${threadId}`} />}
  </div>;
}
