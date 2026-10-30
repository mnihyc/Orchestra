/** A tree of hundreds of rows: a row whose thing did not change is not drawn again, and opening a row draws the rows it adds and not the rows after them. */
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Tree, type TreeRow } from '@/ui/Tree';

interface Thing {
  name: string;
  mark: number;
}

/** Ten groups of thirty and their heads: three hundred and ten rows, which is hundreds, and what a jsdom draws in about a second a test where twenty groups took three and failed the five seconds of a loaded machine. */
const GROUPS = 10;
const IN_GROUP = 30;
/** The group the tests close and open in the middle, and the one after it. */
const MIDDLE = GROUPS / 2;

/** The things a tree is of, the same object for a row until its mark changes, as the tree of the left bar keeps them. */
class Things {
  private readonly held = new Map<string, Thing>();

  of(id: string, mark: number): Thing {
    const held = this.held.get(id);
    if (held?.mark === mark) return held;
    const made = { name: id, mark };
    this.held.set(id, made);
    return made;
  }
}

/** The groups of thirty, the groups in `closed` shut, and the marks of the rows that were changed. */
function rowsOf(things: Things, closed: ReadonlySet<number> = new Set(), marks: ReadonlyMap<string, number> = new Map()): TreeRow<Thing>[] {
  const rows: TreeRow<Thing>[] = [];
  for (let group = 0; group < GROUPS; group += 1) {
    const id = `group-${group}`;
    const open = !closed.has(group);
    rows.push({ id, depth: 0, parentId: null, expandable: true, expanded: open, item: things.of(id, marks.get(id) ?? 0) });
    if (!open) continue;
    for (let at = 0; at < IN_GROUP; at += 1) {
      const child = `${id}-${at}`;
      rows.push({ id: child, depth: 1, parentId: id, expandable: false, expanded: false, item: things.of(child, marks.get(child) ?? 0) });
    }
  }
  return rows;
}

describe('a tree of hundreds of rows', () => {
  /** What was drawn since the count was last emptied, by row. */
  const drawn = new Map<string, number>();
  const renderRow = (row: TreeRow<Thing>) => {
    drawn.set(row.id, (drawn.get(row.id) ?? 0) + 1);
    return <span>{row.item.name}</span>;
  };
  const nameOf = (thing: Thing) => thing.name;
  const onExpandedChange = () => undefined;
  const tree = (rows: TreeRow<Thing>[]) => <Tree label="Things" rows={rows} onExpandedChange={onExpandedChange} nameOf={nameOf} renderRow={renderRow} />;

  it('shows every row to a reader as an item of the tree, with its place among its brothers', () => {
    drawn.clear();
    const { container } = render(tree(rowsOf(new Things())));

    expect(container.querySelector('[role="tree"]')?.getAttribute('aria-label')).toBe('Things');
    expect(container.querySelectorAll('[role="tree"] [role="treeitem"]')).toHaveLength(GROUPS * (IN_GROUP + 1));
    const third = screen.getByText('group-4-2').closest('[role="treeitem"]');
    expect(third?.getAttribute('aria-level')).toBe('2');
    expect(third?.getAttribute('aria-posinset')).toBe('3');
    expect(third?.getAttribute('aria-setsize')).toBe(String(IN_GROUP));
  });

  it('draws again the one row whose thing changed, and no other', () => {
    drawn.clear();
    const things = new Things();
    const { rerender } = render(tree(rowsOf(things)));
    expect(drawn.size).toBe(GROUPS * (IN_GROUP + 1));

    drawn.clear();
    rerender(tree(rowsOf(things, new Set(), new Map([['group-7-12', 1]]))));

    expect([...drawn.entries()]).toEqual([['group-7-12', 1]]);
    expect(screen.getByText('group-7-12')).toBeTruthy();
  });

  it('draws the rows that an opened row adds and a few more, not the hundreds of rows after it', () => {
    drawn.clear();
    const things = new Things();
    const { container, rerender } = render(tree(rowsOf(things, new Set([2]))));
    const items = () => container.querySelectorAll('[role="tree"] [role="treeitem"]').length;
    const before = items();
    expect(before).toBe(GROUPS + (GROUPS - 1) * IN_GROUP);

    drawn.clear();
    rerender(tree(rowsOf(things)));

    expect(items()).toBe(before + IN_GROUP);
    // The thirty new rows and the row opened, and at most the rows of the run they split, drawn anew where they now stand.
    expect(drawn.size).toBeGreaterThanOrEqual(IN_GROUP + 1);
    expect(drawn.size).toBeLessThan(IN_GROUP + 1 + 240);
    for (let at = 0; at < IN_GROUP; at += 1) expect(drawn.get(`group-2-${at}`), `group-2-${at}`).toBe(1);
    // Rows far after it were not looked at.
    expect(drawn.has(`group-${GROUPS - 1}-${IN_GROUP - 1}`)).toBe(false);
    expect(drawn.has(`group-${GROUPS - 3}-3`)).toBe(false);
  });

  it('draws the row that was closed and few others when a row is closed, and shows the rows after it where they were', () => {
    drawn.clear();
    const things = new Things();
    const { rerender } = render(tree(rowsOf(things)));

    drawn.clear();
    rerender(tree(rowsOf(things, new Set([MIDDLE]))));
    // The row that was closed is drawn again, and at most the rows of the runs that its rows were in; nothing is drawn for the rows that went.
    expect(drawn.has(`group-${MIDDLE}`)).toBe(true);
    expect(screen.queryByText(`group-${MIDDLE}-4`)).toBeNull();
    expect(screen.getByText(`group-${MIDDLE + 1}-4`)).toBeTruthy();
    expect(drawn.size).toBeLessThan(240);
  });
});
