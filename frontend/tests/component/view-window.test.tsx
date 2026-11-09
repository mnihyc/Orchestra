/** A movable view retains one draft and one visible opening, with geometry owned by its viewer. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import { useEffect, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { makeTab } from '@/features/workbench/layout';
import { ViewWindow, ViewWindowLease } from '@/features/workbench/ViewWindow';
import { clearViewOpenings, openingForView, shownViewOpening, showViewOpening } from '@/features/workbench/viewOpening';
import { i18n, setLocale } from '@/i18n';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { show } from './support';

const savedSession = client.session.getState();
const savedViewport = { width: innerWidth, height: innerHeight };
beforeEach(() => {
  localStorage.clear(); clearViewOpenings(); client.session.setState({ userId: 'window-viewer-a', status: 'signed_in' });
  Object.defineProperties(window, { innerWidth: { configurable: true, value: 1440 }, innerHeight: { configurable: true, value: 900 } });
});
afterEach(async () => {
  cleanup(); clearViewOpenings(); client.session.setState(savedSession); localStorage.clear();
  Object.defineProperties(window, { innerWidth: { configurable: true, value: savedViewport.width }, innerHeight: { configurable: true, value: savedViewport.height } });
  await setLocale('en');
});

function Editor({ mounted }: { mounted: () => void }) {
  const [text, setText] = useState('');
  useEffect(mounted, [mounted]);
  return <input aria-label="Window draft" value={text} onChange={(event) => setText(event.target.value)} />;
}
const settle = () => act(async () => { await Promise.resolve(); });

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the same editor mounted while it floats, maximizes and docks (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale), mounted = vi.fn();
  show(<ViewWindow name="probe" title="Probe window" onClose={() => undefined}><Editor mounted={mounted} /></ViewWindow>);
  const field = screen.getByRole('textbox', { name: 'Window draft' }); fireEvent.change(field, { target: { value: 'A question kept in place' } });
  fireEvent.click(screen.getByRole('button', { name: t('viewWindow.popOut') }));
  const floating = await screen.findByRole('dialog', { name: 'Probe window' });
  expect(screen.getByRole('textbox')).toBe(field); expect(field).toHaveValue('A question kept in place');
  fireEvent.click(within(floating).getByRole('button', { name: t('viewWindow.maximize') }));
  expect(floating.style.width).toContain('100dvw');
  fireEvent.click(within(floating).getByRole('button', { name: t('viewWindow.restore') }));
  fireEvent.click(within(floating).getByRole('button', { name: t('viewWindow.dock') }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Probe window' })).not.toBeInTheDocument());
  expect(screen.getByRole('textbox')).toBe(field); expect(field).toHaveValue('A question kept in place'); expect(mounted).toHaveBeenCalledOnce();
});

it('restores only this viewer’s placement and size after a remount', async () => {
  const t = i18n.getFixedT('en');
  let view = show(<ViewWindow name="probe" title="Probe window" onClose={() => undefined}><span>Body</span></ViewWindow>);
  fireEvent.click(screen.getByRole('button', { name: t('viewWindow.popOut') }));
  const floating = await screen.findByRole('dialog', { name: 'Probe window' });
  const width = Number.parseFloat(floating.style.width);
  fireEvent.keyDown(within(floating).getByRole('button', { name: t('viewWindow.resize', { name: 'Probe window' }) }), { key: 'ArrowRight' });
  expect(Number.parseFloat(floating.style.width)).toBe(width + 8);
  const stored = readViewChoice('window-viewer-a', 'view-window', 'geometry', '', 'probe');
  expect(JSON.parse(stored).width).toBe(width + 8);
  view.unmount();
  view = show(<ViewWindow name="probe" title="Probe window" onClose={() => undefined}><span>Body</span></ViewWindow>);
  expect(Number.parseFloat(screen.getByRole('dialog', { name: 'Probe window' }).style.width)).toBe(width + 8);
  view.unmount(); act(() => client.session.setState({ userId: 'window-viewer-b' }));
  show(<ViewWindow name="probe" title="Probe window" onClose={() => undefined}><span>Body</span></ViewWindow>);
  expect(screen.queryByRole('dialog', { name: 'Probe window' })).not.toBeInTheDocument();
  expect(readViewChoice('window-viewer-b', 'view-window', 'geometry', '', 'probe')).toBe('');
});

it('keeps a floating opening when its dock closes, then releases it when the view closes', async () => {
  const tab = makeTab('ask'), route = openingForView(tab.id);
  writeViewChoice('window-viewer-a', 'view-window', 'floating', true, 'ask');
  const closeDock = showViewOpening(tab.id, route);
  const view = show(<ViewWindowLease tab={tab} />); await settle();
  const opening = shownViewOpening(tab.id); expect(opening).not.toBeNull();
  act(closeDock); await settle(); expect(shownViewOpening(tab.id)).toBe(opening);
  view.unmount(); await settle(); expect(shownViewOpening(tab.id)).toBeNull();
});

it('does not admit a restored floating view while the workspace is pending', async () => {
  const tab = makeTab('ask'); writeViewChoice('window-viewer-a', 'view-window', 'floating', true, 'ask');
  const view = show(<FrameReadiness.Provider value={{ ready: false, pending: null }}><ViewWindowLease tab={tab} /></FrameReadiness.Provider>);
  await settle(); expect(shownViewOpening(tab.id)).toBeNull();
  view.rerender(<FrameReadiness.Provider value={{ ready: true, pending: null }}><ViewWindowLease tab={tab} /></FrameReadiness.Provider>);
  await settle(); expect(shownViewOpening(tab.id)).not.toBeNull();
});
