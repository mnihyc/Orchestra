/** Changes in the header row: the lines the thread's turns added and removed, opening the Changes view.
 *
 * The counts are read from the file changes of the rows the page holds, each edit counted once, as the footer of a
 * turn counts them; a thread that has changed nothing shows the mark alone.
 */
import { FileDiff } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';

export interface LineCounts {
  files: number;
  added: number;
  removed: number;
}

/** What every file change of the main stream added and removed, and how many distinct paths were changed. */
export function threadLineCounts(items: ReadonlyMap<string, Item>, order: readonly string[]): LineCounts {
  const paths = new Set<string>();
  let added = 0;
  let removed = 0;
  for (const id of order) {
    const item = items.get(id);
    if (item?.kind !== 'tool_call' || item.payload.tool.kind !== 'file_change') continue;
    for (const change of item.payload.tool.changes) {
      paths.add(change.path);
      added += change.added;
      removed += change.removed;
    }
  }
  return { files: paths.size, added, removed };
}

export function ChangesButton({ threadId, onOpen }: { threadId: string; onOpen: () => void }) {
  const { t } = useTranslation();
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  const counts = useMemo(() => threadLineCounts(items, order), [items, order]);
  const name = counts.files === 0 ? t('views.changes.title') : t('thread.frame.changesCounts', { added: counts.added, removed: counts.removed });
  return (
    <Tooltip content={name}>
      <Button size="sm" variant="ghost" aria-label={name} icon={<FileDiff aria-hidden />} onClick={onOpen}>
        {counts.files > 0 && <><span className="text-success-text">{`+${counts.added}`}</span><span className="text-danger-text">{`−${counts.removed}`}</span></>}
      </Button>
    </Tooltip>
  );
}
