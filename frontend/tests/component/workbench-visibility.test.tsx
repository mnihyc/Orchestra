/** A hidden view runs no reader or subscription, and a shown view keeps its draft across hiding and moving. */
import { useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { useEffect, useLayoutEffect, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { forgetViewOpening, showViewOpening, useViewShown, ViewOpening } from '@/features/workbench/viewOpening';
import { clearScreenHeading, currentHeading, useCurrentHeading, useScreenHeading } from '@/shell/screen';
import { show } from './support';

const KEY = 'visibility-probe';
const read = vi.fn(() => Promise.resolve('Current data'));
const subscribe = vi.fn();
const unsubscribe = vi.fn();
const renderBody = vi.fn();
vi.mock('@/features/navigation/placeLabels', () => ({ useNodeLabel: (_id: string | undefined, title: string) => title }));

function Body() {
  renderBody();
  const shown = useViewShown();
  const data = useQuery({ queryKey: [KEY], queryFn: read, enabled: shown, staleTime: Infinity });
  const [draft, setDraft] = useState('');
  useEffect(() => {
    if (!shown) return;
    subscribe();
    return () => { unsubscribe(); };
  }, [shown]);
  return <><p>{data.data}</p><input aria-label="Draft" value={draft} onChange={(event) => setDraft(event.target.value)} /></>;
}
function Slot() {
  useLayoutEffect(() => showViewOpening(KEY, 'route'), []);
  return null;
}
function Frame({ shown, position = 'main' }: { shown: boolean; position?: string }) {
  return <><ViewOpening view={KEY} route="route"><Body /></ViewOpening>{shown && <Slot key={position} />}</>;
}
afterEach(() => { cleanup(); forgetViewOpening(KEY); clearScreenHeading(); vi.clearAllMocks(); });

it.each(['another container', 'collapsed section', 'closed bottom panel', 'background tab', 'closed mobile sheet'])('does not mount data for %s before it is shown', async () => {
  const view = show(<Frame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  expect(renderBody).not.toHaveBeenCalled();
  expect(read).not.toHaveBeenCalled();
  expect(subscribe).not.toHaveBeenCalled();
  view.rerender(<Frame shown />);
  expect(await screen.findByText('Current data')).toBeVisible();
  expect(read).toHaveBeenCalledTimes(1);
  expect(subscribe).toHaveBeenCalledTimes(1);
});

it('drops hidden subscriptions, retains the draft, and does not restart a subscription on a same-commit move', async () => {
  const view = show(<Frame shown />);
  await screen.findByText('Current data');
  fireEvent.change(screen.getByRole('textbox', { name: 'Draft' }), { target: { value: 'Still writing' } });
  view.rerender(<Frame shown position="bottom" />);
  await act(async () => { await Promise.resolve(); });
  expect(subscribe).toHaveBeenCalledTimes(1);
  expect(unsubscribe).not.toHaveBeenCalled();
  view.rerender(<Frame shown={false} />);
  await waitFor(() => expect(unsubscribe).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole('textbox', { name: 'Draft' })).not.toBeInTheDocument();
  view.rerender(<Frame shown />);
  await waitFor(() => expect(subscribe).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('textbox', { name: 'Draft' })).toHaveValue('Still writing');
  expect(read).toHaveBeenCalledTimes(1);
});

function PageBody({ title }: { title: string }) {
  useScreenHeading({ title, views: { ready: true, workspace: true } });
  return <Body />;
}
function Caption() { return <output data-testid="frame-page">{useCurrentHeading()?.title ?? 'No page'}</output>; }
function PageFrame({ shown, title }: { shown: boolean; title: string }) {
  return <><Caption /><ViewOpening view={KEY} route="route"><PageBody title={title} /></ViewOpening>{shown && <Slot />}</>;
}

it('retains the known page identity while a sheet or another tab covers it, without hidden reads', async () => {
  const view = show(<PageFrame shown title="Page one" />);
  await screen.findByText('Current data');
  await waitFor(() => expect(screen.getByTestId('frame-page')).toHaveTextContent('Page one'));
  view.rerender(<PageFrame shown={false} title="Page one" />);
  await waitFor(() => expect(unsubscribe).toHaveBeenCalledTimes(1));
  expect(screen.getByTestId('frame-page')).toHaveTextContent('Page one');
  expect(currentHeading()?.views?.workspace).toBe(true);
  expect(read).toHaveBeenCalledTimes(1);
  // The shell clears metadata at a committed route boundary, not on an ordinary hide.
  act(() => { clearScreenHeading(); });
  view.rerender(<PageFrame shown={false} title="Page two" />);
  await act(async () => { await Promise.resolve(); });
  expect(screen.getByTestId('frame-page')).toHaveTextContent('No page');
  expect(read).toHaveBeenCalledTimes(1);
  view.rerender(<PageFrame shown title="Page two" />);
  await waitFor(() => expect(screen.getByTestId('frame-page')).toHaveTextContent('Page two'));
});
