/** Changes in the header row: the lines the selected review turn added and removed, opening that same Changes view.
 *
 * The counts are read from the file changes of the rows the page holds, each edit counted once, as the footer of a
 * turn counts them. A partial turn shows unknown counts; a turn that changed nothing shows the mark alone.
 */
import { FileDiff } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { Item } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';

export interface LineCounts {
  files: number;
  added: number;
  removed: number;
}

/** What this turn's file changes added and removed, and how many distinct paths were changed. */
export function turnLineCounts(items: ReadonlyMap<string, Item>, order: readonly string[], turnId: string): LineCounts {
  const paths = new Set<string>();
  let added = 0;
  let removed = 0;
  for (const id of order) {
    const item = items.get(id);
    if (item?.turn_id !== turnId || item.kind !== 'tool_call' || item.payload.tool.kind !== 'file_change') continue;
    for (const change of item.payload.tool.changes) {
      paths.add(change.path);
      added += change.added;
      removed += change.removed;
    }
  }
  return { files: paths.size, added, removed };
}

export function ChangesButton({ threadId, turnId, onOpen }: { threadId: string; turnId: string | null; onOpen: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  const turn = useThreadPart(threadId, (view) => turnId === null ? undefined : view.turns.get(turnId));
  const hasOlder = useThreadPart(threadId, (view) => view.hasOlder);
  const hasNewer = useThreadPart(threadId, (view) => view.hasNewer);
  const complete = !hasNewer && (!hasOlder || turn !== undefined && order.some((id) => {
    const item = items.get(id);
    return item?.kind === 'user_message' && item.turn_id === turnId && item.payload.input_id === turn.input_id;
  }));
  const counts = useMemo(() => turnId === null ? { files: 0, added: 0, removed: 0 } : !complete ? null : turnLineCounts(items, order, turnId), [items, order, turnId, complete]);
  const added = counts === null ? '' : formatNumber(counts.added, locale), removed = counts === null ? '' : formatNumber(counts.removed, locale);
  const name = counts === null ? t('thread.frame.changesUnknown') : counts.files === 0 ? t('views.changes.title') : t('thread.frame.changesCounts', { added, removed });
  return (
    <Tooltip content={name}>
      <Button size="sm" variant="ghost" aria-label={name} icon={<FileDiff aria-hidden />} onClick={onOpen}>
        {counts === null ? <span className="text-text-muted">–</span> : counts.files > 0 && <><span className="text-success-text">{`+${added}`}</span><span className="text-danger-text">{`−${removed}`}</span></>}
      </Button>
    </Tooltip>
  );
}
