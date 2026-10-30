/** The outline uses the conversation's words and follows held live rows without another list read. */
import { itemText } from '@/features/timeline/ItemActions';
import type { Item, OutlineEntry, OutlineKind } from '@/generated/api-types';
import type { ThreadView } from '@/realtime/stores/thread';

const visible = (kind: OutlineKind) => kind === 'turn' || kind === 'user_message' || kind === 'agent_message' || kind === 'milestone' || kind === 'approval' || kind === 'error';
const key = (entry: OutlineEntry) => entry.kind === 'turn' ? `turn:${entry.turn_ordinal}` : `${entry.kind}:${entry.item_id}`;

/** Keep older server entries, replace held ones with their current text, and add new live points. */
export function outlineRows(stored: readonly OutlineEntry[], view: Pick<ThreadView, 'order' | 'items' | 'turns'>): OutlineEntry[] {
  const entries = new Map(stored.filter((entry) => visible(entry.kind)).map((entry) => [key(entry), entry]));
  const add = (item: Item, kind: OutlineKind, title: string, ordinal: number | null = null, goalCondition?: string) => {
    const entry: OutlineEntry = { item_id: item.id, seq: item.seq_created, at: item.created_at, title, kind, turn_ordinal: ordinal, ...(goalCondition === undefined ? {} : { goal_condition: goalCondition }) };
    const id = key(entry);
    const previous = entries.get(id);
    // A partially held turn cannot replace its server-provided first-item link with its middle.
    if (kind === 'turn' && previous !== undefined && previous.seq <= entry.seq) return;
    entries.set(id, entry);
  };
  for (const id of view.order) {
    const item = view.items.get(id);
    if (item === undefined || item.kind === 'marker') continue;
    const turn = item.turn_id === null ? undefined : view.turns.get(item.turn_id);
    if (turn !== undefined) add(item, 'turn', '', turn.ordinal);
    if (item.kind === 'user_message' && item.payload.source === 'goal') {
      const condition = item.payload.goal?.condition ?? '';
      add(item, 'user_message', condition, null, condition);
    } else if (item.kind === 'user_message' || item.kind === 'agent_message') add(item, item.kind, itemText(item).trim().split(/\r?\n/)[0] ?? '');
    else if (item.kind === 'error') add(item, 'error', item.payload.message);
    else if (item.kind === 'announcement' && item.payload.category === 'milestone') add(item, 'milestone', item.payload.title);
    else if (item.kind === 'approval') {
      const title = item.payload.approval?.title ?? entries.get(`approval:${item.id}`)?.title;
      if (title !== undefined) add(item, 'approval', title);
    } else if (item.kind === 'tool_call' && item.payload.tool.kind === 'orch' && item.payload.tool.tool === 'announce') {
      const args = item.payload.tool.arguments;
      if (typeof args === 'object' && args !== null && 'category' in args && args.category === 'milestone' && 'title' in args && typeof args.title === 'string') add(item, 'milestone', args.title);
    }
  }
  return [...entries.values()].sort((a, b) => a.seq - b.seq || Number(b.kind === 'turn') - Number(a.kind === 'turn'));
}
