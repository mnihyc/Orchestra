/** One turn's admitted file changes; opening their list does not open another history or workspace. */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { FileChange, Item, Turn } from '@/generated/api-types';
import { FileLink } from '@/features/files/FileLink';
import { useTimelineContext } from '@/features/timeline/context';
import { useThreadPart } from '@/realtime/stores/thread';
import { Input } from '@/ui/Input';
import { Tooltip } from '@/ui/Tooltip';

export interface TurnChanges {
  files: { change: Pick<FileChange, 'path'> & { change: FileChange['change'] | null }; itemId: string | null }[];
  added: number;
  removed: number;
  partial: boolean;
}

/** Distinct paths retain their last recorded change while counts include every edit made in this turn. */
export function turnChanges(items: ReadonlyMap<string, Item>, order: readonly string[], turnId: string, observed: readonly string[] = []): TurnChanges {
  const paths = new Map<string, TurnChanges['files'][number]>();
  let added = 0; let removed = 0;
  for (const id of order) {
    const item = items.get(id);
    if (item?.turn_id !== turnId || item.kind !== 'tool_call' || item.payload.tool.kind !== 'file_change') continue;
    for (const change of item.payload.tool.changes) {
      paths.set(change.path, { change, itemId: item.id });
      added += change.added; removed += change.removed;
    }
  }
  let partial = false;
  for (const path of observed) {
    if (paths.has(path)) continue;
    partial = true;
    paths.set(path, { change: { path, change: null }, itemId: null });
  }
  return { files: [...paths.values()], added, removed, partial };
}

export function useTurnChanges(threadId: string, turnId: string, observed: readonly string[] = []): TurnChanges {
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  const observed = useThreadPart(threadId, (view) => view.turns.get(turnId)?.files_changed_paths);
  return useMemo(() => turnChanges(items, order, turnId, observed ?? []), [items, order, turnId, observed]);
}

export function TurnFiles({ turn, changes }: { turn: Turn; changes: TurnChanges }) {
  const { t } = useTranslation();
  const { detail } = useTimelineContext();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const count = changes.files.length;
  if (count === 0) return null;
  if (detail === 'summary') return <span data-turn-files-count={turn.id}>{t('thread.viewModes.files', { count })}</span>;
  return <details data-turn-files={turn.id} onToggle={(event) => setOpen(event.currentTarget.open)} className="basis-full text-xs text-text-muted">
    <summary className="w-fit cursor-pointer rounded-md py-1">{t('thread.viewModes.files', { count })}</summary>
    {open && <div className="flex flex-col gap-1 py-1">
      {count > 10 && <Input label={t('thread.turnFiles.search')} hideLabel placeholder={t('thread.turnFiles.search')} value={search} onChange={(event) => setSearch(event.target.value)} />}
      <ul className="flex max-h-64 flex-col gap-1 overflow-y-auto" aria-label={t('thread.turnFiles.list')}>
      {changes.files.filter(({ change }) => change.path.toLocaleLowerCase().includes(search.toLocaleLowerCase())).map(({ change }) => <li key={change.path} className="flex min-w-0 flex-wrap items-center gap-2">
        <code className="min-w-0 flex-1 wrap-anywhere">{change.path}</code>
        {change.change === 'delete' ? <Tooltip content={t('inspector.changes.deleted')}><span aria-disabled="true">{t('inspector.changes.openFile')}</span></Tooltip>
          : <FileLink path={change.path}>{t('inspector.changes.openFile')}</FileLink>}
      </li>)}
    </ul></div>}
  </details>;
}
