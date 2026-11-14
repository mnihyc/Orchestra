/** One turn's admitted file changes, as one card at the end of the turn: "3 files changed +10 −3", opening the list, with Review.
 *
 * It stands once, under a turn that is over, and never says the same thing twice; opening the list does not open another
 * history or workspace.
 */
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { FileChange, Item, Turn } from '@/generated/api-types';
import { FileLink } from '@/features/files/FileLink';
import { openTurnReview } from '@/features/inspector/reviewTarget';
import { useConversationOrder, useTimelineContext } from '@/features/timeline/context';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { FileChanges } from '@/features/timeline/tools/FileChange';
import { Dialog } from '@/ui/Dialog';
import { Tooltip } from '@/ui/Tooltip';

export interface TurnChanges {
  files: { change: FileChange; itemId: string }[];
  added: number;
  removed: number;
}

/** Distinct paths retain their last recorded change while counts include every edit made in this turn. */
export function turnChanges(items: ReadonlyMap<string, Item>, order: readonly string[], turnId: string): TurnChanges {
  const paths = new Map<string, { change: FileChange; itemId: string }>();
  let added = 0; let removed = 0;
  for (const id of order) {
    const item = items.get(id);
    if (item?.turn_id !== turnId || item.kind !== 'tool_call' || item.payload.tool.kind !== 'file_change') continue;
    for (const change of item.payload.tool.changes) {
      paths.set(change.path, { change, itemId: item.id });
      added += change.added; removed += change.removed;
    }
  }
  return { files: [...paths.values()], added, removed };
}

export function useTurnChanges(threadId: string, turnId: string): TurnChanges {
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useConversationOrder(threadId);
  return useMemo(() => turnChanges(items, order, turnId), [items, order, turnId]);
}

export function TurnFiles({ turn, changes }: { turn: Turn; changes: TurnChanges }) {
  const { t } = useTranslation();
  const { threadId, inspection, conversation } = useTimelineContext();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [reviewing, setReviewing] = useState(false);
  const wanted = query.trim().toLocaleLowerCase();
  const shown = useMemo(() => changes.files.filter(({ change }) => wanted === '' || change.path.toLocaleLowerCase().includes(wanted)), [changes.files, wanted]);
  const count = changes.files.length;
  if (count === 0) return null;
  return <div data-turn-files={turn.id} className="flex min-w-0 flex-col rounded-lg border border-border bg-surface text-sm">
    <div className="flex min-h-(--control-md) min-w-0 items-center gap-2 pr-1 pl-3">
      <button type="button" aria-expanded={open} onClick={() => setOpen((on) => !on)} className="flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md text-left" data-turn-changes={turn.id}>
        {open ? <ChevronDown className="shrink-0 text-text-subtle" aria-hidden /> : <ChevronRight className="shrink-0 text-text-subtle" aria-hidden />}
        <span className="min-w-0 truncate">{t('timeline.footer.changed', { count, added: changes.added, removed: changes.removed })}</span>
      </button>
      {inspection === undefined && <Button size="sm" variant="ghost" onClick={() => conversation?.stream === undefined ? openTurnReview(threadId, turn.id) : setReviewing(true)}>{t('thread.turnFiles.review')}</Button>}
    </div>
    {open && <div className="flex min-w-0 flex-col gap-2 border-t border-border px-3 py-2">
      <Input label={t('thread.turnFiles.search')} hideLabel placeholder={t('thread.turnFiles.search')} value={query} onChange={(event) => setQuery(event.target.value)} />
      <ul className="flex max-h-80 min-w-0 flex-col gap-1 overflow-y-auto" aria-label={t('thread.turnFiles.list')}>
      {shown.map(({ change }) => <li key={change.path} className="flex min-w-0 flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 wrap-anywhere">{change.path}</code>
        <span className="shrink-0 text-xs text-text-muted tabular-nums">{t('timeline.step.changed', { added: change.added, removed: change.removed })}</span>
        {change.change === 'delete' ? <Tooltip content={t('inspector.changes.deleted')}><span aria-disabled="true" className="text-text-muted">{t('inspector.changes.openFile')}</span></Tooltip>
          : <FileLink path={change.path}>{t('inspector.changes.openFile')}</FileLink>}
      </li>)}
      </ul>
      {shown.length === 0 && <p className="text-sm text-text-muted">{t('thread.turnFiles.noMatch')}</p>}
    </div>}
    {reviewing && <Dialog open onOpenChange={setReviewing} title={t('views.changes.title')}>
      <div className="flex min-h-0 flex-col gap-2">
        <Input label={t('thread.turnFiles.search')} hideLabel placeholder={t('thread.turnFiles.search')} value={query} onChange={(event) => setQuery(event.target.value)} />
        <div className="max-h-[70vh] min-h-0 overflow-y-auto">
          {shown.map(({ change, itemId }) => <FileChanges key={`${itemId}:${change.path}`} changes={[change]} itemId={itemId} />)}
          {shown.length === 0 && <p className="text-sm text-text-muted">{t('thread.turnFiles.noMatch')}</p>}
        </div>
      </div>
    </Dialog>}
  </div>;
}
