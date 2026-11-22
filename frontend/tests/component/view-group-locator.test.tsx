/** The shared tab locator distinguishes paths, preserves dirty close scope and returns keyboard focus. */
import { QueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';
import { screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { namesKey } from '@/api/search';
import { TabLocator } from '@/ui/TabLocator';
import { ViewGroup } from '@/ui/ViewGroup';
import { show } from './support';

it('filters duplicate names by path and closes only the addressed dirty tab without selecting it', async () => {
  const select = vi.fn(), close = vi.fn();
  const path = vi.fn((group: string) => `Team ${group} / Plan`);
  function Group() {
    const [active, setActive] = useState('one');
    const entries = [{ id: 'one', title: 'Plan', group: 'A', unsaved: true }, { id: 'two', title: 'Plan', group: 'B' },
      ...Array.from({ length: 20 }, (_, index) => ({ id: `other-${index}`, title: `Other ${index}`, group: 'Elsewhere' }))];
    return <ViewGroup label="Main area" tabIds={entries.map((tab) => tab.id)} activeId={active}
      tabInfo={entries.map((tab) => ({ ...tab, get path() { return path(tab.group); }, onSelect: () => { select(tab.id); setActive(tab.id); }, onClose: () => { close(tab.id); } }))}
      tabElementId={(id) => `locator-tab-${id}`} panelId="locator-panel"
      renderTab={(id, state) => <button id={`locator-tab-${id}`} role="tab" aria-selected={state.active} tabIndex={state.tabbable ? 0 : -1}>{id}</button>}>
      <p>Retained working content</p>
    </ViewGroup>;
  }
  show(<Group />); const user = userEvent.setup();
  expect(path).not.toHaveBeenCalled();
  const firstTab = screen.getByRole('tab', { name: 'one' });
  await user.click(screen.getByRole('button', { name: 'Find tab in Main area' }));
  let chooser = screen.getByRole('dialog', { name: 'Find tab in Main area' });
  const field = within(chooser).getByRole('combobox', { name: 'Find an open tab' });
  expect(field).toHaveFocus();
  await user.keyboard('{End}{ArrowDown}');
  expect(field).toHaveAttribute('aria-activedescendant', within(chooser).getByRole('option', { name: 'Plan, unsaved' }).id);
  await user.keyboard('{ArrowUp}');
  expect(field).toHaveAttribute('aria-activedescendant', within(chooser).getByRole('option', { name: 'Other 19' }).id);
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

it('offers only open tabs of the matching kind when a restored tab has no title', async () => {
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  client.setQueryData(namesKey({ q: 'North', kinds: ['thread'] }), { partial: false, items: [
    { kind: 'thread', id: 'open', name: 'North work' },
    { kind: 'thread', id: 'closed', name: 'North archive' },
    { kind: 'agent', id: 'other', name: 'North agent' },
  ] });
  const select = vi.fn<(id: string) => void>();
  const tabs = ['open', 'other'].map((id) => ({ id, title: 'Thread', unnamedTarget: { kind: 'thread' as const, id },
    onSelect: () => select(id), onClose: () => undefined }));
  show(<TabLocator label="Main area" activeId={null} tabElementId={(id) => id} tabs={tabs} />, client);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Find tab in Main area' }));
  await user.type(screen.getByRole('combobox'), 'North');
  await screen.findByRole('option', { name: 'North work' });
  expect(screen.getAllByRole('option')).toHaveLength(1);
  await user.keyboard('{Enter}');
  expect(select).toHaveBeenCalledExactlyOnceWith('open');
});
