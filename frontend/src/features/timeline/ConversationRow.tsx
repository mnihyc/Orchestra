/** Every conversation surface draws the same classified turn rows. */
import { useTimelineContext } from '@/features/timeline/context';
import { NewLine } from '@/features/timeline/Catchup';
import { BeforeOrchHeading } from '@/features/timeline/items/BeforeOrch';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { StartLine } from '@/features/timeline/items/StartLine';
import { ItemRow } from '@/features/timeline/ItemRow';
import type { Row } from '@/features/timeline/rows';
import { SideThreadLaneRow } from '@/features/timeline/SideThreadLane';
import { WorkGroup } from '@/features/timeline/Steps';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { useThreadPart } from '@/realtime/stores/thread';

function MarkersRow({ ids }: { ids: readonly string[] }) {
  const { threadId } = useTimelineContext();
  const first = useThreadPart(threadId, (view) => view.items.get(ids[0] ?? ''));
  return first?.kind === 'marker' ? <MarkerLine item={first} count={ids.length} /> : null;
}

function NewLineRow() {
  const { threadId } = useTimelineContext();
  return <NewLine threadId={threadId} />;
}

/** One row of the conversation. */
export function ConversationRow({ row }: { row: Row }) {
  switch (row.kind) {
    case 'item':
      return <ItemRow id={row.id} answer={row.answer === true} named={row.named === true} />;
    case 'work':
      return <WorkGroup rowKey={row.key} turnId={row.turnId} ids={row.ids} last={row.last} live={row.live} only={row.only} />;
    case 'markers':
      return <MarkersRow ids={row.ids} />;
    case 'side':
      return <SideThreadLaneRow id={row.id} {...(row.closedId === undefined ? {} : { closedId: row.closedId })} {...(row.sourceTurn === undefined ? {} : { sourceTurn: row.sourceTurn })} />;
    case 'footer':
      return <TurnFooter turnId={row.turnId} inputItemId={row.inputItemId} answered={row.answered === true} lined={row.lined === true} />;
    case 'divider':
      return <NewLineRow />;
    case 'before':
      return <BeforeOrchHeading />;
    case 'start':
      return <StartLine ids={row.ids} />;
  }
}
