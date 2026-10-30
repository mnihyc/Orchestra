/** The shared navigation controls and temporary bar retain ordinary pointer, keyboard and focus behavior. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { ArrangementSwitch, RecentOrderSelect } from '@/features/navigation/ArrangementSwitch';
import type { NavigationArrangement, RecentOrder } from '@/features/navigation/recent';
import { SlideOutBar } from '@/features/workbench/SlideOutBar';
import { useWorkbench } from '@/features/workbench/store';
import { makeContainer, makeGroup, makeTab, setPinned } from '@/features/workbench/layout';
import { tabEntries } from '@/features/workbench/Tabs';
import { ViewTab } from '@/ui/ViewTab';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const before = useWorkbench.getState();
afterEach(async () => { cleanup(); vi.useRealTimers(); useWorkbench.setState(before); await setLocale('en'); });

function Controls() {
  const [view, choose] = useState<NavigationArrangement>('tree');
  const [order, setOrder] = useState<RecentOrder>('opened');
  return <><ArrangementSwitch value={view} onChange={choose} />{view === 'recent' && <RecentOrderSelect value={order} onChange={setOrder} />}</>;
}
it.each(['en', 'zh-CN'] as const)('switches arrangement and order with short named controls in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<Controls />);
  expect(screen.getByRole('button', { name: i18n.t('navigation.arrangement.tree') })).toHaveAttribute('aria-pressed', 'true');
  await user.click(screen.getByRole('button', { name: i18n.t('navigation.arrangement.recent') }));
  const order = screen.getByRole('combobox', { name: i18n.t('navigation.recentOrder.label') });
  expect(order).toHaveTextContent(i18n.t('navigation.recentOrder.opened'));
  await user.click(order);
  await user.keyboard('{ArrowDown}{Enter}');
  expect(order).toHaveTextContent(i18n.t('navigation.recentOrder.active'));
});

function Floating() {
  const [open, setOpen] = useState(false);
  return <><button onClick={() => setOpen(true)}>Open navigation</button><p data-page>Page</p>
    <SlideOutBar bar="left" size={280} open={open} docked={false} onClose={() => setOpen(false)}><a href="#document">Open document</a></SlideOutBar></>;
}
it('opens a temporary bar by keyboard and restores focus on Escape without docking it', async () => {
  const user = userEvent.setup();
  show(<Floating />);
  const trigger = screen.getByRole('button', { name: 'Open navigation' });
  trigger.focus(); await user.keyboard('{Enter}');
  expect(await screen.findByRole('dialog', { name: i18n.t('workbench.areas.left') })).toBeVisible();
  await waitFor(() => expect(screen.getByRole('link', { name: 'Open document' })).toHaveFocus());
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(trigger).toHaveFocus();
});

it('gives a leaving pointer its grace period and keeps the bar while the pointer comes back', async () => {
  const user = userEvent.setup();
  show(<Floating />);
  await user.click(screen.getByRole('button', { name: 'Open navigation' }));
  const bar = await screen.findByRole('dialog');
  vi.useFakeTimers();
  fireEvent.pointerLeave(bar, { pointerType: 'mouse' });
  await act(() => vi.advanceTimersByTimeAsync(299));
  expect(bar).toBeVisible();
  fireEvent.pointerEnter(bar, { pointerType: 'mouse' });
  await act(() => vi.advanceTimersByTimeAsync(301));
  expect(bar).toBeVisible();
  fireEvent.pointerLeave(bar, { pointerType: 'mouse' });
  await act(() => vi.advanceTimersByTimeAsync(301));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

function Tab() {
  const layout = useWorkbench((state) => state.layout);
  const change = useWorkbench((state) => state.change);
  const tab = layout.main.kind === 'group' ? layout.main.tabs[0] : undefined;
  if (tab === undefined) return null;
  return <div role="tablist"><ViewTab id="everything" panelId="page" active tabbable title="Everything" icon={null} pinned={tab.pinned === true} closable={tab.pinned !== true}
    menu={tabEntries(layout, tab.id, i18n.t, change)} onSelect={() => undefined} onClose={() => useWorkbench.getState().close({ kind: 'tab', tab: tab.id })} /></div>;
}
it('exposes Pin in both tab menus and closes a pin only through its explicit Close', async () => {
  useWorkbench.setState({ layout: { ...before.layout, main: makeGroup([{ id: 'everything', view: 'page', of: { href: '/everything' } }]) } });
  const user = userEvent.setup();
  show(<Tab />);
  await user.click(screen.getByRole('button', { name: i18n.t('workbench.tab.menu', { title: 'Everything' }) }));
  await user.click(screen.getByRole('menuitem', { name: 'Pin' }));
  expect(document.querySelector('[data-pinned]')).not.toBeNull();
  expect(screen.queryByRole('button', { name: i18n.t('workbench.tab.closeNamed', { title: 'Everything' }) })).not.toBeInTheDocument();
  const tab = screen.getByRole('tab'); tab.focus();
  await user.keyboard('{Delete}');
  expect(screen.getByRole('tab')).toBeVisible();
  await user.keyboard('{Shift>}{F10}{/Shift}');
  expect(await screen.findByRole('menuitem', { name: 'Unpin' })).toBeVisible();
  await user.click(screen.getByRole('menuitem', { name: i18n.t('workbench.tab.close') }));
  expect(screen.queryByRole('tab')).not.toBeInTheDocument();
});

it.each(['left', 'right'] as const)('a hidden %s bar shortcut docks it and the next shortcut hides it', (bar) => {
  const group = makeGroup([{ id: 'navigation', view: 'navigation' }]);
  useWorkbench.setState({ layout: { ...before.layout, [bar]: { open: false, size: 280, active: 'navigation', containers: [{ id: group.id, name: 'navigation', active: 'navigation', sections: [{ tab: group.tabs[0]!, open: true, hidden: false, size: 1 }] }] } } });
  useWorkbench.getState().revealBar(bar, true);
  expect(useWorkbench.getState().overlay).toBeNull();
  expect(useWorkbench.getState().layout[bar].open).toBe(true);
  useWorkbench.getState().revealBar(bar, true);
  expect(useWorkbench.getState().layout[bar].open).toBe(false);
  expect(useWorkbench.getState().overlay).toBeNull();
  const pinned = setPinned(useWorkbench.getState().layout, 'navigation', true);
  expect(pinned[bar].containers[0]?.sections[0]?.tab.pinned).toBe(true);
});

it.each(['left', 'right'] as const)('docks a %s edge peek by shortcut and clears it before hiding again', (bar) => {
  useWorkbench.setState({ overlay: bar, layout: { ...before.layout, [bar]: { open: false, size: 280, active: 'navigation', containers: [makeContainer('navigation', [makeTab('navigation')])] } } });
  useWorkbench.getState().revealBar(bar, true);
  expect(useWorkbench.getState().layout[bar].open).toBe(true);
  expect(useWorkbench.getState().overlay).toBeNull();
  useWorkbench.getState().toggle(bar, true);
  expect(useWorkbench.getState().layout[bar].open).toBe(false);
  expect(useWorkbench.getState().overlay).toBeNull();
});

it('keeps the right-bar sheet at medium width and never opens an empty bar', () => {
  useWorkbench.setState({ layout: { ...before.layout, right: { open: false, size: 280, active: 'navigation', containers: [makeContainer('navigation', [makeTab('navigation')])] } } });
  const layout = useWorkbench.getState().layout;
  useWorkbench.getState().revealBar('right', false);
  expect(useWorkbench.getState().overlay).toBe('right');
  expect(useWorkbench.getState().layout).toBe(layout);
  useWorkbench.getState().revealBar('right', false);
  expect(useWorkbench.getState().overlay).toBeNull();
  useWorkbench.setState({ layout: { ...layout, right: { ...layout.right, active: null, containers: [] } } });
  useWorkbench.getState().revealBar('right', true);
  expect(useWorkbench.getState().layout.right.open).toBe(false);
  useWorkbench.getState().revealBar('right', false);
  expect(useWorkbench.getState().overlay).toBeNull();
});
