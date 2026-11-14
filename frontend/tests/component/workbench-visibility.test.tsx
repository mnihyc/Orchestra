/** A hidden view runs no reader or subscription, and a shown view keeps its draft across hiding and moving. */
import { QueryClient, useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { useEffect, useLayoutEffect, useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { forgetViewOpening, showViewOpening, useViewShown, ViewOpening } from '@/features/workbench/viewOpening';
import { clearScreenHeading, currentHeading, useCurrentHeading, useScreenHeading } from '@/shell/screen';
import { show } from './support';
import { feeds, useOrgTreeFeed } from '@/realtime/live';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { chatWriter, useChatFeed, useChatStore } from '@/realtime/stores/chat';
import { EVICT_AFTER_MS, UNSUBSCRIBE_AFTER_MS } from '@/realtime/sync';
import { treeNode } from '../doubles/tree-node';
import { chat, message } from '../doubles/chat-rows';

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

for (const kind of ['tree', 'chat'] as const) for (const hold of [false, true]) it(`keeps the ${kind} model past the hidden feed window with effects held=${hold}`, async () => {
  vi.useFakeTimers();
  const org = 'retained-org', node = treeNode({ id: 'retained-topic', kind: 'topic', parent_id: org, name: 'Kept tree' });
  const current = chat({ seq: 7, name: 'Kept conversation' }), row = message(1);
  treeWriter.reset(org, [node], 7); chatWriter.reset(current.id, current, [row], false, 7);
  const stopped = vi.fn(), feed = kind === 'tree' ? `org:${org}` : `chat:${current.id}`;
  const release = feeds.use(feed, () => ({ lastSeq: () => 7, reload: () => undefined, refresh: () => undefined, stop: stopped }),
    () => { if (kind === 'tree') treeWriter.evict(org); else chatWriter.evict(current.id); });
  const requests = vi.fn(() => Promise.resolve(Response.json({ seq: 7, complete: true, nodes: [], hidden: [] })));
  vi.stubGlobal('fetch', requests);
  function TreeBody() { useOrgTreeFeed(org); const value = useTreeStore((state) => state.byOrg.get(org)); return <output>{value?.nodes.get(node.id)?.name}</output>; }
  function ChatBody() { useChatFeed(current.id); const value = useChatStore((state) => state.byChat.get(current.id)); return <output>{value?.chat?.name}</output>; }
  const Body = kind === 'tree' ? TreeBody : ChatBody;
  function KeptModelFrame({ shown }: { shown: boolean }) { return <><ViewOpening view={KEY} route="route" hold={hold}><Body /></ViewOpening>{shown && <Slot />}</>; }
  const cache = new QueryClient();
  const view = show(<KeptModelFrame shown />, cache);
  try {
    await act(async () => { await Promise.resolve(); });
    release();
    view.rerender(<KeptModelFrame shown={false} />);
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS + EVICT_AFTER_MS + 1); });
    expect(stopped).toHaveBeenCalledOnce();
    if (kind === 'tree') expect(useTreeStore.getState().byOrg.get(org)?.nodes.has(node.id)).toBe(true);
    else expect(useChatStore.getState().byChat.get(current.id)?.messages.has(row.id)).toBe(true);
    view.rerender(<KeptModelFrame shown />);
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByText(kind === 'tree' ? 'Kept tree' : 'Kept conversation')).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(requests).not.toHaveBeenCalled();
  } finally { view.unmount(); release(); feeds.clear(); treeWriter.clear(); chatWriter.clear(); cache.clear(); vi.useRealTimers(); vi.unstubAllGlobals(); }
});

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

it.each(['List', 'Settings'])('shows a kept %s before its revalidation and preserves its draft and scroll after query collection', async (name) => {
  const cache = new QueryClient({ defaultOptions: { queries: { gcTime: 0, staleTime: 0, retry: false } } });
  const beforeContent: string[] = [];
  const requests = vi.fn(() => {
    beforeContent.push(screen.queryByTestId('kept-answer')?.textContent ?? '');
    return Promise.resolve(name);
  });
  function KeptBody() {
    const answer = useQuery({ queryKey: ['kept-answer', name], queryFn: requests });
    const [draft, setDraft] = useState('');
    return <div data-testid="kept-scroll"><output data-testid="kept-answer">{answer.data}</output>
      <input aria-label="Kept draft" value={draft} onChange={(event) => setDraft(event.target.value)} /></div>;
  }
  function KeptFrame({ shown }: { shown: boolean }) {
    return <><ViewOpening view={KEY} route="route"><KeptBody /></ViewOpening>{shown && <Slot />}</>;
  }
  const view = show(<KeptFrame shown />, cache);
  await screen.findByText(name);
  fireEvent.change(screen.getByRole('textbox', { name: 'Kept draft' }), { target: { value: 'Unfinished' } });
  const scroll = screen.getByTestId('kept-scroll');
  scroll.scrollTop = 317;
  view.rerender(<KeptFrame shown={false} />);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
  await act(async () => { await cache.invalidateQueries({ queryKey: ['kept-answer', name], exact: true }); });
  expect(requests).toHaveBeenCalledTimes(1);
  expect(cache.getQueryData(['kept-answer', name])).toBe(name);
  view.rerender(<KeptFrame shown />);
  await waitFor(() => expect(requests).toHaveBeenCalledTimes(2));
  expect(beforeContent).toEqual(['', name]);
  expect(screen.getByRole('textbox', { name: 'Kept draft' })).toHaveValue('Unfinished');
  expect(screen.getByTestId('kept-scroll')).toBe(scroll);
  expect(scroll.scrollTop).toBe(317);
  view.unmount();
  cache.clear();
});

it('pauses hidden reads while an unsaved draft keeps its view effects alive', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const requests = vi.fn(() => Promise.resolve('Kept answer'));
  function UnsavedBody() {
    const query = useQuery({ queryKey: ['unsaved-hidden-reader'], queryFn: requests });
    return <output>{query.data}</output>;
  }
  function UnsavedFrame({ shown }: { shown: boolean }) {
    return <><ViewOpening view={KEY} route="route" hold><UnsavedBody /></ViewOpening>{shown && <Slot />}</>;
  }
  const view = show(<UnsavedFrame shown />, cache);
  await screen.findByText('Kept answer');
  view.rerender(<UnsavedFrame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  await act(async () => { await cache.invalidateQueries({ queryKey: ['unsaved-hidden-reader'], exact: true }); });
  expect(requests).toHaveBeenCalledTimes(1);
  view.unmount(); cache.clear();
});

it('paints a returning answer for a frame before starting its background read', async () => {
  const frames = new Map<number, FrameRequestCallback>();
  let next = 0;
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { const id = ++next; frames.set(id, callback); return id; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => { frames.delete(id); });
  const frame = () => { const callbacks = [...frames.values()]; frames.clear(); for (const callback of callbacks) callback(performance.now()); };
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: 0, retry: false } } });
  const requests = vi.fn(() => Promise.resolve('Kept before the read'));
  function PaintBody() { const query = useQuery({ queryKey: ['return-paint'], queryFn: requests }); return <output>{query.data}</output>; }
  function PaintFrame({ shown }: { shown: boolean }) { return <><ViewOpening view={KEY} route="route"><PaintBody /></ViewOpening>{shown && <Slot />}</>; }
  const view = show(<PaintFrame shown />, cache);
  try {
    await screen.findByText('Kept before the read');
    view.rerender(<PaintFrame shown={false} />);
    await act(async () => { await Promise.resolve(); });
    view.rerender(<PaintFrame shown />);
    await act(async () => { await Promise.resolve(); });
    expect(screen.getByText('Kept before the read')).toBeVisible();
    expect(requests).toHaveBeenCalledTimes(1);
    await act(async () => { frame(); await Promise.resolve(); });
    expect(requests).toHaveBeenCalledTimes(1);
    await act(async () => { frame(); await Promise.resolve(); });
    await waitFor(() => expect(requests).toHaveBeenCalledTimes(2));
  } finally { view.unmount(); cache.clear(); vi.unstubAllGlobals(); }
});

it('releases an obsolete visible request while keeping the current answer across hiding', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { gcTime: 0, staleTime: Infinity, retry: false } } });
  function Current({ subject }: { subject: string }) { const value = useQuery({ queryKey: ['current-view-model', subject], queryFn: () => Promise.resolve(subject) }); return <output>{value.data}</output>; }
  function CurrentFrame({ shown, subject }: { shown: boolean; subject: string }) { return <><ViewOpening view={KEY} route="route"><Current subject={subject} /></ViewOpening>{shown && <Slot />}</>; }
  const view = show(<CurrentFrame shown subject="Former answer" />, cache);
  try {
    await screen.findByText('Former answer');
    view.rerender(<CurrentFrame shown subject="Current answer" />);
    await screen.findByText('Current answer');
    await waitFor(() => expect(cache.getQueryData(['current-view-model', 'Former answer'])).toBeUndefined());
    view.rerender(<CurrentFrame shown={false} subject="Current answer" />);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(cache.getQueryData(['current-view-model', 'Current answer'])).toBe('Current answer');
    view.rerender(<CurrentFrame shown subject="Current answer" />);
    expect(await screen.findByText('Current answer')).toBeVisible();
  } finally { view.unmount(); cache.clear(); }
});

it('retains the newest query object when an older object is removed under the same key', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { gcTime: 0, staleTime: Infinity, retry: false } } });
  const key = ['same-key-view-model']; let replaced = false;
  const off = cache.getQueryCache().subscribe((event) => {
    const removedKey: unknown = event.query.queryKey;
    if (event.type === 'removed' && Array.isArray(removedKey) && removedKey[0] === key[0] && !replaced) { replaced = true; cache.setQueryData(key, 'Newer answer'); }
  });
  function Current({ revision }: { revision: number }) { const value = useQuery({ queryKey: key, queryFn: () => Promise.resolve('Former answer') }); return <output data-revision={revision}>{value.data}</output>; }
  function CurrentFrame({ shown, revision }: { shown: boolean; revision: number }) { return <><ViewOpening view={KEY} route="route"><Current revision={revision} /></ViewOpening>{shown && <Slot />}</>; }
  const view = show(<CurrentFrame shown revision={0} />, cache);
  try {
    await screen.findByText('Former answer');
    act(() => {
      const former = cache.getQueryCache().find({ queryKey: key, exact: true });
      if (former === undefined) throw new Error('The displayed query must exist.');
      cache.getQueryCache().remove(former);
      view.rerender(<CurrentFrame shown revision={1} />);
    });
    await screen.findByText('Newer answer');
    view.rerender(<CurrentFrame shown={false} revision={1} />);
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)); });
    expect(cache.getQueryData(key)).toBe('Newer answer');
  } finally { view.unmount(); off(); cache.clear(); }
});
