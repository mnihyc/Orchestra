/** The shared tab locator distinguishes paths, preserves dirty close scope and returns keyboard focus. */
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ViewGroup } from '@/ui/ViewGroup';
import { show } from './support';

it('filters duplicate names by path and closes only the addressed dirty tab without selecting it', async () => {
  const select = vi.fn(), close = vi.fn();
  function Group() {
    const [active, setActive] = useState('one');
    const entries = [{ id: 'one', title: 'Plan', path: 'Team A / Plan', unsaved: true }, { id: 'two', title: 'Plan', path: 'Team B / Plan' },
      ...Array.from({ length: 20 }, (_, index) => ({ id: `other-${index}`, title: `Other ${index}`, path: 'Elsewhere' }))];
    return <ViewGroup label="Main area" tabIds={entries.map((tab) => tab.id)} activeId={active}
      tabInfo={entries.map((tab) => ({ ...tab, onSelect: () => { select(tab.id); setActive(tab.id); }, onClose: () => { close(tab.id); } }))}
      tabElementId={(id) => `locator-tab-${id}`} panelId="locator-panel"
      renderTab={(id, state) => <button id={`locator-tab-${id}`} role="tab" aria-selected={state.active} tabIndex={state.tabbable ? 0 : -1}>{id}</button>}>
      <p>Retained working content</p>
    </ViewGroup>;
  }
  show(<Group />); const user = userEvent.setup();
  const firstTab = screen.getByRole('tab', { name: 'one' });
  await user.click(screen.getByRole('button', { name: 'Find tab in Main area' }));
  let chooser = screen.getByRole('dialog', { name: 'Find tab in Main area' });
  await user.type(within(chooser).getByRole('combobox', { name: 'Find an open tab' }), 'Team B');
  expect(within(chooser).getAllByRole('option')).toHaveLength(1);
  await user.keyboard('{Enter}'); expect(select).toHaveBeenCalledExactlyOnceWith('two');
  await waitFor(() => expect(screen.getByRole('tab', { name: 'two' })).toHaveFocus());
  await user.click(screen.getByRole('button', { name: 'Find tab in Main area' }));
  chooser = screen.getByRole('dialog', { name: 'Find tab in Main area' });
  await user.type(within(chooser).getByRole('combobox', { name: 'Find an open tab' }), 'Team A');
  const dirty = within(chooser).getByRole('option', { name: 'Plan, unsaved' }); expect(dirty.querySelector('[data-unsaved]')).not.toBeNull();
  await user.click(within(dirty).getByRole('button', { name: 'Close Plan' }));
  expect(close).toHaveBeenCalledExactlyOnceWith('one'); expect(select).toHaveBeenCalledTimes(1);
  // The caller can cancel its consolidated dirty-close dialog; the locator has not removed a tab itself.
  expect(firstTab).toBeInTheDocument();
  expect(firstTab).toHaveAttribute('aria-selected', 'false');
  await user.keyboard('{Escape}'); await waitFor(() => expect(screen.getByRole('button', { name: 'Find tab in Main area' })).toHaveFocus());
});
