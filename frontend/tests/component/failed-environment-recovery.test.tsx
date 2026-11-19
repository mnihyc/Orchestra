/** P1: a failed first setup is actionable, keeps its real queued input and fences late retry replies. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as EnvironmentsApi from '@/api/environments';
import type * as SourceApi from '@/api/source';
import type * as ThreadsApi from '@/api/threads';
import { QueueTray } from '@/features/composer/QueueTray';
import { SourcePreparation } from '@/features/source/Preparation';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { NOT_PICKING, TimelineContextValue } from '@/features/timeline/context';
import { queuedInputs } from '@/features/timeline/rows';
import { StatusLine } from '@/features/timeline/StatusLine';
import type { Job, Thread, ThreadInput } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { threadViewOf, threadWriter, useThreadView } from '@/realtime/stores/thread';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ start: vi.fn<typeof EnvironmentsApi.startEnvironment>(), send: vi.fn<typeof ThreadsApi.sendInput>(), prepare: vi.fn<typeof SourceApi.prepareSource>(), change: vi.fn(), proceed: vi.fn() }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof EnvironmentsApi>(), startEnvironment: calls.start }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), sendInput: calls.send }));
vi.mock('@/api/source', async (load) => ({ ...await load<typeof SourceApi>(), prepareSource: calls.prepare }));
vi.mock('@/features/change/target', async (load) => ({ ...await load<object>(), openChange: calls.change }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const priorAppearance = useAppearance.getState();
let shownId: string | null = null;
beforeEach(() => { vi.clearAllMocks(); calls.start.mockReset(); });
afterEach(async () => { cleanup(); if (shownId !== null) threadWriter.evict(shownId); shownId = null; client.session.setState({ userId: null }); useAppearance.setState(priorAppearance); await setLocale('en'); });

function draw(thread: Thread, input: ThreadInput) {
  shownId = thread.id; client.session.setState({ userId: thread.owner.id });
  threadWriter.reset(thread.id, { thread, inputs: [input] }, false);
  function Surface() {
    const view = useThreadView(thread.id);
    const current = view.thread ?? thread;
    return <><TimelineContextValue.Provider value={{ threadId: current.id, threadName: current.name, detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined, meId: current.owner.id, canInteract: current.access.permissions.includes('thread.interact'), canManage: current.access.permissions.includes('thread.manage'), live: true, lit: null, selection: NOT_PICKING }}><StatusLine /></TimelineContextValue.Provider>
      <BlockedBanner thread={current} onProceed={calls.proceed} /><SourcePreparation thread={current} /><QueueTray threadId={thread.id} queued={queuedInputs(view)} sending={[]} /></>;
  }
  return show(<Surface />);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`explains the failed file setup and keeps the same first input through Retry and another location (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const { thread, input, job } = failedStart(); calls.start.mockResolvedValue(job);
    const user = userEvent.setup(); draw(thread, input);
    expect(screen.getByText(`${i18n.t('startRecovery.filesFailed', { environment: 'Main' })} ${i18n.t('startRecovery.held')}`)).toBeVisible();
    expect(screen.getByText('hello from scenario 39', { exact: true })).toBeVisible();
    expect(screen.queryByText(i18n.t('source.prepare.pending'))).not.toBeInTheDocument();
    expect(screen.queryByText(/file job's environment changed/)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('startRecovery.retry') }));
    await waitFor(() => expect(calls.start).toHaveBeenCalledExactlyOnceWith('failed-environment'));
    expect(queuedInputs(threadViewOf(thread.id))).toEqual([input]);
    await user.click(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
    expect(calls.change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'environment' });
    expect(queuedInputs(threadViewOf(thread.id))).toEqual([input]);
    expect(calls.send).not.toHaveBeenCalled(); expect(calls.prepare).not.toHaveBeenCalled(); expect(calls.proceed).not.toHaveBeenCalled();
    act(() => threadWriter.apply(thread.id, { thread: { ...thread, seq: thread.seq + 1, state: 'starting', blocked: null } }));
    expect(screen.getByText(i18n.t('thread.starting.environment'))).toBeVisible();
    expect(screen.queryByText(`${i18n.t('startRecovery.filesFailed', { environment: 'Main' })} ${i18n.t('startRecovery.held')}`)).not.toBeInTheDocument();
    expect(queuedInputs(threadViewOf(thread.id))).toEqual([input]);
  });
}

it('keeps the alternative available during a slow retry and ignores an old refusal after a native environment switch', async () => {
  const { thread, input, job } = failedStart();
  const gate: { reject?: (error: unknown) => void } = {};
  const pending = new Promise<Job>((_resolve, reject) => { gate.reject = reject; });
  calls.start.mockReturnValue(pending);
  const user = userEvent.setup(); draw(thread, input);
  try {
    await user.click(screen.getByRole('button', { name: i18n.t('startRecovery.retry') }));
    expect(screen.getByText(`${i18n.t('startRecovery.retrying', { environment: 'Main' })} ${i18n.t('startRecovery.held')}`)).toBeVisible();
    const elsewhere = screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') });
    expect(elsewhere).toBeEnabled(); expect(elsewhere).not.toHaveAttribute('aria-disabled', 'true'); await user.click(elsewhere);
    const environment = thread.environment;
    if (environment === null) throw new Error('the failed start has its environment');
    act(() => threadWriter.apply(thread.id, { thread: { ...thread, seq: thread.seq + 1, environment: { ...environment, id: 'replacement', name: 'Replacement' } } }));
    expect(screen.getByText(`${i18n.t('startRecovery.filesFailed', { environment: 'Replacement' })} ${i18n.t('startRecovery.held')}`)).toBeVisible();
    await act(async () => { gate.reject?.(new Error('Old environment refused the retry')); await pending.catch(() => undefined); });
    expect(screen.getAllByRole('alert')).toHaveLength(1);
    expect(screen.getByRole('button', { name: i18n.t('startRecovery.retry') })).toBeEnabled();
    expect(screen.getByRole('button', { name: i18n.t('startRecovery.retry') })).not.toHaveAttribute('aria-disabled', 'true');
    expect(queuedInputs(threadViewOf(thread.id))).toEqual([input]);
    calls.start.mockResolvedValue({ ...job, environment_id: 'replacement' });
    await user.click(screen.getByRole('button', { name: i18n.t('startRecovery.retry') }));
    await waitFor(() => expect(calls.start).toHaveBeenLastCalledWith('replacement'));
    expect(calls.send).not.toHaveBeenCalled();
  } finally { gate.reject?.(new Error('test finished')); await pending.catch(() => undefined); }
});

it('uses current environment and thread authority independently and never submits an empty environment id', async () => {
  const { thread, input } = failedStart();
  const user = userEvent.setup(); draw({ ...thread, environment: null, environment_permissions: [] }, input);
  expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
  expect(calls.change).toHaveBeenCalledOnce(); expect(calls.start).not.toHaveBeenCalled();
  act(() => threadWriter.apply(thread.id, { thread: { ...thread, seq: thread.seq + 1, environment_permissions: [], access: { ...thread.access, permissions: ['thread.view'] } } }));
  expect(screen.queryByRole('button', { name: i18n.t('startRecovery.retry') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('startRecovery.elsewhere') })).not.toBeInTheDocument();
  expect(queuedInputs(threadViewOf(thread.id))).toEqual([input]);
});
