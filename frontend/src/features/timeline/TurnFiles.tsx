/** One turn's admitted file changes, as one card at the end of the turn: "3 files changed +10 −3", opening the list, with Review.
 *
 * It stands once, under a turn that is over, and never says the same thing twice; opening the list does not open another
 * history or workspace.
 */
import { ChevronDown, ChevronRight } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { Turn } from '@/generated/api-types';
import { FileLink } from '@/features/files/FileLink';
import { openTurnReview } from '@/features/inspector/reviewTarget';
import { useConversationOrder, useTimelineContext } from '@/features/timeline/context';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { FileChanges } from '@/features/timeline/tools/FileChange';
import { Dialog } from '@/ui/Dialog';
import { Tooltip } from '@/ui/Tooltip';
import { VirtualList } from '@/ui/VirtualList';

import { turnChanges, type TurnChanges } from './turnChanges';
export { turnChanges, type TurnChanges };

export function useTurnChanges(threadId: string, turnId: string): TurnChanges {
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useConversationOrder(threadId);
  const context = useTimelineContext();
  const stream = context.threadId === threadId ? context.conversation?.stream ?? 'main' : 'main';
  const held = useThreadPart(threadId, (view) => view.turns.get(turnId));
  const turn = stream === 'main' ? (context.threadId === threadId && context.conversation !== undefined ? context.conversation.turns.get(turnId) : held) : undefined;
  const observed = turn?.files_changed_paths;
  const complete = turn?.files_changed_complete !== false;
  return useMemo(() => turnChanges(new Map([...items].filter(([, item]) => item.stream === stream)), order, turnId, observed, complete), [items, order, turnId, stream, observed, complete]);
}

export function turnFilesLabel(t: TFunction, changes: TurnChanges): string {
  return t(changes.coverage === 'observed' ? 'thread.turnFiles.changed' : changes.coverage === 'scoped' ? 'thread.turnFiles.known' : 'thread.turnFiles.reported', { count: changes.files.length });
}

export function TurnFiles({ turn, changes }: { turn: Turn; changes: TurnChanges }) {
  return changes.files.length === 0 ? null : <FileDisclosure turn={turn} changes={changes} />;
}

function FileDisclosure({ turn, changes }: { turn: Turn; changes: TurnChanges }) {
  const { t } = useTranslation();
  const { threadId, inspection, conversation } = useTimelineContext();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [reviewing, setReviewing] = useState(false);
  const count = changes.files.length;
  const large = count > 10;
  const wanted = large ? query.trim().toLocaleLowerCase() : '';
  const shown = useMemo(() => changes.files.filter(({ change }) => wanted === '' || change.path.toLocaleLowerCase().includes(wanted) || change.old_path?.toLocaleLowerCase().includes(wanted) === true), [changes.files, wanted]);
  const line = (change: TurnChanges['files'][number]['change']) => <div className="flex min-w-0 flex-wrap items-center gap-2 py-1">
    <code className="min-w-0 flex-1 wrap-anywhere">{change.path}</code>
    {change.change !== null && <span className="shrink-0 text-xs text-text-muted tabular-nums">{t('timeline.step.changed', { added: change.added, removed: change.removed })}</span>}
    {change.change === 'delete' ? <Tooltip content={t('inspector.changes.deleted')}><span aria-disabled="true" className="text-text-muted">{t('inspector.changes.openFile')}</span></Tooltip>
      : <FileLink path={change.path}>{t('inspector.changes.openFile')}</FileLink>}
  </div>;
  return <div data-turn-files={turn.id} className="flex min-w-0 flex-col rounded-lg border border-border bg-surface text-sm">
    <div className="flex min-h-(--control-md) min-w-0 items-center gap-2 pr-1 pl-3">
      <button type="button" aria-expanded={open} onClick={() => setOpen((on) => !on)} className="flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md text-left" data-turn-changes={turn.id}>
        {open ? <ChevronDown className="shrink-0 text-text-subtle" aria-hidden /> : <ChevronRight className="shrink-0 text-text-subtle" aria-hidden />}
        <span className="min-w-0 truncate">{turnFilesLabel(t, changes)}{(changes.added > 0 || changes.removed > 0) && <> · {t('thread.turnFiles.toolLines', { added: changes.added, removed: changes.removed })}</>}</span>
      </button>
      {inspection === undefined && <Button size="sm" variant="ghost" onClick={() => conversation?.stream === undefined ? openTurnReview(threadId, turn.id) : setReviewing(true)}>{t('thread.turnFiles.review')}</Button>}
    </div>
    {open && <div className="flex min-w-0 flex-col gap-2 border-t border-border px-3 py-2">
      {large && <Input type="search" label={t('thread.turnFiles.find')} hideLabel placeholder={t('thread.turnFiles.find')} value={query} onChange={(event) => setQuery(event.target.value)} />}
      {large ? <VirtualList className="h-64 overflow-y-auto" startAtEnd={false} follow={false} count={shown.length}
        label={t('thread.turnFiles.list')} keyOf={(index) => shown[index]?.change.path ?? String(index)} estimate={() => 36}
        render={(index) => { const file = shown[index]; return file === undefined ? null : line(file.change); }} />
        : <ul className="flex max-h-80 min-w-0 flex-col gap-1 overflow-y-auto" aria-label={t('thread.turnFiles.list')}>
          {shown.map(({ change }) => <li key={change.path}>{line(change)}</li>)}
        </ul>}
      {shown.length === 0 && <p className="text-sm text-text-muted">{t('thread.turnFiles.noMatches')}</p>}
    </div>}
    {reviewing && <Dialog open onOpenChange={setReviewing} title={t('views.changes.title')}>
      <div className="flex min-h-0 flex-col gap-2">
        <Input label={t('thread.turnFiles.find')} hideLabel placeholder={t('thread.turnFiles.find')} value={query} onChange={(event) => setQuery(event.target.value)} />
        <div className="max-h-[70vh] min-h-0 overflow-y-auto">
          {shown.map(({ change, itemId }) => change.change === null ? <div key={change.path}>{line(change)}</div> : <FileChanges key={`${itemId}:${change.path}`} changes={[change]} itemId={itemId ?? undefined} />)}
          {shown.length === 0 && <p className="text-sm text-text-muted">{t('thread.turnFiles.noMatches')}</p>}
        </div>
      </div>
    </Dialog>}
  </div>;
}
