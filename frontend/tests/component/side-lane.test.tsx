/** An aside opens its own compact conversation and composer at the source turn without driving the parent. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError, client } from '@/api/client';
import type * as ThreadsApi from '@/api/threads';
import type * as NodesApi from '@/api/nodes';
import type * as ForksApi from '@/api/forks';
import type * as SourceApi from '@/api/source';
import type * as Live from '@/realtime/live';
import type * as OpenView from '@/features/workbench/openView';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { SideLaneSlot, SideThreadLaneRow, SideThreadPanel } from '@/features/timeline/SideThreadLane';
import { SideThreadList } from '@/features/side-threads/SideChips';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { sideLaneKey, timelineUi } from '@/features/timeline/state';
import type { ContentPart, Draft, Item, MarkerSideClosed, NodeRef, Thread } from '@/generated/api-types';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { defaultThreadRead, resultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { agentMessage, change, command, fileChange, LOGIN_DIFF, marker, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  draft: vi.fn<() => Promise<Draft | null>>(), clear: vi.fn<() => Promise<void>>(),
  send: vi.fn<typeof ThreadsApi.sendInput>(), stop: vi.fn<typeof ThreadsApi.interruptTurn>(),
  remove: vi.fn<typeof NodesApi.deleteNode>(), older: vi.fn<typeof ThreadsApi.fetchItems>(),
  sides: vi.fn<() => Promise<Thread[]>>(),
  panel: vi.fn(), feed: vi.fn<(id: ThreadReadTarget) => void>(), release: vi.fn<(id: ThreadReadTarget) => void>(), ready: true,
}));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), sendInput: calls.send, interruptTurn: calls.stop, fetchItems: calls.older,
  draftQuery: (id: string) => ({ queryKey: ['thread-draft', id], queryFn: calls.draft, staleTime: Infinity }) }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof SourceApi>(), clearDraftText: calls.clear }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof NodesApi>(), deleteNode: calls.remove }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(), sidesQuery: (id: string) => ({ queryKey: ['thread-sides', id, 'open'], queryFn: calls.sides }) }));
vi.mock('@/realtime/live', async (load) => {
  const { useEffect } = await import('react');
  return { ...await load<typeof Live>(), useSocketStatus: () => ({ phase: calls.ready ? 'ready' : 'connecting' }),
    useThreadFeed: (id: ThreadReadTarget) => { useEffect(() => { calls.feed(id); return () => calls.release(id); }, [id]); } };
});
vi.mock('@/features/workbench/openView', async (load) => ({ ...await load<typeof OpenView>(), revealView: calls.panel }));

const PARENT = 'lane-parent', SIDE = 'lane-side';
const SIDE_READ = defaultThreadRead(SIDE);
const ADA = userRef(me(1, 'ada', 'Ada'));
const OTHER = userRef(me(2, 'grace', 'Grace'));
const reference: NodeRef = { id: SIDE, kind: 'thread', name: 'Login (side)', redacted: false };
const context: TimelineContext = { threadId: PARENT, threadName: 'Login', detail: 'steps', recent: new Set(['main-turn']), layout: 'one', setLayout: () => undefined,
  meId: ADA.id, canInteract: true, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const closed: MarkerSideClosed = { side: reference, by: ADA, turns: 2, why: 'closed', into: null };
const numbered = (item: Item, seq: number): Item => ({ ...item, thread_id: SIDE, segment_id: 'side-segment', seq_created: seq, seq_updated: seq });
function aside(more: Partial<Thread> = {}): Thread {
  return { id: SIDE, kind: 'thread', name: reference.name, seq: 20, revision: 1, state: 'idle', archived: false, lock: null, blocked: null,
    side_of: { id: PARENT, name: 'Login', kind: 'thread', redacted: false }, side_from_turn: 1, side_reading: 'read_only', sides: null,
    owner: ADA, binding: null, cwd: '/workspace/web', directory: 'web', environment: null, environment_permissions: [],
    can: { fork: true }, access: { permissions: ['thread.view', 'thread.interact'] },
    queue: { held: false, revision: 1, by: null, held_inputs: 0 }, ...more } as Thread;
}
function seed(more: Partial<Thread> = {}, running = false) {
  // A second native snapshot changes the same turn only at a newer sequence.
  const seq = (threadViewOf(SIDE).thread?.seq ?? 19) + 1;
  threadWriter.reset(PARENT, { thread: { ...aside(), id: PARENT, side_of: null, name: 'Login', state: 'running', current_turn_id: 'main-turn' },
    turns: [turn('main-turn', { ordinal: 1, inputId: 'main-input', state: 'running' })], items: [agentMessage('main-answer', 'The parent keeps working.', { turn: 'main-turn' })] }, false);
  threadWriter.reset(SIDE, { thread: aside({ seq, state: running ? 'running' : 'idle', current_turn_id: running ? 'side-turn' : null, ...more }), turns: [{ ...turn('side-turn', { ordinal: 1, inputId: 'side-input', state: running ? 'running' : 'completed' }), seq_updated: seq }],
    items: [numbered(userMessage('side-question', 'Which redirect test?', { author: ADA, turn: 'side-turn' }), 1),
      numbered(command('side-command', { command: 'check redirect', output: 'redirect is covered', exit: 0, turn: 'side-turn' }), 2),
      numbered(agentMessage('side-answer', 'The same-origin test covers it.', { turn: 'side-turn' }), 3)] }, false);
}
const draw = (open = true, parent = context) => {
  timelineUi.setStep(PARENT, sideLaneKey(SIDE), open);
  return show(<TimelineContextValue.Provider value={parent}><SideLaneSlot side={reference} by={ADA} /></TimelineContextValue.Provider>);
};
let originalLocale = currentLocale();
let appearance = useAppearance.getState();
let previousUser = client.session.getState().userId;
beforeEach(() => {
  originalLocale = currentLocale(); appearance = useAppearance.getState(); previousUser = client.session.getState().userId;
  calls.draft.mockReset().mockResolvedValue(null); calls.clear.mockReset().mockResolvedValue(undefined); calls.send.mockReset(); calls.stop.mockReset();
  calls.remove.mockReset(); calls.older.mockReset(); calls.sides.mockReset().mockResolvedValue([]); calls.panel.mockClear(); calls.feed.mockClear(); calls.release.mockClear(); calls.ready = true;
  client.session.setState({ userId: ADA.id }); useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {} });
  timelineUi.forget(PARENT); threadWriter.evict(PARENT); threadWriter.evict(SIDE); seed();
});
afterEach(async () => {
  cleanup(); threadWriter.evict(PARENT); threadWriter.evict(SIDE); threadWriter.evict(resultThreadRead(SIDE, 'side-tab')); timelineUi.forget(PARENT);
  useComposerStore.setState({ text: {}, parts: {}, sending: {}, refused: {} }); client.session.setState({ userId: previousUser }); useAppearance.setState(appearance); await setLocale(originalLocale);
});

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('a shared aside lane in %s / %s', (locale, theme) => {
  beforeEach(async () => { await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme }); });

  it('uses the same bordered frame, messages and command displays as a child stream, then follows the actual answer', async () => {
    draw();
    const lane = screen.getByRole('region', { name: i18n.t('side.lane.label', { name: reference.name }) });
    expect(lane).toHaveAttribute('data-conversation-lane', 'side'); expect(lane).toHaveAttribute('data-density', 'compact');
    expect(lane).toHaveClass('border-l-2', 'pl-3');
    const log = within(lane).getByRole('log', { name: i18n.t('side.lane.conversation') });
    expect(within(log).getByText('Which redirect test?')).toBeVisible();
    expect(within(log).getByRole('button', { name: /check redirect/ })).toBeVisible();
    expect(within(log).getByText('The same-origin test covers it.')).toBeVisible();
    expect(log).not.toHaveTextContent('The parent keeps working.');
    expect(within(lane).getByText(i18n.t('side.ask.mode', { mode: i18n.t('side.reading.read_only') }))).toBeVisible();
    act(() => threadWriter.apply(SIDE, { items: [numbered(agentMessage('live-answer', 'The cookie test also covers expiry.', { turn: 'side-turn' }), 21)] }));
    expect(within(log).getByText('The cookie test also covers expiry.')).toBeVisible();
    expect(threadViewOf(PARENT).items.has('live-answer')).toBe(false);
    await waitFor(() => expect(calls.draft).toHaveBeenCalledTimes(1));
    expect(calls.feed).toHaveBeenCalledExactlyOnceWith(SIDE_READ);
  });

  it('sends from its own inline box on Enter, keeps Shift/IME in the box, and preserves the parent draft and turn', async () => {
    const input = { ...threadInput('accepted-side-input', 'Explain the expiry', { author: ADA, position: '1' }), thread_id: SIDE, seq_updated: 22 };
    calls.send.mockResolvedValue({ input, seq: 22 });
    composerWriter.setText(PARENT, 'The next main task');
    draw();
    const editor = screen.getByRole('textbox', { name: i18n.t('side.lane.box', { name: reference.name }) });
    await userEvent.type(editor, 'Explain the expiry');
    fireEvent.keyDown(editor, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(editor, { key: 'Enter', keyCode: 229 });
    fireEvent.keyDown(editor, { key: 'Enter', shiftKey: true });
    expect(calls.send).not.toHaveBeenCalled();
    fireEvent.keyDown(editor, { key: 'Enter' });
    await waitFor(() => expect(calls.send).toHaveBeenCalledTimes(1));
    const [sentTo, body] = calls.send.mock.calls[0]!;
    expect(sentTo).toEqual(SIDE_READ);
    expect(typeof body.clientId).toBe('string');
    expect(body.clientId).not.toBe('');
    expect(body).toEqual({ content: [{ type: 'text', text: 'Explain the expiry' }], delivery: 'queue', clientId: body.clientId });
    expect(editor).toHaveValue('');
    expect(useComposerStore.getState().text[PARENT]).toBe('The next main task');
    expect(threadViewOf(PARENT).turns.get('main-turn')?.state).toBe('running');
    expect(calls.stop).not.toHaveBeenCalled();
  });

  it('shows a viewer the aside and one watch notice, without a composer, Keep or Close', async () => {
    seed({ access: { ...aside().access, permissions: ['thread.view'] }, can: { ...aside().can, fork: false } });
    client.session.setState({ userId: OTHER.id }); draw();
    expect(screen.getByText('The same-origin test covers it.')).toBeVisible();
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('side.close') })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('side.keep') })).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('side.lane.readOnly'));
    expect(calls.draft).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('side.lane.openPanel') }));
    expect(calls.panel).toHaveBeenCalledExactlyOnceWith('side-thread', { thread: PARENT, side: SIDE });
    expect(calls.release).toHaveBeenCalledExactlyOnceWith(SIDE_READ);
  });
});

it('mounts nothing for folded, closed or retained side rows and closes to its actual turn count', async () => {
  draw(false);
  expect(calls.feed).not.toHaveBeenCalled(); expect(calls.draft).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: /Login \(side\)/, expanded: false }));
  expect(calls.feed).toHaveBeenCalledExactlyOnceWith(SIDE_READ);
  cleanup(); calls.feed.mockClear(); calls.draft.mockClear();
  show(<TimelineContextValue.Provider value={context}><SideLaneSlot side={reference} closed={closed} /></TimelineContextValue.Provider>);
  expect(screen.getByText(i18n.t('side.lane.closed', { name: reference.name, count: 2 }))).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument(); expect(calls.feed).not.toHaveBeenCalled(); expect(calls.draft).not.toHaveBeenCalled();
  cleanup();
  draw(true, { ...context, inspection: { threadId: PARENT } });
  expect(screen.getByText(i18n.t('side.lane.retained'))).toBeVisible(); expect(calls.feed).not.toHaveBeenCalled();
});

it('updates the anchored row from its committed close without reading a deleted side', () => {
  const began = marker('begin', { marker: 'side_started', data: { side: reference, by: ADA, from_turn: 1 } });
  act(() => threadWriter.apply(PARENT, { items: [{ ...began, thread_id: PARENT, seq_created: 21, seq_updated: 21 }] }));
  timelineUi.openSide(PARENT, SIDE);
  show(<TimelineContextValue.Provider value={context}><SideThreadLaneRow id="begin" closedId="end" /></TimelineContextValue.Provider>);
  expect(document.querySelector('[data-side-lane]')).not.toBeNull();
  const ended = marker('end', { marker: 'side_closed', data: closed });
  act(() => threadWriter.apply(PARENT, { items: [{ ...ended, thread_id: PARENT, seq_created: 22, seq_updated: 22 }] }));
  expect(document.querySelector('[data-side-lane]')).toBeNull();
  expect(screen.getByText(i18n.t('side.lane.closed', { name: reference.name, count: 2 }))).toBeVisible();
  expect(calls.release).toHaveBeenCalledExactlyOnceWith(SIDE_READ);
});

it('keeps a locked or offline aside draft writable but never sends it or interrupts its parent', async () => {
  calls.ready = false; seed({}, true); draw();
  await userEvent.type(screen.getByRole('textbox'), 'Keep this until the connection returns');
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
  expect(calls.send).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: i18n.t('composer.stop') })).toBeDisabled();
  expect(screen.getByText(i18n.t('composer.reason.offline'))).toBeVisible();
  expect(useComposerStore.getState().text[SIDE]).toBe('Keep this until the connection returns');
});

it('queues while its own turn runs, and its Stop names only that side turn', async () => {
  seed({}, true);
  calls.stop.mockResolvedValue({ ...turn('side-turn', { ordinal: 1, inputId: 'side-input', state: 'interrupted' }), seq_updated: 22 });
  const input = { ...threadInput('queued-side', 'Then check expiry', { author: ADA, position: '1' }), thread_id: SIDE, seq_updated: 23 };
  calls.send.mockResolvedValue({ input, seq: 23 });
  draw();
  await userEvent.type(screen.getByRole('textbox'), 'Then check expiry');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('side.lane.queue') }));
  await waitFor(() => expect(document.querySelector('[data-queue]')).toHaveTextContent('Then check expiry'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  expect(calls.stop).toHaveBeenCalledExactlyOnceWith(SIDE_READ, 'side-turn');
  expect(threadViewOf(PARENT).turns.get('main-turn')?.state).toBe('running');
});

it('keeps restored files and newer words when a side send is refused', async () => {
  calls.send.mockRejectedValue(new ApiError({ type: 'about:blank', title: 'Refused', status: 403, code: 'authz.denied', detail: 'This provider is no longer available to you.', request_id: 'side-send' }, null));
  const file: ContentPart = { type: 'image', blob: 'diagram', name: 'diagram.png', media_type: 'image/png' };
  composerWriter.setText(SIDE, 'Explain this diagram'); composerWriter.setParts(SIDE, [file]);
  draw();
  expect(screen.getByText('diagram.png')).toBeVisible();
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
  await waitFor(() => expect(screen.getByRole('textbox')).toHaveValue('Explain this diagram'));
  expect(useComposerStore.getState().parts[SIDE]).toEqual([file]);
  expect(calls.send.mock.calls[0]?.[1].content).toEqual([{ type: 'text', text: 'Explain this diagram' }, file]);
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('errors.authz.denied'));
});

it('keeps an unconfirmed side message in its own queue and retries with the same identity', async () => {
  const input = { ...threadInput('retry-side', 'An unconfirmed question', { author: ADA, position: '1' }), thread_id: SIDE, seq_updated: 22 };
  calls.send.mockRejectedValueOnce(new Error('The answer was lost.')).mockResolvedValue({ input, seq: 22 });
  draw();
  await userEvent.type(screen.getByRole('textbox'), 'An unconfirmed question');
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter' });
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(calls.send).toHaveBeenCalledTimes(1);
  await userEvent.click(retry);
  await waitFor(() => expect(calls.send).toHaveBeenCalledTimes(2));
  expect(calls.send.mock.calls[1]).toEqual(calls.send.mock.calls[0]);
  expect(threadViewOf(PARENT).inputs.has(input.id)).toBe(false);
});

it('uses the same side draft and full tool displays when moved to its panel', async () => {
  act(() => threadWriter.apply(SIDE, { items: [numbered(fileChange('side-edit', { turn: 'side-turn', changes: [change('notes/login.md', LOGIN_DIFF)] }), 21)] }));
  const query = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  // Populate once, then mount the same editor again in the panel; only one server draft is read.
  const first = show(<TimelineContextValue.Provider value={context}><SideLaneSlot side={reference} /></TimelineContextValue.Provider>);
  await userEvent.click(screen.getByRole('button', { expanded: false, name: /Login \(side\)/ }));
  await waitFor(() => expect(calls.draft).toHaveBeenCalledTimes(1));
  act(() => composerWriter.setText(SIDE, 'Continue in the panel'));
  first.unmount();
  // This represents the workbench's shared QueryClient, rather than a second application.
  query.setQueryData(['thread-draft', SIDE], null);
  // This unit stubs following entirely. Seed the new panel owner with its own synthetic
  // native cut; the browser/reader suites separately forbid borrowing another live window.
  const panelRead = resultThreadRead(SIDE, 'side-tab');
  const fixture = threadViewOf(SIDE);
  threadWriter.reset(panelRead, { thread: fixture.thread, items: [...fixture.items.values()],
    turns: [...fixture.turns.values()], segments: fixture.segments, inputs: [...fixture.inputs.values()] }, fixture.hasOlder);
  show(<QueryClientProvider client={query}><SideThreadPanel tab={{ id: 'side-tab', view: 'side-thread', of: { thread: PARENT, side: SIDE } }} area="right" /></QueryClientProvider>);
  expect(document.querySelector('[data-side-panel]')).toHaveAttribute('data-side-panel', SIDE);
  expect(screen.getByRole('textbox')).toHaveValue('Continue in the panel');
  expect(calls.feed).toHaveBeenLastCalledWith(panelRead);
  expect(screen.queryByRole('table', { name: /notes\/login.md/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { expanded: false, name: /Edited notes\/login.md/ }));
  expect(screen.getByRole('table', { name: /notes\/login.md/ })).toBeVisible();
  expect(calls.draft).toHaveBeenCalledTimes(1);
});

it('lets the maker or a parent manager close, without giving another interactor that control', async () => {
  client.session.setState({ userId: OTHER.id });
  const notManager = draw();
  expect(screen.queryByRole('button', { name: i18n.t('side.close') })).not.toBeInTheDocument();
  notManager.unmount();
  calls.remove.mockResolvedValue({ deletion_id: null });
  draw(true, { ...context, canManage: true });
  await userEvent.click(screen.getByRole('button', { name: i18n.t('side.close') }));
  await waitFor(() => expect(document.querySelector('[data-side-lane]')).toBeNull());
  expect(calls.remove).toHaveBeenCalledExactlyOnceWith(SIDE);
  expect(screen.getByText(i18n.t('side.lane.closedUnknown', { name: reference.name }))).toBeVisible();
  expect(threadViewOf(PARENT).turns.get('main-turn')?.state).toBe('running');
});

it('reads older side messages only on demand, and never reads a hidden panel or private side reference', async () => {
  show(<ViewOpeningContext.Provider value={null}><SideThreadPanel tab={{ id: 'hidden-side', view: 'side-thread', of: { thread: PARENT, side: SIDE } }} area="right" /></ViewOpeningContext.Provider>);
  expect(calls.feed).not.toHaveBeenCalled(); expect(calls.draft).not.toHaveBeenCalled();
  cleanup();
  show(<TimelineContextValue.Provider value={context}><SideLaneSlot side={{ ...reference, redacted: true, name: null }} /></TimelineContextValue.Provider>);
  expect(screen.queryByText('Login (side)')).not.toBeInTheDocument(); expect(calls.feed).not.toHaveBeenCalled();
  cleanup();
  threadWriter.reset(SIDE, { thread: aside(), items: [numbered(agentMessage('latest', 'A later answer'), 20)] }, true);
  calls.older.mockResolvedValue({ items: [numbered(agentMessage('earlier', 'An earlier answer'), 1)], turns: [], segments: [], has_older: false, has_newer: false });
  draw();
  expect(calls.older).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('side.lane.older') }));
  expect(await screen.findByText('An earlier answer')).toBeVisible();
  expect(calls.older).toHaveBeenCalledWith(SIDE_READ, { before: 20, stream: 'main' }, 100, expect.any(AbortSignal));
});

it('keeps the open list current from the parent markers without another list or per-side read', async () => {
  const parent = { ...aside(), id: PARENT, side_of: null, sides: { open: 1, max: 4, allowed: true, mode: 'read_only', reason: null } } as Thread;
  calls.sides.mockResolvedValue([aside()]);
  const reveal = vi.fn(), opened = vi.fn();
  show(<SideThreadList thread={parent} onRevealItem={reveal} onOpened={opened} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Login (side)' })).toBeVisible());
  const began = marker('list-start', { marker: 'side_started', data: { side: reference, by: ADA, from_turn: 1 } });
  act(() => threadWriter.apply(PARENT, { items: [{ ...began, seq_created: 21, seq_updated: 21 }] }));
  await userEvent.click(screen.getByRole('button', { name: 'Login (side)' }));
  expect(reveal).toHaveBeenCalledExactlyOnceWith('list-start');
  expect(opened).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('side.lane.openPanel') }));
  expect(calls.panel).toHaveBeenCalledExactlyOnceWith('side-thread', { thread: PARENT, side: SIDE });
  expect(opened).toHaveBeenCalledTimes(2);
  const ended = marker('list-end', { marker: 'side_closed', data: closed });
  act(() => threadWriter.apply(PARENT, { items: [{ ...ended, seq_created: 22, seq_updated: 22 }] }));
  expect(screen.queryByRole('button', { name: 'Login (side)' })).not.toBeInTheDocument();
  expect(calls.sides).toHaveBeenCalledTimes(1); expect(calls.feed).not.toHaveBeenCalled(); expect(calls.draft).not.toHaveBeenCalled();
});
