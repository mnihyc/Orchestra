/** All status places use the same checked menu, move action and drag/keyboard order editor. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { defaultStatus } from '@/features/status-items/arrangement';
import { STATUS_ITEMS, type StatusPlace as Place } from '@/features/status-items/registry';
import { useWorkbench } from '@/features/workbench/store';
import { i18n } from '@/i18n';
import { show } from './support';

vi.mock('@/features/status-items/useValues', () => ({ useStatusValues: (_thread: string, place: Place) => {
  const entries = useWorkbench((state) => state.statusItems);
  return entries.filter((entry) => entry.place === place && entry.shown).map((entry) => ({ id: entry.id, title: i18n.t(STATUS_ITEMS.find((item) => item.id === entry.id)?.title ?? 'statusItems.title'), value: { text: entry.id === 'speed' ? '48 tok/s' : `${entry.id} value` } }));
} }));
beforeEach(() => useWorkbench.setState({ statusItems: defaultStatus() }));
afterEach(() => useWorkbench.setState({ statusItems: defaultStatus() }));

it('keeps the bottom-bar editor available while no thread is in view', async () => {
  const user = userEvent.setup();
  show(<StatusPlace place="bar" />);
  const region = screen.getByRole('group', { name: 'Status bar status items' });
  fireEvent.contextMenu(region);
  await user.click(screen.getByRole('menuitem', { name: 'Customize…' }));
  expect(await screen.findByRole('list', { name: 'Item order' })).toBeVisible();
});

for (const place of ['bar', 'header', 'live'] as const) it(`shows, hides, moves and resets items from the ${place} context menu`, async () => {
  const user = userEvent.setup();
  show(<StatusPlace place={place} threadId="thread" />);
  const region = screen.getByRole('group', { name: `${i18n.t(`statusItems.places.${place}`)} status items` });
  fireEvent.contextMenu(region);
  const toggle = screen.getByRole('menuitemcheckbox', { name: 'Output speed' });
  if (toggle.getAttribute('aria-checked') === 'true') { await user.click(toggle); fireEvent.contextMenu(region); }
  await user.click(screen.getByRole('menuitemcheckbox', { name: 'Output speed' }));
  expect(useWorkbench.getState().statusItems.find((entry) => entry.id === 'speed')).toMatchObject({ place, shown: true });
  fireEvent.contextMenu(screen.getByRole('button', { name: 'Output speed' }));
  await user.hover(screen.getByRole('menuitem', { name: 'Move to' }));
  const destination = place === 'header' ? 'bar' : 'header';
  await user.click(await screen.findByRole('menuitem', { name: i18n.t(`statusItems.places.${destination}`) }));
  expect(useWorkbench.getState().statusItems.find((entry) => entry.id === 'speed')).toMatchObject({ place: destination, shown: true });
  fireEvent.contextMenu(region); await user.click(screen.getByRole('menuitem', { name: 'Customize…' }));
  await user.click(await screen.findByRole('button', { name: 'Reset' }));
  expect(useWorkbench.getState().statusItems).toEqual(defaultStatus());
});

it('orders the live items with Alt arrows and dragging, and returns focus on Escape', async () => {
  const user = userEvent.setup();
  show(<StatusPlace place="live" threadId="thread" />);
  const region = screen.getByRole('group', { name: 'Live line status items' });
  fireEvent.contextMenu(region); await user.click(screen.getByRole('menuitem', { name: 'Customize…' }));
  const list = await screen.findByRole('list', { name: 'Item order' });
  const rows = () => within(list).getAllByRole('listitem').map((row) => row.getAttribute('data-status-order'));
  const speed = within(list).getByRole('button', { name: 'Reorder Output speed' });
  act(() => speed.focus()); await user.keyboard('{Alt>}{ArrowDown}{/Alt}');
  expect(rows()).toEqual(['phase', 'elapsed', 'speed', 'tokens']);
  const first = within(list).getAllByRole('listitem')[0], last = within(list).getAllByRole('listitem')[3];
  if (first === undefined || last === undefined) throw new Error('The four live defaults must be listed');
  fireEvent.dragStart(last, { dataTransfer: { setData: vi.fn(), effectAllowed: 'move' } });
  fireEvent.dragOver(first); fireEvent.drop(first);
  expect(rows()).toEqual(['tokens', 'phase', 'elapsed', 'speed']);
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Status items' })).not.toBeInTheDocument());
  expect(region).toHaveFocus();
});
