/** Ordinary inverse operations for one shown block, guarded against newer selected work. */
import type { DocumentBlock, DocumentChanges, DocumentOp, NewBlock } from '@/generated/api-types';
import { comparePositions } from '@/features/documents/blocks';

type Change = DocumentChanges['blocks'][number];
export type Inverse = { kind: 'update' | 'added' | 'removed'; ops: DocumentOp[] } | { kind: 'stale'; ops: [] };

const shape = (block: DocumentBlock) => JSON.stringify([block.type, block.props, block.content, block.parent_id, block.position, block.removed]);
const ordered = (blocks: DocumentBlock[]) => blocks.sort((a, b) => comparePositions(a.position, b.position) || a.id.localeCompare(b.id));
function descendants(root: string, blocks: Map<string, DocumentBlock>): DocumentBlock[] {
  const out: DocumentBlock[] = [];
  const walk = (parent: string) => {
    for (const child of ordered([...blocks.values()].filter((one) => !one.removed && one.parent_id === parent))) {
      if (out.some((one) => one.id === child.id)) continue;
      out.push(child); walk(child.id);
    }
  };
  walk(root); return out;
}
function predecessor(block: DocumentBlock, live: Map<string, DocumentBlock>): string | null {
  return ordered([...live.values()].filter((one) => !one.removed && one.id !== block.id && one.parent_id === block.parent_id && comparePositions(one.position, block.position) < 0)).at(-1)?.id ?? null;
}
/** Returns no operation when the after-state changed or the selected parent is gone. */
export function inverseOf(change: Change, changes: DocumentChanges, liveBlocks: Iterable<DocumentBlock>, freshId: () => string): Inverse {
  const live = new Map([...liveBlocks].filter((block) => !block.removed).map((block) => [block.id, block]));
  const current = live.get(change.id);
  const stale: Inverse = { kind: 'stale', ops: [] };
  if (change.after !== null) {
    if (current?.seq_updated !== change.after.seq_updated || shape(current) !== shape(change.after)) return stale;
    if (change.before === null) {
      const after = new Map(changes.blocks.flatMap((one) => one.after === null ? [] : [[one.id, one.after] as const]));
      const expected = descendants(change.id, after);
      const actual = descendants(change.id, live);
      if (expected.length !== actual.length || actual.some((one) => { const wanted = after.get(one.id); return wanted?.seq_updated !== one.seq_updated || shape(wanted) !== shape(one); })) return stale;
      return { kind: 'added', ops: [{ op: 'remove', id: change.id }] };
    }
    const before = change.before;
    if (before.parent_id !== null && !live.has(before.parent_id)) return stale;
    const ops: DocumentOp[] = [{ op: 'update', id: current.id, base_seq: current.seq_updated, type: before.type, props: before.props, content: before.content }];
    if (before.parent_id !== current.parent_id || before.position !== current.position) ops.push({ op: 'move', id: current.id, parent_id: before.parent_id, after_id: predecessor(before, live) });
    return { kind: 'update', ops };
  }
  const before = change.before;
  if (before === null || current !== undefined || (before.parent_id !== null && !live.has(before.parent_id))) return stale;
  const historical = new Map(changes.blocks.flatMap((one) => one.before === null ? [] : [[one.id, one.before] as const]));
  const subtree = [before, ...descendants(before.id, historical)];
  // Old discussion anchors are intentionally not reused; internal block links in the returned copy are.
  const ids = new Map(subtree.map((block) => [block.id, freshId()]));
  const rewrite = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(rewrite);
    if (value !== null && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key,
      key === 'href' && typeof item === 'string' && (item.startsWith('#') || item.startsWith('?') || item.split(/[?#]/)[0] === `/doc/${before.document_id}`)
        ? item.replace(/([#?&]block=)([^&#\s)]+)/gi, (whole: string, prefix: string, id: string) => ids.has(id) ? `${prefix}${ids.get(id)}` : whole) : rewrite(item)]));
    return value;
  };
  const copy = (block: DocumentBlock): NewBlock => ({ id: ids.get(block.id) ?? freshId(), type: block.type, props: rewrite(block.props) as NewBlock['props'], content: rewrite(block.content) as NewBlock['content'], children: ordered(subtree.filter((child) => child.parent_id === block.id)).map(copy) });
  return { kind: 'removed', ops: [{ op: 'insert', block: copy(before), parent_id: before.parent_id, after_id: predecessor(before, live) }] };
}
