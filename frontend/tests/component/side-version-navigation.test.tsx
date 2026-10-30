/** Side-local version locators keep real item anchors; synthetic HTTP proves client ownership only. */
import { Activity, useState } from 'react';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { captureThreadRead } from '@/api/threadRead';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { SideLaneSlot, SideThreadPanel } from '@/features/timeline/SideThreadLane';
import { sideLaneKey, timelineUi } from '@/features/timeline/state';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import type * as Live from '@/realtime/live';
import { feeds, reloadThread, socket } from '@/realtime/live';
import { defaultThreadRead, resultThreadRead } from '@/realtime/threadScope';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import type { Item, ThreadContinuationProjection, ThreadItemListReply, ThreadPromptSummary, ThreadPromptVersion, ThreadSync } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { agentMessage, userMessage } from '../doubles/thread-rows';
import { A, B, C, deferred, opening, projection, snapshot, summary, viewedPrompt } from '../unit/continuation-fixtures';
import { show } from './support';

vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));

const PARENT = snapshot().thread.id;
const SIDE = '01950000-0000-7000-8000-000000000020';
const PANEL = 'side-version-panel';
const GROUP = '01950000-0000-7000-8000-000000000030';
const IA = '01950000-0000-7000-8000-000000000031', IB = '01950000-0000-7000-8000-000000000032', IC = '01950000-0000-7000-8000-000000000033';
const DA = '01950000-0000-7000-8000-000000000041', DB = '01950000-0000-7000-8000-000000000042', DC = '01950000-0000-7000-8000-000000000043';
const owner = snapshot().thread.owner;
const panelRead = resultThreadRead(SIDE, PANEL), sideDefault = defaultThreadRead(SIDE);
const parentContext: TimelineContext = { threadId: PARENT, threadName: 'Parent stays selected', detail: 'steps', recent: new Set(),
  layout: 'one', setLayout: () => undefined, meId: owner.id, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const reference = { id: SIDE, kind: 'thread' as const, name: 'Versioned aside', redacted: false };
let previousWorkbench = useWorkbench.getState();
let previousSession = client.session.getState();
let previousComposer = useComposerStore.getState();

function version(id: string): ThreadPromptVersion {
  const ordinal = id === A ? 1 : id === B ? 2 : 3;
  const input = id === A ? IA : id === B ? IB : IC, item = id === A ? DA : id === B ? DB : DC;
  return { ...viewedPrompt('ready', id), id: input, input_id: input, item_id: item, group_id: GROUP, author: owner, ordinal, total: 3,
    edit: { available: false, reason: 'native_boundary_unsupported', detail: null } };
}
function prompt(version: ThreadPromptVersion): ThreadPromptSummary {
  return { ...summary(version.input_id, version.continuation_id), author: owner, group_id: GROUP, ordinal: version.ordinal,
    total: version.total, edit: version.edit };
}
function spoken(version: ThreadPromptVersion): Item {
  const letter = version.continuation_id === A ? 'A' : version.continuation_id === B ? 'B' : 'C';
  if (version.item_id === null) throw new Error('Only actual delivered item IDs enter a native item page');
  return { ...userMessage(version.item_id, `Prompt ${letter}`, { author: owner, inputId: version.input_id }),
    thread_id: SIDE, continuation_id: version.continuation_id, seq_created: 5, seq_updated: 5 };
}
function sideCut(scope: ThreadContinuationProjection, current: ThreadPromptVersion, count: number, include: boolean): ThreadSync {
  const cut = snapshot(scope, 500);
  const rows = Array.from({ length: count - (include ? 1 : 0) }, (_, index): Item => ({
    ...agentMessage(`tail-${current.continuation_id}-${index}`, `Later aside answer ${index}`), thread_id: SIDE,
    continuation_id: current.continuation_id, seq_created: 100 + index, seq_updated: 100 + index,
  }));
  return { ...cut, thread: { ...cut.thread, id: SIDE, name: reference.name, state: 'idle', current_turn_id: null,
    side_of: { id: PARENT, kind: 'thread', name: parentContext.threadName, redacted: false }, side_reading: 'read_only',
    active_continuation_id: scope.active_continuation_id, execution_revision: scope.execution_revision,
    access: { ...cut.thread.access, permissions: ['thread.view', 'thread.interact'] } },
    items: include ? [spoken(current), ...rows] : rows, prompt_summaries: include ? [prompt(current)] : [],
    viewed_prompt: scope.viewed_continuation_id === A ? null : current, has_older: true };
}
function transport() {
  const calls: Request[] = [];
  const versions = new Map<string, ThreadPromptVersion>([A, B, C].map((id) => [id, version(id)]));
  const visible = new Set([A, B]);
  const state: { count: number; active: string; execution: number;
    around: ((request: Request) => Promise<Response>) | null; older: ((request: Request) => Promise<Response>) | null }
    = { count: 3, active: A, execution: 4, around: null, older: null };
  const current = (id: string) => {
    const found = versions.get(id); if (found === undefined) throw new Error('Unknown synthetic continuation'); return found;
  };
  const scope = (id: string, follows = false): ThreadContinuationProjection => ({ ...projection(id, state.active, id === A ? 1 : id === B ? 2 : 3, follows), execution_revision: state.execution });
  const page = (id: string): ThreadItemListReply => ({ projection: scope(id), prompt_summaries: [prompt(current(id))],
    items: [spoken(current(id))], turns: [], segments: [], has_older: true, has_newer: true });
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init), url = new URL(request.url); calls.push(request);
    const match = /^\/api\/v1\/threads\/([^/]+)(.*)$/.exec(url.pathname);
    if (match?.[1] !== SIDE || match[2] === undefined) return Promise.reject(new Error('A side read must keep its actual native thread ID'));
    const selected = url.searchParams.get('continuation_id'), id = selected ?? state.active, suffix = match[2];
    if (suffix === '/opening' || suffix === '/sync') {
      expect(url.searchParams.get('limit')).toBe('200');
      const cut = sideCut(scope(id, selected === null), current(id), state.count, visible.has(id));
      return Promise.resolve(Response.json(suffix === '/opening' ? opening(cut) : cut));
    }
    if (suffix === '/draft') return Promise.resolve(Response.json(null));
    if (suffix.endsWith('/versions')) {
      const adjacent = url.searchParams.has('before') ? (id === C ? B : A) : (id === A ? B : C);
      return Promise.resolve(Response.json({ projection: scope(id, selected === null), group_id: GROUP, rows: [current(adjacent)], before: null, after: null }));
    }
    if (suffix === '/items' && url.searchParams.has('around')) {
      if (state.around !== null) return state.around(request);
      expect(url.searchParams.get('around')).toBe(current(id).item_id);
      return Promise.resolve(Response.json(page(id)));
    }
    if (suffix === '/items' && url.searchParams.has('before')) {
      if (state.older !== null) return state.older(request);
      return Promise.resolve(Response.json({ ...page(id), items: [], has_older: false, has_newer: false }));
    }
    if (request.method === 'POST' && suffix.endsWith('/activate')) return Promise.resolve(Response.json({ continuation_id: B,
      input_id: IB, operation_id: 'synthetic-explicit-activation', seq: 501,
      job: { ...viewedPrompt('preparing').job!, id: 'synthetic-explicit-activation' } }, { status: 202 }));
    return Promise.reject(new Error(`Unexpected side fixture path ${suffix}`));
  });
  return { calls, state, versions, visible, scope, page, current };
}
function panel() {
  const layout = useWorkbench.getState().layout;
  useWorkbench.setState({ layout: { ...layout, main: { kind: 'group', id: 'side-proof-group', active: PANEL,
    tabs: [{ id: PANEL, view: 'side-thread', of: { thread: PARENT, side: SIDE, continuation: B } }] } } });
  function Harness() {
    const [shown, setShown] = useState(true);
    const tab = useWorkbench((state) => allTabs(state.layout).find((one) => one.id === PANEL));
    if (tab === undefined) throw new Error('The panel owns its real tab');
    return <><button type="button" onClick={() => setShown((held) => !held)}>Toggle panel</button>
      <TimelineContextValue.Provider value={parentContext}><ViewOpeningContext.Provider value={shown ? 'side-proof-opening' : null}>
        <Activity mode={shown ? 'visible' : 'hidden'}><SideThreadPanel tab={tab} area="main" /></Activity>
      </ViewOpeningContext.Provider></TimelineContextValue.Provider></>;
  }
  return show(<Harness />);
}
function inline() {
  timelineUi.setStep(PARENT, sideLaneKey(SIDE), true);
  return show(<TimelineContextValue.Provider value={parentContext}><SideLaneSlot side={reference} /></TimelineContextValue.Provider>);
}
function sideRow(text: string): HTMLElement {
  const element = screen.getByText(text).closest<HTMLElement>('[data-item]');
  if (element === null) throw new Error('An actual delivered item must be visible'); return element;
}
const arounds = (calls: readonly Request[]) => calls.filter((request) => new URL(request.url).pathname.endsWith('/items') && new URL(request.url).searchParams.has('around'));

beforeEach(() => {
  previousWorkbench = useWorkbench.getState(); previousSession = client.session.getState(); previousComposer = useComposerStore.getState();
  feeds.clear(); socket.stop(); threadWriter.clear(); queryClient.clear();
  client.session.setState({ userId: owner.id }); useWorkbench.setState({ userId: null });
  composerWriter.setText(PARENT, 'Unsent parent words');
  threadWriter.reset(PARENT, snapshot(), false, captureThreadRead(PARENT));
  const cut = sideCut(projection(A, A, 1, true), version(A), 3, true);
  threadWriter.reset(sideDefault, cut, false, captureThreadRead(sideDefault));
});
afterEach(() => {
  cleanup(); feeds.clear(); socket.stop(); threadWriter.clear(); queryClient.clear();
  useWorkbench.setState(previousWorkbench); client.session.setState(previousSession); useComposerStore.setState(previousComposer);
  timelineUi.forget(PARENT); timelineUi.forget(panelRead); timelineUi.forget(sideDefault); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it('follows the real beyond-tail side item with one scoped page and restores its own version-strip focus', { timeout: 30_000 }, async () => {
  const server = transport(); server.state.count = 200;
  const before = threadViewOf(PARENT), routeSide = threadViewOf(sideDefault), url = window.location.href;
  const scroll = vi.spyOn(Element.prototype, 'scrollIntoView'); panel();
  await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  const message = await screen.findByText('Prompt C');
  expect(message).toBeVisible(); expect(arounds(server.calls)).toHaveLength(1);
  const request = arounds(server.calls)[0];
  expect(new URL(request?.url ?? 'http://invalid').searchParams.get('around')).toBe(DC);
  expect(new URL(request?.url ?? 'http://invalid').searchParams.get('continuation_id')).toBe(C);
  expect(new URL(request?.url ?? 'http://invalid').searchParams.get('limit')).toBe('100');
  await waitFor(() => expect(within(sideRow('Prompt C')).getByRole('button', { name: i18n.t('messageVersions.previous') })).toHaveFocus());
  expect(scroll).toHaveBeenCalledWith({ block: 'nearest' });
  expect(threadViewOf(panelRead).read?.selection).toBe(C); expect(threadViewOf(panelRead).order).toEqual([DC]);
  expect(threadViewOf(PARENT)).toBe(before); expect(threadViewOf(sideDefault)).toBe(routeSide);
  expect(useComposerStore.getState().text[PARENT]).toBe('Unsent parent words'); expect(window.location.href).toBe(url);
  expect(server.calls.filter((request) => request.method !== 'GET')).toHaveLength(0);
});

it.each(['success', 'failure'] as const)('retires an old around-page %s after another local version is selected', async (result) => {
  const waiting = deferred<Response>(); const server = transport();
  server.state.around = () => waiting.promise; panel(); await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await waitFor(() => expect(arounds(server.calls)).toHaveLength(1));
  // The fallback strip remains a real local navigation while the delivered anchor is being read.
  const navigation = screen.getByRole('group', { name: i18n.t('messageVersions.label') });
  await userEvent.click(within(navigation).getByRole('button', { name: i18n.t('messageVersions.previous') }));
  await screen.findByText('Prompt B'); const currentRead = captureThreadRead(panelRead);
  await act(async () => {
    if (result === 'success') waiting.resolve(Response.json(server.page(C))); else waiting.reject(new Error('Retired side page failed'));
    await Promise.resolve();
  });
  expect(threadViewOf(panelRead).read?.identity).toBe(currentRead.scope.identity);
  expect(threadViewOf(panelRead).read?.epoch).toBe(currentRead.scope.epoch);
  expect(threadViewOf(panelRead).read?.selection).toBe(B); expect(threadViewOf(panelRead).items.has(DC)).toBe(false);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument(); expect(screen.getByText('Prompt B')).toBeVisible();
});

it('retires an earlier-page error when its read is replaced and leaves the new side window usable', async () => {
  const waiting = deferred<Response>(), server = transport(); server.state.older = () => waiting.promise;
  panel(); await screen.findByText('Prompt B');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('side.lane.older') }));
  await waitFor(() => expect(server.calls.some((request) => new URL(request.url).searchParams.has('before'))).toBe(true));
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await screen.findByText('Prompt C');
  await act(async () => { waiting.reject(new Error('Old earlier page failed')); await Promise.resolve(); });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: i18n.t('side.lane.older') })).not.toHaveAttribute('aria-disabled', 'true');
  expect(threadViewOf(panelRead).read?.selection).toBe(C);
});

it('waits for a pending prompt to gain a real delivered item before reading its anchor', async () => {
  const server = transport(); server.versions.set(C, { ...version(C), item_id: null, status: 'preparing', job: viewedPrompt('preparing', C).job });
  panel(); await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await waitFor(() => expect(threadViewOf(panelRead).viewedPrompt?.status).toBe('preparing'));
  expect(arounds(server.calls)).toHaveLength(0);
  server.versions.set(C, version(C));
  act(() => threadWriter.apply(panelRead, { seq: 501, viewed_prompt: version(C) }, captureThreadRead(panelRead)));
  await screen.findByText('Prompt C'); expect(arounds(server.calls)).toHaveLength(1);
  expect(new URL(arounds(server.calls)[0]?.url ?? 'http://invalid').searchParams.get('around')).toBe(DC);
  expect(arounds(server.calls).some((request) => new URL(request.url).searchParams.get('around') === IC)).toBe(false);
});

it('keeps a panel locator across hide/show and Jump to latest reloads only that selected side tail', async () => {
  const server = transport(), before = threadViewOf(PARENT); panel(); await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await screen.findByText('Prompt C');
  const locate = () => allTabs(useWorkbench.getState().layout).find((tab) => tab.id === PANEL)?.of;
  expect(locate()).toMatchObject({ thread: PARENT, side: SIDE, continuation: C, prompt: IC, item: DC });
  await userEvent.click(screen.getByRole('button', { name: 'Toggle panel' }));
  await userEvent.click(screen.getByRole('button', { name: 'Toggle panel' }));
  expect(await screen.findByText('Prompt C')).toBeVisible(); expect(arounds(server.calls)).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.jump') }));
  await waitFor(() => expect(threadViewOf(panelRead).loaded).toBe('tail'));
  expect(locate()).toEqual({ thread: PARENT, side: SIDE, continuation: C });
  expect(threadViewOf(panelRead).read?.selection).toBe(C); expect(threadViewOf(PARENT)).toBe(before);
  expect(arounds(server.calls)).toHaveLength(1);
});

it('an earlier side activation cannot follow active after a new B-to-C-to-B browsing intent', async () => {
  const server = transport(), url = window.location.href, before = threadViewOf(PARENT); inline();
  await screen.findByText('Prompt A');
  await userEvent.click(within(sideRow('Prompt A')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await screen.findByText('Prompt B');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('messageVersions.continueHere') }));
  await waitFor(() => expect(server.calls.filter((request) => request.method === 'POST')).toHaveLength(1));
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await screen.findByText('Prompt C');
  await userEvent.click(within(sideRow('Prompt C')).getByRole('button', { name: i18n.t('messageVersions.previous') }));
  await screen.findByText('Prompt B');
  act(() => threadWriter.apply(sideDefault, { seq: 502, projection: { ...server.scope(B), active_continuation_id: B, execution_revision: 5 },
    viewed_prompt: version(B) }, captureThreadRead(sideDefault)));
  expect(threadViewOf(sideDefault).read?.selection).toBe(B); expect(screen.getByText('Prompt B')).toBeVisible();
  expect(threadViewOf(PARENT)).toBe(before); expect(window.location.href).toBe(url);
  expect(useComposerStore.getState().text[PARENT]).toBe('Unsent parent words');
});

it('drops an old owner page on credential retirement without publishing a failure in the replacement read', async () => {
  const waiting = deferred<Response>(), server = transport(); server.state.around = () => waiting.promise;
  panel(); await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  await waitFor(() => expect(arounds(server.calls)).toHaveLength(1));
  const nextReader = (client.session.getState().readerVersion ?? 0) + 1;
  act(() => { client.session.setState({ readerVersion: nextReader }); threadWriter.rebase(panelRead, nextReader); });
  server.state.around = null;
  act(() => reloadThread(panelRead));
  await screen.findByText('Prompt C'); const fresh = captureThreadRead(panelRead);
  await act(async () => { waiting.reject(new Error('Former reader page failed')); await Promise.resolve(); });
  expect(threadViewOf(panelRead).read?.identity).toBe(fresh.scope.identity);
  expect(threadViewOf(panelRead).read?.epoch).toBe(fresh.scope.epoch);
  expect(threadViewOf(panelRead).items.has(DC)).toBe(true); expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(threadViewOf(panelRead).read?.reader).toBe(nextReader);
});


it('keeps a current anchor failure explicit and makes only one new bounded page when Retry is chosen', async () => {
  const server = transport(); server.state.around = () => Promise.reject(new NetworkError('failed'));
  panel(); await screen.findByText('Prompt B');
  await userEvent.click(within(sideRow('Prompt B')).getByRole('button', { name: i18n.t('messageVersions.next') }));
  const error = await screen.findByRole('alert');
  expect(arounds(server.calls)).toHaveLength(1);
  server.state.around = null;
  await userEvent.click(within(error).getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByText('Prompt C')).toBeVisible();
  expect(arounds(server.calls)).toHaveLength(2); expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(threadViewOf(panelRead).read?.selection).toBe(C);
});
