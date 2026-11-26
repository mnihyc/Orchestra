/** An open editor or pending act keeps its controller when a server page stops showing the row. */
import { useCallback, useMemo, useState } from 'react';

export function useRetainedRows<T extends { id: string }>(rows: readonly T[]) {
  const [held, setHeld] = useState<ReadonlyMap<string, T>>(new Map());
  const keep = useCallback((row: T, active: boolean) => setHeld((before) => {
    if (active ? before.get(row.id) === row : !before.has(row.id)) return before;
    const next = new Map(before);
    if (active) next.set(row.id, row); else next.delete(row.id);
    return next;
  }), []);
  const active = useMemo(() => new Set(held.keys()), [held]);
  const visible = new Map(rows.map((row) => [row.id, row]));
  return { active, rows: [...visible.values(), ...[...held.values()].filter((row) => !visible.has(row.id))], visible, keep };
}
