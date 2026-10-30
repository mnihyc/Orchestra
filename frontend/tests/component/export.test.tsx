/** A tree of ticks, as the dialog of a place draws it: a box on each row that is ticked, empty or half ticked, pressed or spaced to change, and one that cannot be changed. */
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { Tree, type TreeRow } from '@/ui/Tree';

// A keyboard typed into a tree of rows takes long on a machine that is busy with other work.
vi.setConfig({ testTimeout: 30_000 });

interface Thing {
  name: string;
}

const row = (id: string, depth: number, parentId: string | null, more: Partial<TreeRow<Thing>> = {}): TreeRow<Thing> => ({
  id,
  depth,
  parentId,
  expandable: false,
  expanded: false,
  item: { name: id },
  ...more,
});

const ROWS: TreeRow<Thing>[] = [
  row('web', 0, null, { expandable: true, expanded: true, tick: 'mixed' }),
  row('checkout', 1, 'web', { tick: true }),
  row('payments', 1, 'web', { tick: false }),
  row('secret', 1, 'web', { tick: false, tickLocked: true }),
  row('plain', 1, 'web'),
];

function draw(onTick: (row: TreeRow<Thing>, on: boolean) => void, onExpandedChange: (id: string, open: boolean) => void = () => undefined) {
  return render(<Tree label="What to export" rows={ROWS} onExpandedChange={onExpandedChange} onTick={onTick} nameOf={(thing) => thing.name} renderRow={(one) => <span>{one.item.name}</span>} />);
}

describe('a tree of ticks', () => {
  it('says to a reader which rows are ticked, empty and half ticked, and which have no box', () => {
    draw(() => undefined);
    expect(screen.getByRole('treeitem', { name: /web/ })).toHaveAttribute('aria-checked', 'mixed');
    expect(screen.getByRole('treeitem', { name: /checkout/ })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('treeitem', { name: /payments/ })).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByRole('treeitem', { name: /plain/ })).not.toHaveAttribute('aria-checked');
  });

  it('asks for the other state when a row is pressed: a half ticked row is ticked, a ticked one cleared', async () => {
    const asked = vi.fn<(row: TreeRow<Thing>, on: boolean) => void>();
    draw(asked);
    await userEvent.click(screen.getByText('payments'));
    await userEvent.click(screen.getByText('checkout'));
    await userEvent.click(screen.getByText('web'));
    expect(asked.mock.calls.map(([one, on]) => [one.id, on])).toEqual([
      ['payments', true],
      ['checkout', false],
      ['web', true],
    ]);
  });

  it('ticks the row in focus with the space bar, and still opens and closes it with the arrows', async () => {
    const asked = vi.fn<(row: TreeRow<Thing>, on: boolean) => void>();
    const opened = vi.fn<(id: string, open: boolean) => void>();
    draw(asked, opened);
    screen.getByRole('treeitem', { name: /web/ }).focus();
    await userEvent.keyboard('{ArrowDown}{ }');
    expect(asked).toHaveBeenCalledWith(expect.objectContaining({ id: 'checkout' }), false);
    await userEvent.keyboard('{ArrowUp}{ArrowLeft}');
    expect(opened).toHaveBeenCalledWith('web', false);
  });

  it('leaves a locked row as it is, whether it is pressed or spaced', async () => {
    const asked = vi.fn<(row: TreeRow<Thing>, on: boolean) => void>();
    draw(asked);
    await userEvent.click(screen.getByText('secret'));
    screen.getByRole('treeitem', { name: /secret/ }).focus();
    await userEvent.keyboard('{ }');
    expect(asked).not.toHaveBeenCalled();
  });

  it('still opens a row with the space bar where the tree has no ticks', async () => {
    const opened = vi.fn<(id: string, open: boolean) => void>();
    render(<Tree label="Plain" rows={[row('group', 0, null, { expandable: true })]} onExpandedChange={opened} nameOf={(thing) => thing.name} renderRow={(one) => <span>{one.item.name}</span>} />);
    screen.getByRole('treeitem').focus();
    await userEvent.keyboard('{ }');
    expect(opened).toHaveBeenCalledWith('group', true);
  });
});
