/** Workbench tabs announce the same complete menu opened by pointer or keyboard. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { expect, it, vi } from 'vitest';

import { ViewGroup } from '@/ui/ViewGroup';
import { ViewTab } from '@/ui/ViewTab';
import { show } from './support';

it('announces its popup, retains both menu entrances and returns pointer-menu focus to the tab', async () => {
  const pin = vi.fn(), close = vi.fn(), select = vi.fn();
  const entries = [{ id: 'pin', label: 'Pin', onSelect: pin }, { id: 'close', label: 'Close', onSelect: close }];
  show(<ViewGroup label="Main area" tabIds={['work']} activeId="work" panelId="work-panel" tabElementId={() => 'work-tab'}
    tabInfo={[{ id: 'work', title: 'Work', onSelect: select, onClose: close }]}
    renderTab={(_id, state) => <ViewTab id="work-tab" panelId="work-panel" title="Work" icon={null}
      active={state.active} tabbable={state.tabbable} menu={entries} onSelect={select} onClose={close} />}>
    Working content
  </ViewGroup>);
  const user = userEvent.setup();
  const tab = screen.getByRole('tab', { name: 'Work' });
  expect(tab).toHaveAttribute('aria-haspopup', 'menu');

  await user.click(screen.getByLabelText('Menu of Work'));
  let menu = screen.getByRole('menu', { name: 'Menu of Work' });
  expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent)).toEqual(['Pin', 'Close']);
  await user.keyboard('{Escape}');
  await waitFor(() => expect(tab).toHaveFocus());

  await user.keyboard('{Shift>}{F10}{/Shift}');
  menu = screen.getByRole('menu', { name: 'Menu of Work' });
  expect(within(menu).getAllByRole('menuitem').map((item) => item.textContent)).toEqual(['Pin', 'Close']);
  await user.click(within(menu).getByRole('menuitem', { name: 'Pin' }));
  expect(pin).toHaveBeenCalledExactlyOnceWith();
  expect(close).not.toHaveBeenCalled();
  expect(select).not.toHaveBeenCalled();
});

for (const entrance of ['pointer', 'keyboard'] as const) it(`returns focus to the selected neighbor when the ${entrance} menu closes its tab`, async () => {
  function Group() {
    const [tabs, setTabs] = useState(['First', 'Second']);
    const [active, setActive] = useState('Second');
    const close = (id: string) => { setTabs((before) => before.filter((tab) => tab !== id)); setActive('First'); };
    return <ViewGroup label="Main area" tabIds={tabs} activeId={active} panelId="close-panel" tabElementId={(id) => `close-${id}`}
      tabInfo={tabs.map((id) => ({ id, title: id, onSelect: () => setActive(id), onClose: () => close(id) }))}
      renderTab={(id, state) => <ViewTab id={`close-${id}`} panelId="close-panel" title={id} icon={null}
        active={state.active} tabbable={state.tabbable} menu={[{ id: 'close', label: 'Close', onSelect: () => close(id) }]}
        onSelect={() => setActive(id)} onClose={() => close(id)} />}>
      Working content
    </ViewGroup>;
  }
  show(<Group />);
  const user = userEvent.setup();
  const tab = screen.getByRole('tab', { name: 'Second' });
  if (entrance === 'pointer') await user.click(screen.getByLabelText('Menu of Second'));
  else { tab.focus(); await user.keyboard('{Shift>}{F10}{/Shift}'); }
  const close = within(screen.getByRole('menu', { name: 'Menu of Second' })).getByRole('menuitem', { name: 'Close' });
  close.focus();
  await user.keyboard('{Enter}');
  expect(screen.queryByRole('tab', { name: 'Second' })).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByRole('tab', { name: 'First' })).toHaveFocus());
});
