/** A row's place among its siblings depends on identity and parent order, not its live content. */
interface PositionedRow {
  id: string;
  parentId: string | null;
}

export interface TreePositions {
  /** A small identity token lets row views recognize this index without retaining its maps. */
  stamp: object;
  sizes: ReadonlyMap<string | null, number>;
  position: ReadonlyMap<string, number>;
}

interface PositionsMemory {
  rows: readonly PositionedRow[];
  positions: TreePositions;
}

export const newPositionsMemory = (): PositionsMemory => ({ rows: [], positions: { stamp: {}, sizes: new Map(), position: new Map() } });

/** Reuse the index while only row content changes; keep one previous snapshot and linear storage. */
export function treePositions(rows: readonly PositionedRow[], memory: PositionsMemory): TreePositions {
  if (rows.length === memory.rows.length && rows.every((row, at) => {
    const held = memory.rows[at];
    return held?.id === row.id && held.parentId === row.parentId;
  })) return memory.positions;
  const sizes = new Map<string | null, number>();
  const position = new Map<string, number>();
  for (const row of rows) {
    const count = (sizes.get(row.parentId) ?? 0) + 1;
    sizes.set(row.parentId, count);
    position.set(row.id, count);
  }
  // Keep values rather than row references, so a replaced array also observes reparented objects.
  memory.rows = rows.map(({ id, parentId }) => ({ id, parentId }));
  memory.positions = { stamp: {}, sizes, position };
  return memory.positions;
}
