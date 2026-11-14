/** How a thread began, as one line: "Started with <agent> on <provider>".
 *
 * Who created the thread and when it began are under the pointer. The line opens, on a click, into the
 * lines it stands for: the thread was created, its source was prepared, its agent was started.
 */
import { ChevronRight, Sparkles } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatExact } from '@/lib/time';
import { useItem, useThreadPart } from '@/realtime/stores/thread';
import { cx } from '@/ui/cx';
import { useTimelineContext } from '../context';
import { MarkerLine } from './Marker';

function Part({ id }: { id: string }) {
  const { threadId } = useTimelineContext();
  const item = useItem(threadId, id);
  return item?.kind === 'marker' ? <MarkerLine item={item} /> : null;
}

export function StartLine({ ids }: { ids: readonly string[] }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const { threadId } = useTimelineContext();
  const [open, setOpen] = useState(false);
  // Selectors answer with an id, which is the same value each time, rather than with a list made anew.
  const startedId = useThreadPart(threadId, (view) => ids.find((id) => {
    const item = view.items.get(id);
    return item?.kind === 'marker' && item.payload.marker === 'segment_started';
  }));
  const createdId = useThreadPart(threadId, (view) => ids.find((id) => {
    const item = view.items.get(id);
    return item?.kind === 'marker' && item.payload.marker === 'created';
  }));
  const started = useItem(threadId, startedId ?? '');
  const created = useItem(threadId, createdId ?? '');
  if (started?.kind !== 'marker' || started.payload.marker !== 'segment_started') return null;
  const { agent, provider } = started.payload.data;
  const creator = created?.kind === 'marker' && created.payload.marker === 'created' ? created.payload.data.by.display_name : null;
  const hover = [creator === null ? null : t('timeline.start.created', { by: creator }), formatExact((created ?? started).created_at, context)]
    .filter((part): part is string => part !== null).join(' · ');
  return (
    <div className="flex flex-col items-center gap-1 py-1 text-sm text-text-subtle" data-thread-start>
      <button type="button" aria-expanded={open} title={hover} onClick={() => setOpen((held) => !held)}
        className="inline-flex max-w-full items-center gap-2 rounded-md px-2 py-0.5 text-center hover:bg-bg-muted">
        <Sparkles className="size-4 shrink-0" aria-hidden />
        <span className="wrap-anywhere">{t('timeline.start.line', { agent: agent.name, provider: provider.name })}</span>
        <ChevronRight className={cx('size-4 shrink-0 transition-transform', open && 'rotate-90')} aria-hidden />
      </button>
      {open && (
        <ol aria-label={t('timeline.start.open')} className="flex w-full list-none flex-col">
          {ids.map((id) => <li key={id}><Part id={id} /></li>)}
        </ol>
      )}
    </div>
  );
}
