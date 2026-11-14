/** The common rows keep their order, exact native targets and person-anchored checkpoint. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Threads from '@/api/threads';
import type * as Source from '@/api/source';
import { captureThreadRead } from '@/api/threadRead';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import type { Snapshot } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { threadWriter } from '@/realtime/stores/thread';
import { agentMessage, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot, summary } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ copy: vi.fn(), fork: vi.fn(), reveal: vi.fn(), retry: vi.fn<typeof Threads.retryTurn>(), send: vi.fn<typeof Threads.sendInput>(), checkpoint: vi.fn<(id: string) => Promise<Snapshot>>() }));
vi.mock('@/ui/clipboard', async (original) => ({ ...await original<object>(), copyText: wire.copy }));
vi.mock('@/features/thread/ImmediateFork', () => ({ useImmediateFork: () => ({ pending: false, start: wire.fork, confirmation: null }) }));
vi.mock('@/features/workbench/openView', async (original) => ({ ...await original<object>(), revealView: wire.reveal }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), retryTurn: wire.retry, sendInput: wire.send }));
vi.mock('@/api/source', async (original) => ({ ...await original<typeof Source>(), snapshotQuery: (id: string) => ({ queryKey: ['checkpoint', id], queryFn: () => wire.checkpoint(id) }) }));
const appearance = useAppearance.getState(); const themeBefore = document.documentElement.dataset.theme;
const ids: string[] = [];
afterEach(async () => { cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); vi.resetAllMocks(); useAppearance.setState(appearance);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en'); });

function draw(width: number, middle = false, checkpoint = true, state: 'completed' | 'failed' | 'interrupted' = 'completed') {
  const source = snapshot(); const id = source.thread.id; ids.push(id);
  const completed = { ...turn('hover-turn', { ordinal: 1, state, inputId: 'hover-input', completedAt: '2026-10-01T10:00:12Z' }), before_snapshot_id: checkpoint ? 'before-person-message' : null };
  const person = { ...userMessage('hover-person', 'Fix this route', { author: source.thread.owner, turn: completed.id, inputId: completed.input_id }), continuation_id: A };
  if (person.kind !== 'user_message') throw new Error('The checkpoint belongs to the human input');
  const answer = { ...agentMessage('hover-answer', 'The route is fixed.', { turn: completed.id }), continuation_id: A };
  const input = { ...threadInput(completed.input_id, 'Fix this route', { author: source.thread.owner, position: 'a', state: 'delivered' }), thread_id: id };
  const thread = { ...source.thread, state: 'idle' as const, current_turn_id: null, environment_permissions: ['files.view', 'files.manage'], can: { ...source.thread.can, fork: true } };
  threadWriter.reset(id, { ...source, thread, items: [person, answer], inputs: [input], turns: [completed,
    ...(middle ? [turn('later', { ordinal: 2, state: 'completed', inputId: 'later-input' })] : [])],
    prompt_summaries: [{ ...summary(input.id), author: source.thread.owner, total: 1, edit: { available: true, reason: null, detail: null } }] }, false, captureThreadRead(id));
  wire.send.mockResolvedValue({ input, seq: source.seq + 1 }); wire.retry.mockResolvedValue({ input, seq: source.seq + 1 }); wire.copy.mockResolvedValue(true);
  wire.checkpoint.mockImplementation(() => new Promise<Snapshot>(() => undefined));
  const context: TimelineContext = { threadId: id, threadName: thread.name, meId: source.thread.owner.id, detail: 'steps', recent: new Set(),
    layout: 'one', setLayout: () => undefined, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><TimelineContextValue.Provider value={context}>
    <UserMessage item={person} /><TurnFooter turnId={completed.id} inputItemId={person.id} />
  </TimelineContextValue.Provider></div></ViewCompactContext.Provider>);
  return { ...view, id, completed, input };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps common controls in order and the restore on the request at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const view = draw(width);
    const person = view.container.querySelector<HTMLElement>('[data-hover-actions="person"]');
    const agent = view.container.querySelector<HTMLElement>('[data-hover-actions="turn"]');
    if (person === null || agent === null) throw new Error('Both messages retain their reserved action row');
    expect(within(person).getAllByRole('button').map((button) => button.getAttribute('aria-label'))).toEqual([
      i18n.t('messageVersions.edit'), i18n.t('timeline.actions.copy'), i18n.t('thread.turnActions.restoreBefore'), i18n.t('timeline.actions.more'),
    ]);
    expect(within(agent).getAllByRole('button').map((button) => button.getAttribute('aria-label'))).toEqual([
      i18n.t('thread.turnActions.copy'), i18n.t('timeline.footer.retry'), i18n.t('timeline.selection.forkNow'), i18n.t('thread.turnActions.more'),
    ]);
    expect(agent.querySelector('[data-restore-turn]')).toBeNull();
    expect(person).toHaveClass('min-h-(--control-sm)');
    if (width === 400) expect(person).toHaveAttribute('data-always-visible', 'true');
    else expect(person).toHaveClass('opacity-0', 'group-focus-within/item:opacity-100');
    await userEvent.click(within(agent).getByRole('button', { name: i18n.t('thread.turnActions.copy') }));
    expect(wire.copy).toHaveBeenCalledWith('Fix this route\n\nThe route is fixed.', i18n.t('thread.turnActions.copied'));
    await userEvent.click(within(person).getByRole('button', { name: i18n.t('thread.turnActions.restoreBefore') }));
    await waitFor(() => expect(wire.checkpoint).toHaveBeenCalledExactlyOnceWith('before-person-message'));
    expect(await screen.findByRole('dialog', { name: i18n.t('source.restore.titleTurn') })).toBeVisible();
    expect(wire.send).not.toHaveBeenCalled(); expect(wire.retry).not.toHaveBeenCalled();
  });
}

it('repeats the last completed input through the ordinary input and keeps Retry visible but blocked on a middle turn', async () => {
  const view = draw(400);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.selection.forkNow') }));
  expect(wire.fork).toHaveBeenCalledExactlyOnceWith(view.completed.ordinal);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.footer.retry') }));
  await waitFor(() => expect(wire.send).toHaveBeenCalledWith(expect.objectContaining({ threadId: view.id }), expect.objectContaining({ content: view.input.content, delivery: 'queue' })));
  expect(wire.retry).not.toHaveBeenCalled(); cleanup(); threadWriter.evict(view.id); wire.send.mockClear();
  draw(400, true);
  const retry = screen.getByRole('button', { name: i18n.t('timeline.footer.retry') });
  expect(retry).toHaveAttribute('aria-disabled', 'true');
  await userEvent.hover(retry); expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('thread.turnActions.retryMiddle'));
  await userEvent.click(retry); expect(wire.send).not.toHaveBeenCalled(); expect(wire.retry).not.toHaveBeenCalled();
});

it('explains an unavailable checkpoint and opens only that turn for per-file review', async () => {
  const view = draw(400, false, false);
  const review = screen.getByRole('button', { name: i18n.t('source.restore.review') });
  await userEvent.hover(review);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('thread.turnActions.noCheckpoint'));
  await userEvent.click(review);
  expect(wire.reveal).toHaveBeenCalledExactlyOnceWith('changes', { thread: view.id, scope: 'turn', turn: view.completed.id });
  expect(wire.checkpoint).not.toHaveBeenCalled(); expect(wire.send).not.toHaveBeenCalled();
});

it.each(['failed', 'interrupted'] as const)('retries a middle %s turn through its existing native operation', async (state) => {
  const view = draw(400, true, true, state);
  const retry = screen.getByRole('button', { name: i18n.t('timeline.footer.retry') });
  expect(retry).not.toHaveAttribute('aria-disabled');
  await userEvent.click(retry);
  await waitFor(() => expect(wire.retry).toHaveBeenCalledWith(expect.objectContaining({ threadId: view.id }), view.completed.id, expect.any(String)));
  expect(wire.send).not.toHaveBeenCalled();
});
