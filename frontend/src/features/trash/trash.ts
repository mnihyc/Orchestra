/** Moving things to their place's trash at once, and putting back exactly what one act moved.
 *
 * Deleting is no question asked first: what is deleted goes to the trash of its place, where it stays until its
 * retention ends, and the act is said in one line with Show and Undo (`tip.tsx`). Each thing is its own server act,
 * applied one after another so that one refusal never stops the rest and is named in the result. Only deleting for
 * good (a purge) still asks.
 */

/** What one act did to each thing it was given: moved (with what its Undo needs) or refused (with why). */
export interface TrashResult<Item, Receipt> {
  done: { item: Item; receipt: Receipt }[];
  failed: { item: Item; error: unknown }[];
}

/** Applies `act` to every item, one after another, and keeps going past a refusal. */
export async function eachInTurn<Item, Receipt>(items: readonly Item[], act: (item: Item) => Promise<Receipt>): Promise<TrashResult<Item, Receipt>> {
  const result: TrashResult<Item, Receipt> = { done: [], failed: [] };
  for (const item of items) {
    try {
      result.done.push({ item, receipt: await act(item) });
    } catch (error) {
      result.failed.push({ item, error });
    }
  }
  return result;
}

/** The selected things that are not inside another selected thing: deleting the outer one takes the inner one with it. */
export function outermost<Item>(items: readonly Item[], idOf: (item: Item) => string, ancestorsOf: (item: Item) => readonly string[]): Item[] {
  const chosen = new Set(items.map(idOf));
  const seen = new Set<string>();
  return items.filter((item) => {
    const id = idOf(item);
    if (seen.has(id)) return false;
    seen.add(id);
    return !ancestorsOf(item).some((above) => above !== id && chosen.has(above));
  });
}

/** Splits a selection into runs of at most `size`, the most one server act takes. */
export function chunks<Item>(items: readonly Item[], size: number): Item[][] {
  const out: Item[][] = [];
  for (let index = 0; index < items.length; index += size) out.push(items.slice(index, index + size));
  return out;
}
