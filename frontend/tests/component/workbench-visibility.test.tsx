/** A hidden view runs no reader or subscription, and a shown view keeps its draft across hiding and moving. */
import { QueryClient, QueryClientProvider, QueryObserver, useQuery, useQueryClient } from '@tanstack/react-query';
import { act, cleanup, fireEvent, renderHook, screen, waitFor } from '@testing-library/react';
import { useEffect, useLayoutEffect, useState, type ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { forgetViewOpening, showViewOpening, useViewShown, ViewOpening } from '@/features/workbench/viewOpening';
import { clearScreenHeading, currentHeading, useCurrentHeading, useScreenHeading } from '@/shell/screen';
import { show } from './support';
import { feeds, socket, useOrgTreeFeed } from '@/realtime/live';
import type { FeedListener } from '@/realtime/socket';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { chatWriter, useChatFeed, useChatStore } from '@/realtime/stores/chat';
import { EVICT_AFTER_MS, UNSUBSCRIBE_AFTER_MS } from '@/realtime/sync';
import { treeNode } from '../doubles/tree-node';
import { chat, message } from '../doubles/chat-rows';
import { FileText } from 'lucide-react';
import { ViewHost, ViewSlot } from '@/features/workbench/Board';
import { useWorkbench } from '@/features/workbench/store';
import { findTab, makeGroup, makeTab } from '@/features/workbench/layout';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { meta } from '../doubles/fixtures';

import { useInterfaceReading } from '@/lib/interfaceReading';
import { useReadProgress } from '@/lib/useReadProgress';
import { readFreshness } from '@/api/freshness';
import { queryClient, LiveQueryClient } from '@/api/query';
import { KeptQueryScope, viewReadState } from '@/features/workbench/KeptQueries';
import { isReadRetainer } from '@/api/reads';
import { scheduledReads } from '@/api/readScheduler';

const KEY = 'visibility-probe';
const read = vi.fn(() => Promise.resolve('Current data'));
const subscribe = vi.fn();
const unsubscribe = vi.fn();
const renderBody = vi.fn();
vi.mock('@/features/navigation/placeLabels', async (loadOriginal) => ({ ...await loadOriginal<object>(), useNodeLabel: (_id: string | undefined, title: string) => title }));

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

it('clears a chat capacity refusal when the retained cursor is accepted without new messages', async () => {
  const current = chat({ seq: 7 }), row = message(1);
  chatWriter.reset(current.id, current, [row], false, 7);
  let listener: FeedListener | undefined;
  const subscribed = vi.spyOn(socket, 'subscribe').mockImplementation((_feed, next) => { listener = next; return { unsubscribe: () => undefined }; });
  function Chat() { useChatFeed(current.id); return null; }
  const view = show(<Chat />);
  try {
    await act(async () => { await Promise.resolve(); });
    if (listener === undefined) throw new Error('The chat must own its native feed');
    const before = useChatStore.getState().byChat.get(current.id);
    act(() => listener?.onFrame({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'capacity' }));
    expect(useChatStore.getState().byChat.get(current.id)?.error).not.toBeNull();
    act(() => listener?.onFrame({ op: 'subscribed', id: 2, feed: `chat:${current.id}`, mode: 'delta', seq: 7 }));
    expect(useChatStore.getState().byChat.get(current.id)?.error).toBeNull();
    expect(useChatStore.getState().byChat.get(current.id)?.messages).toBe(before?.messages); expect(listener.since()).toBe(7);
  } finally { view.unmount(); feeds.clear(); chatWriter.clear(); subscribed.mockRestore(); }
});

it('keeps the sole recently closed content draft when a sidebar section is hidden and reopened', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  cache.setQueryData(['me-state'], { interface: { kept_closed_tabs: 1 } });
  cache.setQueryData(['meta'], meta());
  const saved = useWorkbench.getState();
  const content = makeTab('closed-content-probe'), peer = makeTab('closed-content-probe'), side = makeTab('closed-side-probe');
  const main = makeGroup([content, peer], content.id);
  function Draft({ tab }: ViewProps) {
    const [draft, setDraft] = useState('');
    return <input aria-label={`Draft ${tab.id}`} value={draft} onChange={(event) => setDraft(event.target.value)} />;
  }
  const removeContent = registerView({ name: 'closed-content-probe', role: 'content', title: 'views.screen.title', about: 'views.screen.about', icon: FileText, screens: 'all', area: 'main', body: Draft });
  const removeSide = registerView({ name: 'closed-side-probe', role: 'inspector', title: 'views.screen.title', about: 'views.screen.about', icon: FileText, screens: 'all', area: 'left', body: Draft });
  useWorkbench.setState({ userId: 'closed-view-reader', layout: { ...saved.layout, main,
    left: { ...saved.layout.left, open: true, active: 'probe', containers: [{ id: 'probe-side', name: 'probe', active: side.id, sections: [{ tab: side, open: true, hidden: false, size: 1 }] }] } },
    activeContent: content.id, focusedTab: content.id, focusedMainTab: content.id, focusedMainGroup: main.id, closedTabs: [], unsaved: new Set() });
  function ActualHost() {
    const layout = useWorkbench((state) => state.layout), active = useWorkbench((state) => state.focusedMainTab);
    const current = active === null ? null : findTab(layout, active), section = findTab(layout, side.id);
    return <FrameReadiness.Provider value={{ ready: true, pending: null }}><ViewHost layout={layout} />
      {current !== null && <ViewSlot tab={current.tab} />}{section !== null && !section.section?.hidden && <ViewSlot tab={section.tab} />}
    </FrameReadiness.Provider>;
  }
  const view = show(<ActualHost />, cache);
  try {
    const input = await screen.findByRole('textbox', { name: `Draft ${content.id}` });
    fireEvent.change(input, { target: { value: 'Retained closed draft' } });
    act(() => useWorkbench.getState().close({ kind: 'tab', tab: content.id }));
    await screen.findByRole('textbox', { name: `Draft ${peer.id}` });
    act(() => useWorkbench.getState().close({ kind: 'tab', tab: side.id }));
    await act(async () => { await Promise.resolve(); });
    act(() => { useWorkbench.getState().reopen(); useWorkbench.getState().reopen(); });
    const reopened = await screen.findByRole('textbox', { name: `Draft ${content.id}` });
    expect(reopened).toBe(input); expect(reopened).toHaveValue('Retained closed draft');
  } finally {
    view.unmount(); removeContent(); removeSide(); cache.clear();
    useWorkbench.getState().leave(); useWorkbench.setState(saved);
  }
});

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

it('keeps an admitted cold answer while hidden so returning needs no replacement read', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  let finish!: (value: string) => void;
  let signal: AbortSignal | undefined;
  const requests = vi.fn(({ signal: current }: { signal: AbortSignal }) => { signal = current; return new Promise<string>((resolve) => { finish = resolve; }); });
  function ColdBody() { const query = useQuery({ queryKey: ['admitted-cold-answer'], queryFn: requests }); return <output>{query.data}</output>; }
  function ColdFrame({ shown }: { shown: boolean }) { return <><ViewOpening view={KEY} route="route"><ColdBody /></ViewOpening>{shown && <Slot />}</>; }
  const view = show(<ColdFrame shown />, cache);
  try {
    await waitFor(() => expect(requests).toHaveBeenCalledOnce());
    view.rerender(<ColdFrame shown={false} />);
    await act(async () => { await Promise.resolve(); });
    expect(signal?.aborted).toBe(false);
    await act(async () => { finish('The admitted answer'); await Promise.resolve(); });
    expect(cache.getQueryData(['admitted-cold-answer'])).toBe('The admitted answer');
    view.rerender(<ColdFrame shown />);
    expect(await screen.findByText('The admitted answer')).toBeVisible();
    expect(requests).toHaveBeenCalledOnce();
  } finally { view.unmount(); cache.clear(); }
});

it.each([false, true])('waits until a hidden failed first read is shown before retrying with pending work=%s', async (hold) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: (attempt) => attempt < 1, retryDelay: 0 } } });
  let fail!: (error: Error) => void;
  const requests = vi.fn<() => Promise<string>>().mockImplementationOnce(() => new Promise((_resolve, reject) => { fail = reject; })).mockResolvedValue('Visible recovery');
  function ColdBody() { const query = useQuery({ queryKey: ['hidden-cold-retry'], queryFn: requests }); return <output>{query.data}</output>; }
  function ColdFrame({ shown }: { shown: boolean }) { return <><ViewOpening view={KEY} route="route" hold={hold}><ColdBody /></ViewOpening>{shown && <Slot />}</>; }
  const view = show(<ColdFrame shown />, cache);
  try {
    await waitFor(() => expect(requests).toHaveBeenCalledOnce());
    view.rerender(<ColdFrame shown={false} />);
    await act(async () => { await Promise.resolve(); fail(new Error('The admitted read failed')); });
    await waitFor(() => expect(cache.getQueryState(['hidden-cold-retry'])?.fetchStatus).toBe('idle'));
    expect(requests).toHaveBeenCalledOnce();
    view.rerender(<ColdFrame shown />);
    expect(await screen.findByText('Visible recovery')).toBeVisible();
    expect(requests).toHaveBeenCalledTimes(2);
  } finally { view.unmount(); cache.clear(); }
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

it('shows a kept draft without redrawing its fresh data readers to pause and resume them', async () => {
  const owner = new QueryClient();
  const drawn = vi.fn(), requests = vi.fn(() => Promise.resolve('Refreshed'));
  const key = ['kept-draft-options'];
  owner.setQueryData(key, 'Ready');
  function Reader() {
    drawn();
    const result = useQuery({ queryKey: key, queryFn: requests, staleTime: Infinity });
    return <output>{result.data}</output>;
  }
  // The view remains mounted while its local work is held. Only its read policy changes.
  const readers = Array.from({ length: 20 }, (_, i) => <Reader key={i} />);
  const frame = (shown: boolean) => <KeptQueryScope shown={shown}>{readers}</KeptQueryScope>;
  const view = show(frame(true), owner);
  try {
    expect(screen.getAllByText('Ready')).toHaveLength(20);
    drawn.mockClear();
    view.rerender(frame(false));
    expect(owner.getQueryCache().find({ queryKey: key })?.isActive()).toBe(false);
    view.rerender(frame(true));
    await waitFor(() => expect(owner.getQueryCache().find({ queryKey: key })?.isActive()).toBe(true));
    expect(drawn.mock.calls.length).toBe(0);
    expect(requests).not.toHaveBeenCalled();
    act(() => { owner.setQueryData(key, 'Changed'); });
    expect(await screen.findAllByText('Changed')).toHaveLength(20);
  } finally { view.unmount(); owner.clear(); }
});

it('resumes only the current enabled reader when an enclosing kept view returns', async () => {
  const owner = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const read = vi.fn((id: string) => Promise.resolve(id));
  function Readers({ subject }: { subject: string }) {
    const value = useQuery({ queryKey: ['nested-page', subject], queryFn: () => read(subject) });
    useQuery({ queryKey: ['never-enabled'], queryFn: () => read('disabled'), enabled: false });
    return <output>{value.data}</output>;
  }
  const frame = (shown: boolean, subject: string) => <KeptQueryScope shown={shown}><KeptQueryScope shown>
    <Readers subject={subject} />
  </KeptQueryScope></KeptQueryScope>;
  const view = show(frame(true, 'Original'), owner);
  try {
    expect(await screen.findByText('Original')).toBeVisible();
    view.rerender(frame(false, 'Original'));
    view.rerender(frame(false, 'Replacement'));
    expect(read.mock.calls).toEqual([['Original']]);
    view.rerender(frame(true, 'Replacement'));
    expect(await screen.findByText('Replacement')).toBeVisible();
    expect(read.mock.calls).toEqual([['Original'], ['Replacement']]);
  } finally { view.unmount(); owner.clear(); }
});

it('pauses both readers even if the first unsubscribes while its tab is being hidden', () => {
  const owner = new QueryClient(), key = ['shared-pausing-read'];
  owner.setQueryData(key, 'Current');
  let client: QueryClient | undefined;
  function Readers() {
    const scoped = useQueryClient();
    useLayoutEffect(() => { client = scoped; }, [scoped]);
    useQuery({ queryKey: key, staleTime: Infinity });
    return null;
  }
  const readers = <Readers />;
  const frame = (shown: boolean) => <KeptQueryScope shown={shown}>{readers}</KeptQueryScope>;
  const view = show(frame(true), owner);
  if (client === undefined) throw new Error('Expected the mounted view’s query client');
  const options = { queryKey: key, queryFn: () => Promise.resolve('Current'), staleTime: Infinity };
  const first = new QueryObserver(client, options), second = new QueryObserver(client, options);
  const leave = first.subscribe((value) => { if (!value.isEnabled) leave(); });
  const stop = second.subscribe(() => undefined);
  try {
    view.rerender(frame(false));
    expect(second.getCurrentResult().isEnabled).toBe(false);
  } finally { leave(); stop(); view.unmount(); owner.clear(); }
});

it('revalidates a kept answer after its initial page supplier retires behind another tab', async () => {
  const owner = new LiveQueryClient();
  const key = ['kept-after-opening'], supplier = ['page-read', 'kept-after-opening'];
  owner.setQueryData(key, 'From opening');
  scheduledReads.supply(supplier, [key]);
  const read = vi.fn(() => Promise.resolve('From revalidation'));
  function Body() {
    const result = useQuery({ queryKey: key, staleTime: 0, queryFn: read });
    return <output>{result.data}</output>;
  }
  const body = <Body />;
  const frame = (shown: boolean) => <KeptQueryScope shown={shown}>{body}</KeptQueryScope>;
  const view = show(frame(true), owner);
  try {
    expect(screen.getByText('From opening')).toBeVisible();
    expect(read).not.toHaveBeenCalled();
    view.rerender(frame(false));
    scheduledReads.release(supplier);
    view.rerender(frame(true));
    expect(screen.getByText('From opening')).toBeVisible();
    expect(await screen.findByText('From revalidation')).toBeVisible();
    expect(read).toHaveBeenCalledOnce();
  } finally { scheduledReads.release(supplier); view.unmount(); owner.clear(); }
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

it.each([['application', LiveQueryClient], ['external', QueryClient]] as const)('resolves kept read options with bounded lookups among 5,000 cached queries (%s cache)', (kind, Client) => {
  const owner = new Client({ defaultOptions: { queries: { gcTime: Infinity } } });
  const cache = owner.getQueryCache();
  for (let i = 0; i < 5_000; i++) owner.setQueryData(['thread', i], i);
  const bootstrap = cache.build(owner, { queryKey: ['shell-bootstrap', 'first'] });
  const all = vi.spyOn(cache, 'getAll');
  const wrapper = ({ children }: { children: ReactNode }) =>
    <QueryClientProvider client={owner}><KeptQueryScope shown>{children}</KeptQueryScope></QueryClientProvider>;
  const hook = renderHook(() => useQueryClient(), { wrapper });
  const peer = renderHook(() => useQueryClient(), { wrapper });
  const options = (queryKey: string[]) => hook.result.current.defaultQueryOptions({ queryKey, enabled: true });
  const get = vi.spyOn(cache, 'get');
  try {
    // All kept tabs share the index. Only an external cache needs to adopt existing queries.
    expect(all).toHaveBeenCalledTimes(kind === 'external' ? 1 : 0);
    all.mockClear();
    for (let i = 0; i < 100; i++) {
      expect(options(['permissions']).enabled).toBe(false);
      expect(peer.result.current.defaultQueryOptions({ queryKey: ['settings', 'system'], enabled: true }).enabled).toBe(true);
    }
    expect(all).not.toHaveBeenCalled();
    expect(get.mock.calls.length).toBeLessThanOrEqual(200);
    // As with the original prefix lookup, the first live match supplies the bootstrap state.
    owner.setQueryData(['shell-bootstrap', 'second'], { ready: true });
    expect(options(['permissions']).enabled).toBe(false);
    cache.remove(bootstrap);
    expect(options(['permissions']).enabled).toBe(true);
    owner.removeQueries({ queryKey: ['shell-bootstrap'] });
    const pending = cache.build(owner, { queryKey: ['shell-bootstrap', 'second'] });
    expect(options(['permissions']).enabled).toBe(false);
    owner.setQueryData(['permissions'], []);
    expect(options(['permissions']).enabled).toBe(true);
    cache.remove(pending);
    owner.removeQueries({ queryKey: ['permissions'] });
    expect(options(['permissions']).enabled).toBe(true);
  } finally {
    all.mockRestore(); get.mockRestore(); hook.unmount(); peer.unmount(); owner.clear();
  }
});

it('applies a kept reader’s resolved options without restarting option resolution for each observer update', () => {
  const owner = new LiveQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) =>
    <QueryClientProvider client={owner}><KeptQueryScope shown>{children}</KeptQueryScope></QueryClientProvider>;
  const hook = renderHook(() => useQueryClient(), { wrapper });
  const key = ['stable-reader'];
  const client = hook.result.current;
  owner.setQueryData(key, 1);
  const options = client.defaultQueryOptions({ queryKey: key, queryFn: () => Promise.resolve(1), retry: () => false, staleTime: Infinity });
  const observer = new QueryObserver(client, options);
  const off = observer.subscribe(() => undefined);
  const resolve = vi.spyOn(owner, 'defaultQueryOptions');
  const changes = vi.fn();
  const stop = owner.getQueryCache().subscribe((event) => {
    if (event.type === 'observerOptionsUpdated' && event.observer === observer) changes();
  });
  try {
    for (let i = 0; i < 100; i++) observer.setOptions(options);
    expect(changes.mock.calls.length).toBe(0);
    expect(resolve.mock.calls.length).toBe(0);
  } finally { stop(); resolve.mockRestore(); off(); hook.unmount(); owner.clear(); }
});

it('rechecks a missing answer when the shell starts supplying it before the observer attaches', () => {
  const owner = new LiveQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) =>
    <QueryClientProvider client={owner}><KeptQueryScope shown>{children}</KeptQueryScope></QueryClientProvider>;
  const hook = renderHook(() => useQueryClient(), { wrapper });
  const client = hook.result.current;
  const read = vi.fn(() => Promise.resolve([]));
  const options = client.defaultQueryOptions({ queryKey: ['permissions'], queryFn: read });
  owner.getQueryCache().build(owner, { queryKey: ['shell-bootstrap', 'reader'] });
  const observer = new QueryObserver(client, options);
  const off = observer.subscribe(() => undefined);
  try {
    expect(read).not.toHaveBeenCalled();
    owner.setQueryData(['permissions'], ['view']);
    expect(observer.getCurrentResult().data).toEqual(['view']);
  } finally { off(); hook.unmount(); owner.clear(); }
});

it('shares the retention of one answer across ten tabs until the last tab releases it', () => {
  const owner = new QueryClient();
  const key = ['shared-catalogue'];
  owner.setQueryData(key, 'Already read');
  function Body() {
    const read = useQuery({ queryKey: key, staleTime: Infinity });
    return <output>{String(read.data)}</output>;
  }
  function Tabs({ count }: { count: number }) {
    return Array.from({ length: count }, (_, i) => <KeptQueryScope key={i} shown><Body /></KeptQueryScope>);
  }
  const view = show(<Tabs count={10} />, owner);
  const query = owner.getQueryCache().find({ queryKey: key, exact: true })!;
  const holders = () => query.observers.filter(isReadRetainer);
  try {
    expect(holders()).toHaveLength(1);
    const holder = holders()[0];
    view.rerender(<Tabs count={1} />);
    expect(holders()).toEqual([holder]);
    expect(screen.getByText('Already read')).toBeVisible();
    view.rerender(<Tabs count={0} />);
    expect(holders()).toHaveLength(0);
  } finally { view.unmount(); owner.clear(); }
});

it('checks the returning page’s own pending read without enumerating the other 5,000 reads', () => {
  const owner = new QueryClient({ defaultOptions: { queries: { gcTime: Infinity } } });
  for (let i = 0; i < 5_000; i++) owner.setQueryData(['background', i], i);
  const key = ['pending-page'];
  const hook = renderHook(() => {
    useQuery({ queryKey: key, queryFn: () => new Promise<string>(() => undefined) });
    return useQueryClient();
  }, { wrapper: ({ children }) => <QueryClientProvider client={owner}><KeptQueryScope shown>{children}</KeptQueryScope></QueryClientProvider> });
  const all = vi.spyOn(owner.getQueryCache(), 'getAll');
  try {
    expect(viewReadState(hook.result.current)).toEqual({ pending: true, failed: false });
    owner.setQueryData(key, 'Page ready');
    expect(viewReadState(hook.result.current)).toEqual({ pending: false, failed: false });
    expect(all).not.toHaveBeenCalled();
  } finally { all.mockRestore(); hook.unmount(); owner.clear(); }
});

it('does no preference option resolution in kept controls when a running thread publishes unrelated rows', () => {
  const owner = new LiveQueryClient();
  const cache = owner.getQueryCache();
  owner.setQueryData(['meta'], meta());
  owner.setQueryData(['me-state'], { interface: { progress_delay_ms: 125, kept_closed_tabs: 7 } });
  const unrelated = cache.build(owner, { queryKey: ['thread-parameters', 'running'] });
  function Controls({ busy }: { busy: boolean }) {
    const slow = useReadProgress(busy);
    return <button aria-busy={busy}>{String(slow)}</button>;
  }
  function Preferences() {
    const { kept_closed_tabs, progress_delay_ms, ready } = useInterfaceReading();
    return <output>{ready ? `${kept_closed_tabs}/${progress_delay_ms}` : 'pending'}</output>;
  }
  const view = show(<KeptQueryScope shown><Preferences />
    {Array.from({ length: 40 }, (_, i) => <Controls key={i} busy={i === 0} />)}
  </KeptQueryScope>, owner);
  try {
    expect(view.container.querySelector('output')).toHaveTextContent('7/125');
    const resolve = vi.spyOn(owner, 'defaultQueryOptions');
    act(() => { for (let seq = 1; seq <= 10; seq++) unrelated.setData({ seq }); });
    expect(resolve.mock.calls.length).toBe(0);
    act(() => { owner.setQueryData(['me-state'], { interface: { progress_delay_ms: 0, kept_closed_tabs: 3 } }); });
    expect(view.container.querySelector('output')).toHaveTextContent('3/0');
    act(() => { owner.removeQueries({ queryKey: ['meta'], exact: true }); });
    expect(view.container.querySelector('output')).toHaveTextContent('pending');
  } finally { view.unmount(); owner.clear(); }
});

it('reads the current freshness window without resolving two other queries for every observer', () => {
  const stateKey = ['me-state'], oldState = queryClient.getQueryState(stateKey);
  const baseline = readFreshness(['settings', 'system']);
  const resolve = vi.spyOn(queryClient, 'defaultQueryOptions');
  try {
    expect(readFreshness(['settings', 'system'])).toBe(baseline);
    expect(resolve.mock.calls.length).toBe(0);
    queryClient.setQueryData(stateKey, { interface: { read_freshness_ms: { directory: 8754 } } });
    resolve.mockClear();
    expect(readFreshness(['settings', 'system'])).toBe(8754);
    expect(resolve.mock.calls.length).toBe(0);
  } finally {
    resolve.mockRestore();
    if (oldState === undefined) queryClient.removeQueries({ queryKey: stateKey, exact: true });
    else queryClient.getQueryCache().find({ queryKey: stateKey, exact: true })?.setState(oldState);
  }
});
