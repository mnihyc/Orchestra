/** Picking rows as file managers do: a box ticks one, Shift ticks the range from the last one touched, Ctrl or ⌘ adds or
 * removes one row, the head box ticks every loaded row, and the bar says the count with Select all and Clear. */
import { act, cleanup, renderHook, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { RowBox, SelectedBar, selectRange, useRowSelection } from '@/ui/RowSelection';
import { show } from './support';

const rows = ['a', 'b', 'c', 'd', 'e'];
const click = (on: Partial<{ shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }>) => ({ shiftKey: false, ctrlKey: false, metaKey: false, ...on });

afterEach(cleanup);

describe('the picking rules', () => {
  it('a range runs from the last row touched among the loaded rows and keeps ticks elsewhere', () => {
    expect([...selectRange(new Set(['x']), rows, 'b', 'd', true, true)]).toEqual(['x', 'b', 'c', 'd']);
    expect([...selectRange(new Set(['b', 'c', 'd']), rows, 'b', 'c', false, true)]).toEqual(['d']);
    expect([...selectRange(new Set(), rows, 'gone', 'c', true, true)]).toEqual(['c']);
  });

  it('a plain click opens: only Shift, Ctrl and ⌘ clicks pick', () => {
    const { result } = renderHook(({ list }) => useRowSelection(list, (row) => row), { initialProps: { list: rows } });
    expect(result.current.click('a', click({}))).toBe(false);
    act(() => { result.current.click('b', click({ ctrlKey: true })); });
    act(() => { result.current.click('d', click({ shiftKey: true })); });
    expect(result.current.selected).toEqual(['b', 'c', 'd']);
    act(() => { result.current.click('c', click({ metaKey: true })); });
    expect(result.current.selected).toEqual(['b', 'd']);
    expect(result.current.all).toBe('indeterminate');
    act(() => { result.current.toggleAll(true); });
    expect(result.current.all).toBe(true);
    act(() => { result.current.clear(); });
    expect(result.current.selected).toEqual([]);
  });

  it('a row that leaves the list leaves the selection', () => {
    const { result, rerender } = renderHook(({ list }) => useRowSelection(list, (row) => row), { initialProps: { list: rows } });
    act(() => { result.current.toggle('a', true); result.current.toggle('e', true); });
    rerender({ list: ['a', 'b'] });
    expect(result.current.selected).toEqual(['a']);
  });
});

describe('the boxes and the bar', () => {
  it('Shift with a box asks for the range', async () => {
    const onChange = vi.fn();
    show(<RowBox name="Plans" checked={false} onChange={onChange} />);
    // One session of the keyboard and pointer, so that the held Shift reaches the click.
    const user = userEvent.setup();
    await user.keyboard('{Shift>}');
    await user.click(screen.getByRole('checkbox', { name: 'Select Plans' }));
    await user.keyboard('{/Shift}');
    expect(onChange).toHaveBeenCalledWith(true, true);
    await user.click(screen.getByRole('checkbox', { name: 'Select Plans' }));
    expect(onChange).toHaveBeenLastCalledWith(true, false);
  });

  it('says the count and offers Select all until every loaded row is ticked', async () => {
    const all = vi.fn();
    const clear = vi.fn();
    const { rerender } = show(<SelectedBar count={2} total={5} onSelectAll={all} onClear={clear}><button type="button">Delete</button></SelectedBar>);
    expect(screen.getByRole('toolbar')).toHaveTextContent('2 selected');
    await userEvent.click(screen.getByRole('button', { name: 'Select all' }));
    expect(all).toHaveBeenCalledOnce();
    await userEvent.click(screen.getByRole('button', { name: 'Clear selection' }));
    expect(clear).toHaveBeenCalledOnce();
    rerender(<SelectedBar count={5} total={5} onSelectAll={all} onClear={clear} />);
    expect(screen.queryByRole('button', { name: 'Select all' })).not.toBeInTheDocument();
  });
});
