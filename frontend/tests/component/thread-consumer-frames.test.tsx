import { defaultThreadRead } from '@/realtime/threadScope';
/** A bad cached conversation frame is contained in its own view; a complete native frame restores it without reloading the application. */
import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { ThreadScreen } from '@/features/thread/Thread';
import { isThreadForScreen, threadCapabilities } from '@/features/thread/cached';
import { mayExportThread } from '@/features/export/pending';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { agentMessage } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show, skipOverflowFallback } from './support';
import { Profiler, useLayoutEffect } from 'react';
import { QueryClient } from '@tanstack/react-query';
import { KeptQueryScope } from '@/features/workbench/KeptQueries';
import { ViewOpening, showViewOpening, forgetViewOpening } from '@/features/workbench/viewOpening';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { useThreadView } from '@/realtime/stores/thread';
import type { FeedListener } from '@/realtime/socket';
import { serve, OLDER } from '../unit/thread-read-fixtures';
import { answer } from '../unit/continuation-fixtures';
import { captureThreadRead } from '@/api/threadRead';

const calls = vi.hoisted(() => ({ reload: vi.fn(), feed: vi.fn<typeof Live.useThreadFeed>(), native: false }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: calls.feed, useOrgTreeFeed: () => undefined, reloadThread: calls.reload }));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', async (loadOriginal) => ({ ...await loadOriginal<object>(), selectOrg: () => undefined }));
vi.mock('@/features/thread/Header', () => ({ ThreadHeader: ({ thread }: { thread: Thread }) => <header><h1>{thread.name}</h1></header> }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => <textarea aria-label="Thread draft" /> }));
vi.mock('@/features/composer/QueueTray', () => ({ QueueTray: () => null }));
vi.mock('@/features/schedules/PendingAgentSchedules', () => ({ PendingAgentSchedules: () => null }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/thread/BlockedBanner', () => ({ BlockedBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => calls.native ? <RetainedNativeRows /> : <p>The redirect has been checked.</p> }));
vi.mock('@/features/summaries/ThreadSummaryInspector', () => ({ ThreadSummaryInspector: () => null }));
vi.mock('@/features/gateways/ThreadGatewayStatus', () => ({ ThreadGatewayStatus: () => null }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ children }: { children: ReactNode }) => children }));

let native: Thread;
function RetainedNativeRows() {
  const view = useThreadView(native.id);
  return <>{view.order.map((id) => { const item = view.items.get(id); return item?.kind === 'agent_message' ? <p key={id}>{item.payload.text}</p> : null; })}</>;
}
let theme: string | null = null;
beforeEach(() => {
  calls.reload.mockReset(); theme = document.documentElement.getAttribute('data-theme');
  calls.native = false; calls.feed.mockReset().mockImplementation(() => undefined);
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, node), state: 'idle' };
  threadWriter.reset(native.id, { thread: native, items: [{ ...agentMessage('answer', 'The redirect has been checked.'), thread_id: native.id, seq_created: 1, seq_updated: 1 }] }, false);
});
afterEach(async () => { threadWriter.evict(native.id); await setLocale('en'); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('retains the conversation without capability guesses, and contains an incomplete frame with Retry in %s/%s', async (locale, look) => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', look);
  const missingCan = { ...native };
  Reflect.deleteProperty(missingCan, 'can');
  expect(isThreadForScreen(missingCan, native.id)).toBe(true);
  expect(threadCapabilities(missingCan)).toBeNull(); expect(mayExportThread(missingCan)).toBeNull();
  act(() => threadWriter.apply(native.id, { thread: { ...missingCan, seq: native.seq + 1 } }));
  show(<><input aria-label="Another view's draft" defaultValue="Keep these words" /><ThreadScreen threadId={native.id} item={null} /></>);
  expect(screen.getByRole('heading', { name: native.name })).toBeVisible();
  expect(screen.getByText('The redirect has been checked.')).toBeVisible();
  const thin = { ...native, seq: native.seq + 2 };
  Reflect.deleteProperty(thin, 'input_access');
  act(() => threadWriter.apply(native.id, { thread: thin }));
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('thread.screen.incomplete'));
  expect(screen.getByRole('textbox', { name: "Another view's draft" })).toHaveValue('Keep these words');
  expect(threadViewOf(native.id).items.has('answer')).toBe(true);
  expect(calls.reload).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(calls.reload).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(native.id));
  act(() => threadWriter.apply(native.id, { thread: { ...native, name: 'The redirect is fixed', seq: native.seq + 3 } }));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'The redirect is fixed' })).toBeVisible();
  expect(screen.getByText('The redirect has been checked.')).toBeVisible();
  expect(screen.getByRole('textbox', { name: "Another view's draft" })).toHaveValue('Keep these words');
  expect(calls.reload).toHaveBeenCalledTimes(1);
});

it.each([{ hiddenFor: 15_000, hold: false }, { hiddenFor: 6 * 60_000, hold: true }])('keeps the actual thread screen and cut after $hiddenFor hidden milliseconds with pending work=$hold', { timeout: 30_000 }, async ({ hiddenFor, hold }) => {
  skipOverflowFallback();
  const live = await vi.importActual<typeof Live>('@/realtime/live');
  live.feeds.clear(); threadWriter.clear(); calls.native = true;
  calls.feed.mockImplementation(live.useThreadFeed); calls.reload.mockImplementation(live.reloadThread);
  const server = serve(), owner = 'retained-native-screen', read = resultThreadRead(native.id, owner);
  const listeners = new Map<string, FeedListener>();
  const subscribed = vi.spyOn(live.socket, 'subscribe').mockImplementation((feed, listener) => {
    listeners.set(feed, listener); return { unsubscribe: () => { if (listeners.get(feed) === listener) listeners.delete(feed); } };
  });
  const watched = vi.spyOn(live.socket, 'visible');
  function Slot() { useLayoutEffect(() => showViewOpening(owner, 'native-route'), []); return null; }
  function Kept({ shown }: { shown: boolean }) { return <ThreadReadProvider read={read}>
    <ViewOpening view={owner} route="native-route" hold={hold}><ThreadScreen threadId={native.id} item={null} /></ViewOpening>{shown && <Slot />}
  </ThreadReadProvider>; }
  vi.useFakeTimers();
  const view = show(<Kept shown />);
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(threadViewOf(read).read?.ready).toBe(true);
    act(() => threadWriter.older(read, [answer(OLDER, 'Older native history', undefined, 5)], false, captureThreadRead(read)));
    const before = threadViewOf(read), cut = before.read;
    const viewport = view.container.querySelector('[data-scroll-area]');
    if (!(viewport instanceof HTMLElement)) throw new Error('The actual ThreadScreen viewport must be mounted');
    viewport.scrollTop = 317;
    const draft = screen.getByRole('textbox', { name: 'Thread draft' });
    (draft as HTMLTextAreaElement).value = 'Retained local draft';
    const listener = listeners.get(`thread:${native.id}`);
    if (listener === undefined) throw new Error('The actual thread must own a native subscription');
    view.rerender(<Kept shown={false} />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(listeners.has(`thread:${native.id}`)).toBe(false);
    expect(watched).toHaveBeenLastCalledWith(`thread:${native.id}`, false);
    await act(async () => {
      listener.onFrame({ op: 'resync', id: 1, feed: `thread:${native.id}`, reload: true });
      await vi.advanceTimersByTimeAsync(hiddenFor);
    });
    expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(1);
    expect(threadViewOf(read).items.has(OLDER)).toBe(true); expect(threadViewOf(read).read?.identity).toBe(cut?.identity);
    view.rerender(<Kept shown />); await act(async () => { await vi.advanceTimersByTimeAsync(50); });
    expect(listeners.get(`thread:${native.id}`)?.since()).toBe(20);
    expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(1);
    expect(screen.getByText('Older native history')).toBeVisible();
    expect(view.container.querySelector('[data-scroll-area]')).toBe(viewport); expect(viewport.scrollTop).toBe(317);
    expect(screen.getByRole('textbox', { name: 'Thread draft' })).toBe(draft); expect(draft).toHaveValue('Retained local draft');
    expect(threadViewOf(read).read?.epoch).toBe(cut?.epoch);
  } finally {
    view.unmount(); live.feeds.clear(); subscribed.mockRestore(); watched.mockRestore();
    calls.native = false; calls.feed.mockImplementation(() => undefined); vi.unstubAllGlobals(); vi.useRealTimers(); forgetViewOpening(owner);
  }
});

it('pauses an early thread read when its slot hides before its screen mounts', async () => {
  const live = await vi.importActual<typeof Live>('@/realtime/live');
  live.feeds.clear(); threadWriter.clear(); const server = serve(), read = resultThreadRead(native.id, 'early-thread-slot');
  const listeners = new Set<FeedListener>();
  const subscribed = vi.spyOn(live.socket, 'subscribe').mockImplementation((_feed, listener) => {
    listeners.add(listener); return { unsubscribe: () => { listeners.delete(listener); } };
  });
  vi.useFakeTimers(); const warm = live.warmThread(read, 10 * 60_000);
  try {
    await vi.advanceTimersByTimeAsync(0); expect(listeners.size).toBe(1);
    const before = threadViewOf(read), old = [...listeners][0];
    warm.shown(false); expect(listeners.size).toBe(0);
    old?.onFrame({ op: 'resync', id: 1, feed: `thread:${native.id}`, reload: true });
    await vi.advanceTimersByTimeAsync(6 * 60_000);
    expect(threadViewOf(read).read).toBe(before.read); expect(server.calls).toHaveLength(1);
    warm.shown(true); await vi.advanceTimersByTimeAsync(0);
    expect(listeners.size).toBe(1); expect([...listeners][0]?.since()).toBe(20); expect(server.calls).toHaveLength(1);
  } finally { warm(); live.feeds.clear(); subscribed.mockRestore(); vi.unstubAllGlobals(); vi.useRealTimers(); }
});

it('refuses foreign, incomplete or wrong-thread frames and keeps unknown capabilities distinct from denied or granted', () => {
  for (const value of [null, [], {}, { id: native.id, kind: 'thread', name: native.name }, { ...native, id: 'another' }, { ...native, kind: 'topic' }, { ...native, binding: {} }, { ...native, owner: null }]) {
    expect(isThreadForScreen(value, native.id)).toBe(false);
  }
  expect(threadCapabilities({ can: { export: 'true', fork: null, archive: false, place: true } })).toEqual({ archive: false, place: true });
  expect(threadCapabilities({ ...native, can: undefined })).toBeNull();
  expect(mayExportThread({ ...native, can: { ...native.can, export: false } })).toBe(false);
  expect(mayExportThread({ ...native, can: { ...native.can, export: true } })).toBe(true);
});

it('profiles thread screen renders while a running turn receives live answers with a large cache', () => {
  skipOverflowFallback();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  for (let i = 0; i < 5_000; i++) cache.setQueryData(['background', i], i);
  const durations: number[] = [];
  calls.native = true;
  threadWriter.apply(native.id, { thread: { ...native, state: 'running', seq: native.seq + 1 } });
  const view = show(<KeptQueryScope shown><Profiler id="running-thread" onRender={(_id, _phase, ms) => durations.push(ms)}>
    <ThreadScreen threadId={native.id} item={null} />
  </Profiler></KeptQueryScope>, cache);
  try {
    durations.length = 0;
    const scans = vi.spyOn(cache.getQueryCache(), 'getAll');
    for (let i = 0; i < 10; i++) {
      act(() => threadWriter.apply(native.id, { items: [{ ...agentMessage('answer', `Running output ${i}`), thread_id: native.id, seq_created: 1, seq_updated: i + 2 }] }));
    }
    expect(view.container).toHaveTextContent('Running output 9');
    expect(durations.length).toBeGreaterThan(0);
    process.stdout.write(`Running thread React updates ${JSON.stringify({ commits: durations.length, maxMs: Math.max(...durations) })}\n`);
    expect(scans).not.toHaveBeenCalled();
  } finally { view.unmount(); cache.clear(); }
});
