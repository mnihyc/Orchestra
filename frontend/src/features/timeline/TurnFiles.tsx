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
import { VirtualList } from '@/ui/VirtualList';

export interface TurnChanges {
  files: { change: FileChange; itemId: string }[];
  added: number;
  removed: number;
}

function snapshotChanges(item: Item): FileChange[] | null {
  if (item.kind !== 'marker' || item.payload.marker !== 'harness_changed' || item.payload.data.change !== 'turn_diff') return null;
  const value = item.payload.data.value;
  if (typeof value !== 'object' || value === null || !('changes' in value) || !Array.isArray(value.changes)) return null;
  const changes: FileChange[] = [];
  const reported: unknown[] = value.changes;
  for (const raw of reported) {
    if (typeof raw !== 'object' || raw === null) return null;
    const row = raw as Record<string, unknown>;
    const { path, change, old_path, diff, added, removed, binary, truncated } = row;
    if (typeof path !== 'string' || (change !== 'add' && change !== 'modify' && change !== 'delete' && change !== 'rename')
      || !(old_path === null || typeof old_path === 'string') || !(diff === null || typeof diff === 'string')
      || typeof added !== 'number' || !Number.isFinite(added) || typeof removed !== 'number' || !Number.isFinite(removed)
      || typeof binary !== 'boolean' || typeof truncated !== 'boolean') return null;
    // Keep only the public diff fields; a summary is no substitute for a tool's retained restore images.
    changes.push({ path, change, old_path, diff, added, removed, binary, truncated });
  }
  return changes;
}

/** A native turn/stream snapshot replaces its component edits, including an empty net change. */
export function turnChanges(items: ReadonlyMap<string, Item>, order: readonly string[], turnId: string): TurnChanges {
  const paths = new Map<string, { change: FileChange; itemId: string }>();
  const snapshots = new Map<string, FileChange[]>();
  const streams = new Set<string>();
  const scope = (item: Item) => JSON.stringify([item.segment_id, item.stream]);
  const held: Item[] = [];
  for (const id of new Set([...order, ...items.keys()])) {
    const item = items.get(id);
    if (item?.turn_id !== turnId) continue;
    held.push(item);
    const changes = snapshotChanges(item);
    if (changes === null) continue;
    snapshots.set(item.id, changes);
    streams.add(scope(item));
  }
  held.sort((a, b) => Date.parse(a.updated_at) - Date.parse(b.updated_at) || a.seq_updated - b.seq_updated);
  let added = 0; let removed = 0;
  const net = new Map<string, FileChange>();
  for (const item of held) {
    const snapshot = snapshots.get(item.id);
    const edits = item.kind === 'tool_call' && item.payload.tool.kind === 'file_change' && !streams.has(scope(item)) ? item.payload.tool.changes : [];
    for (const change of snapshot ?? edits) {
      paths.set(change.path, { change, itemId: item.id });
      if (snapshot === undefined) { added += change.added; removed += change.removed; }
      else net.set(change.path, change);
    }
  }
  for (const change of net.values()) { added += change.added; removed += change.removed; }
  return { files: [...paths.values()], added, removed };
}

export function useTurnChanges(threadId: string, turnId: string): TurnChanges {
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useConversationOrder(threadId);
  return useMemo(() => turnChanges(items, order, turnId), [items, order, turnId]);
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
  const line = (change: FileChange) => <div className="flex min-w-0 flex-wrap items-center gap-2 py-1">
    <code className="min-w-0 flex-1 wrap-anywhere">{change.path}</code>
    <span className="shrink-0 text-xs text-text-muted tabular-nums">{t('timeline.step.changed', { added: change.added, removed: change.removed })}</span>
    {change.change === 'delete' ? <Tooltip content={t('inspector.changes.deleted')}><span aria-disabled="true" className="text-text-muted">{t('inspector.changes.openFile')}</span></Tooltip>
      : <FileLink path={change.path}>{t('inspector.changes.openFile')}</FileLink>}
  </div>;
  return <div data-turn-files={turn.id} className="flex min-w-0 flex-col rounded-lg border border-border bg-surface text-sm">
    <div className="flex min-h-(--control-md) min-w-0 items-center gap-2 pr-1 pl-3">
      <button type="button" aria-expanded={open} onClick={() => setOpen((on) => !on)} className="flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md text-left" data-turn-changes={turn.id}>
        {open ? <ChevronDown className="shrink-0 text-text-subtle" aria-hidden /> : <ChevronRight className="shrink-0 text-text-subtle" aria-hidden />}
        <span className="min-w-0 truncate">{t('timeline.footer.changed', { count, added: changes.added, removed: changes.removed })}</span>
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
          {shown.map(({ change, itemId }) => <FileChanges key={`${itemId}:${change.path}`} changes={[change]} itemId={itemId} />)}
          {shown.length === 0 && <p className="text-sm text-text-muted">{t('thread.turnFiles.noMatches')}</p>}
        </div>
      </div>
    </Dialog>}
  </div>;
}
