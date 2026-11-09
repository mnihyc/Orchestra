/** The Now line observes actual admitted rows; the native Stop and approval cache own authority. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Threads from '@/api/threads';
import { approvalKey } from '@/api/approvals';
import { activityWords, NowLine, ThreadNowProvider } from '@/features/thread/NowLine';
import { StateFact } from '@/features/thread/Header';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { useAppearance } from '@/lib/appearance';
import type { Item } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { ALLOW_DENY, approvalItem, command, harnessApproval, reasoning, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

const wire = vi.hoisted(() => ({ interrupt: vi.fn<typeof Threads.interruptTurn>() }));
const ADA = userRef(me(1, 'ada', 'Ada'));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), interruptTurn: wire.interrupt }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useSocketStatus: () => ({ phase: 'ready' }) }));
const ids: string[] = [];
const appearance = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
afterEach(async () => {
  cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); vi.clearAllMocks(); await setLocale('en');
  useAppearance.setState(appearance);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
});

it.each(['en', 'zh-CN'] as const)('names the current turn, advances its work and stops through the native action in %s', async (locale) => {
  await setLocale(locale);
  const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const current = { ...turn('now-turn', { state: 'running', ordinal: 2, inputId: 'input', startedAt: new Date().toISOString() }), thread_id: id };
  const stale = command('old-running', { turn: 'old', command: 'Old command' });
  const item = command('current-command', { turn: current.id, command: 'ls', at: new Date(Date.now() - 3000).toISOString() });
  threadWriter.reset(id, { ...source, thread: { ...source.thread, state: 'running', current_turn_id: current.id }, turns: [current], items: [stale, item] }, false);
  wire.interrupt.mockResolvedValue({ ...current, seq_updated: source.seq + 1, state: 'interrupted', completed_at: new Date().toISOString() });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const view = show(<QueryClientProvider client={cache}><NowLine threadId={id} /></QueryClientProvider>);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.now.running', { command: 'ls' }));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.viewModes.toolCalls', { count: 1 }));
  expect(screen.queryByText('Old command')).not.toBeInTheDocument();
  expect(screen.getByRole('status').textContent).toMatch(/3/);
  act(() => threadWriter.apply(id, { items: [{ ...item, seq_updated: source.seq + 1, status: 'completed' }, reasoning('thinking', 'Private thought', { turn: current.id, status: 'in_progress', at: new Date().toISOString() })] }));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.now.thinking'));
  expect(screen.queryByText('Private thought')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
  await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
  await waitFor(() => expect(view.container.querySelector('[data-thread-now]')).toBeNull());
  cache.clear();
});

it.each(['en', 'zh-CN'] as const)('prioritizes a pending native request and retires its confirmed answer without waiting for the item in %s', async (locale) => {
  await setLocale(locale); const source = snapshot(null); const id = source.thread.id; ids.push(id);
  const current = turn('approval-turn', { state: 'running', ordinal: 1, inputId: 'input' });
  const approval = harnessApproval('now-request', { threadId: id, itemId: 'ask', requester: ADA, title: 'Run cleanup', request: { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'clean output', cwd: '/work', reason: null } } });
  const item: Item = { ...approvalItem('ask', approval.id, { turn: current.id }), kind: 'approval', payload: { approval_id: approval.id, approval } };
  threadWriter.reset(id, { ...source, thread: { ...source.thread, state: 'waiting_approval', current_turn_id: current.id }, turns: [current], items: [item] }, false);
  const cache = new QueryClient(); const view = show(<QueryClientProvider client={cache}><NowLine threadId={id} /></QueryClientProvider>);
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.now.approvalFor', { target: 'clean output' }));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.now.approvals', { count: 1 }));
  await act(() => { cache.setQueryData(approvalKey(approval.id), { ...approval, state: 'approved' }); });
  await waitFor(() => expect(screen.getByRole('status')).not.toHaveTextContent('clean output'));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('thread.now.approvals', { count: 0 }));
  act(() => threadWriter.apply(id, { thread: { ...source.thread, seq: source.seq + 1, state: 'idle', current_turn_id: null } }));
  expect(view.container.querySelector('[data-thread-now]')).toBeNull();
  cache.clear();
});

it('does not invent a timer origin for an unknown step', () => {
  const current = turn('starting', { state: 'starting', ordinal: 1, inputId: 'input' });
  current.started_at = null;
  expect(activityWords(undefined, current, i18n.t)).toEqual({ text: i18n.t('thread.now.starting'), since: null });
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps one Now/Stop owner in the state chip on a phone and the line on desktop at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const source = snapshot(null); const id = source.thread.id; ids.push(id);
    const current = turn('compact-now', { ordinal: 1, state: 'running', inputId: 'input', startedAt: new Date().toISOString() });
    const thread = { ...source.thread, display: executionDisplay('running'), state: 'running' as const, current_turn_id: current.id };
    threadWriter.reset(id, { ...source, thread, turns: [current], items: [command('compact-command', { turn: current.id, command: 'ls', at: new Date().toISOString() })] }, false);
    wire.interrupt.mockResolvedValue({ ...current, state: 'interrupted', seq_updated: source.seq + 1 });
    const view = show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><ThreadNowProvider threadId={id}><StateFact thread={thread} compact /><NowLine threadId={id} /></ThreadNowProvider></div></ViewCompactContext.Provider>);
    const line = view.container.querySelector('[data-thread-now]');
    const chip = view.container.querySelector('[data-thread-now-chip]');
    expect(width === 400 ? chip : line).toHaveTextContent(i18n.t('thread.now.running', { command: 'ls' }));
    expect(width === 400 ? line : chip).toBeNull();
    expect(screen.getAllByRole('button', { name: i18n.t('composer.stop') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('composer.stop') }));
    await waitFor(() => expect(wire.interrupt).toHaveBeenCalledExactlyOnceWith(defaultThreadRead(id), current.id));
  });
}
