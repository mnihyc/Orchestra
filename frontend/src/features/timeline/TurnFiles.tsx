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
