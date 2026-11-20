/** One page summary follows the current thread read selection, including retained history; the person can fold it to its title, and that choice is kept for them. */
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadState, useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { StateLine } from '@/ui/StateLine';
import { ResourceSummary } from './ResourceSummary';

export function ThreadSummaryInspector({ threadId, enabled = true }: { threadId: string; enabled?: boolean }) {
  const { t } = useTranslation();
  const { summaryHidden, setSummaryHidden } = useTimelinePreferences();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  const state = threadReadState(view);
  const reason = state.readiness === 'loading' ? t('summaryBox.reading')
    : state.readiness === 'ready' ? t('summaryBox.historical') : t('summaryBox.viewUnavailable');
  return <div className="border-b border-border px-3 py-2" data-summary-inspector>
    {state.readOnly ? <StateLine compact title={reason} className="text-text-muted"
      role={state.readiness === 'loading' ? 'status' : undefined}
      action={state.readiness === 'failed' ? <Button size="sm" variant="ghost" onClick={() => reloadThread(read)}>{t('common.tryAgain')}</Button> : undefined} />
      : <div className="flex min-w-0 items-start gap-1">
        <IconButton size="sm" label={t(summaryHidden ? 'summaryBox.show' : 'summaryBox.hide')} icon={summaryHidden ? <ChevronRight aria-hidden /> : <ChevronDown aria-hidden />} aria-expanded={!summaryHidden} onClick={() => setSummaryHidden(!summaryHidden)} className="shrink-0" data-summary-fold />
        {summaryHidden ? <span className="min-h-(--control-sm) text-sm font-medium leading-(--control-sm) text-text-muted">{t('summaryBox.title')}</span>
          : <div className="min-w-0 flex-1"><ResourceSummary target={{ kind: 'thread', id: threadId }} reference={`/thread/${threadId}`} enabled={enabled} /></div>}
      </div>}
  </div>;
}
