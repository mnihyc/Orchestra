/** One turn's admitted file changes; opening their list does not open another history or workspace. */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { FileChange, Item, Turn } from '@/generated/api-types';
import { FileLink } from '@/features/files/FileLink';
import { useTimelineContext } from '@/features/timeline/context';
import { useThreadPart } from '@/realtime/stores/thread';
import { Tooltip } from '@/ui/Tooltip';

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
  const order = useThreadPart(threadId, (view) => view.order);
  return useMemo(() => turnChanges(items, order, turnId), [items, order, turnId]);
}

export function TurnFiles({ turn, changes }: { turn: Turn; changes: TurnChanges }) {
  const { t } = useTranslation();
  const { detail } = useTimelineContext();
  const [open, setOpen] = useState(false);
  const count = changes.files.length;
  if (count === 0) return null;
  if (detail === 'summary') return <span data-turn-files-count={turn.id}>{t('thread.viewModes.files', { count })}</span>;
  return <details data-turn-files={turn.id} onToggle={(event) => setOpen(event.currentTarget.open)} className="basis-full text-xs text-text-muted">
    <summary className="w-fit cursor-pointer rounded-md py-1">{t('thread.viewModes.files', { count })}</summary>
    {open && <ul className="flex flex-col gap-1 py-1" aria-label={t('thread.turnFiles.list')}>
      {changes.files.map(({ change }) => <li key={change.path} className="flex min-w-0 flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 wrap-anywhere">{change.path}</code>
        {change.change === 'delete' ? <Tooltip content={t('inspector.changes.deleted')}><span aria-disabled="true">{t('inspector.changes.openFile')}</span></Tooltip>
          : <FileLink path={change.path}>{t('inspector.changes.openFile')}</FileLink>}
      </li>)}
    </ul>}
  </details>;
}
