/** Layout opens the same status editor and restores keyboard focus when it closes. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ArrangementMenu } from '@/features/workbench/Arrangements';
import { useWorkbench } from '@/features/workbench/store';
import { defaultStatus, moveStatus } from '@/features/status-items/arrangement';
import { show } from './support';

afterEach(() => useWorkbench.setState({ statusItems: defaultStatus() }));
it('opens Status items from Layout and edits the same saved places', async () => {
  const user = userEvent.setup();
  useWorkbench.setState({ statusItems: moveStatus(defaultStatus(), 'speed', 'header') });
  show(<ArrangementMenu screen="thread" current="conversation" onChoose={vi.fn()} onReset={vi.fn()} />);
  const layout = screen.getByRole('button', { name: 'Layout' });
  await user.click(layout); await user.click(screen.getByRole('menuitem', { name: 'Status items…' }));
  const editor = await screen.findByRole('dialog', { name: 'Status items' });
  await user.click(within(editor).getByRole('combobox', { name: 'Place' }));
  await user.click(screen.getByRole('option', { name: 'Header' }));
  expect(within(editor).getByRole('checkbox', { name: 'Output speed' })).toBeChecked();
  await user.click(within(editor).getByRole('checkbox', { name: 'Output speed' }));
  expect(useWorkbench.getState().statusItems.find((entry) => entry.id === 'speed')).toMatchObject({ place: 'header', shown: false });
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Status items' })).not.toBeInTheDocument());
  expect(layout).toHaveFocus();
});
