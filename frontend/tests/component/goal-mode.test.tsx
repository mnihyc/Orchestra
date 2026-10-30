/** Goal mode: the goal line above the composer says where the goal stands in one row and offers Pause or Resume only to
 * whoever may drive the thread; a goal that stopped at a limit that still holds is not resumed until the limit is raised;
 * the slash command sets a goal from its words, or opens the editor, and is listed disabled where it cannot be used. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { chipOf, figures, reasonWords } from '@/features/goal/model';
import { GoalLine } from '@/features/goal/GoalLine';
import { PassiveStrip } from '@/features/thread/PassiveStrip';
import { useGoalCommand } from '@/features/goal/slash';
import type { Thread, ThreadGoal } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const goals = vi.hoisted(() => ({ setGoal: vi.fn(), updateGoal: vi.fn(), clearGoal: vi.fn() }));
vi.mock('@/api/goals', () => goals);
vi.mock('@/features/goal/plan', () => ({ useGoalPlans: () => false }));
const editor = vi.hoisted(() => ({ openGoalEditor: vi.fn() }));
vi.mock('@/features/goal/editor', () => editor);

const base = {
  id: 'goal-1', objective: 'Make the tests pass', condition: 'all tests pass', set_by: { display_name: 'Ada' }, set_at: '2026-10-28T10:00:00Z',
  status: 'active', reason: null, detail: null, waiting: null, checking: false, plan_first: false, notify_on_end: false,
  limits: { max_turns: null, max_minutes: null, max_credits: null },
  budgets: { max_turns: 30, max_minutes: 0, max_credits: '0', locked: false },
  used: { turns: 7, elapsed_secs: 600, running_since: null, tokens: 1_200_000, credits: '0' },
  helper: true, agent_reports: true, last_report: null, ended_at: null,
} as unknown as ThreadGoal;

function goalOf(patch: Partial<ThreadGoal>): ThreadGoal {
  return { ...base, ...patch };
}

function threadOf(goal: ThreadGoal | null, permissions = ['thread.view', 'thread.interact']): Thread {
  return { id: 'thread-goal', name: 'Fix the tests', archived: false, side_of: null, state: 'idle', goal,
    access: { permissions }, binding: { agent: { name: 'Scripted agent' }, plan_mode: false } } as unknown as Thread;
}

beforeEach(() => { vi.clearAllMocks(); goals.updateGoal.mockResolvedValue(base); goals.clearGoal.mockResolvedValue(base); goals.setGoal.mockResolvedValue(base); });
afterEach(async () => { await setLocale('en'); });

const STATES = [
  { patch: {}, chip: 'working' },
  { patch: { waiting: 'approval' }, chip: 'waiting' },
  { patch: { status: 'paused', reason: 'person' }, chip: 'paused' },
  { patch: { status: 'paused', reason: 'blocked', detail: 'The staging key is missing' }, chip: 'waiting' },
  { patch: { status: 'met', ended_at: '2026-10-28T11:00:00Z' }, chip: 'met' },
  { patch: { status: 'stopped', reason: 'turns' }, chip: 'stopped' },
] as const;

for (const locale of ['en', 'zh-CN'] as const) {
  it(`the line shows its state as a chip in words, with the condition and the figures (${locale})`, async () => {
    await setLocale(locale);
    for (const state of STATES) {
      const goal = goalOf(state.patch);
      const view = show(<GoalLine thread={threadOf(goal)} />);
      const line = within(screen.getByRole('region', { name: i18n.t('goals.line.label') }));
      expect(line.getByText(i18n.t(`goals.status.${state.chip}`))).toBeInTheDocument();
      expect(chipOf(goal)).toBe(state.chip);
      expect(screen.getAllByText(i18n.t('goals.line.condition', { condition: 'all tests pass' })).length).toBeGreaterThan(0);
      view.unmount();
    }
  });
}

it('says the turns of a limited goal and the reason a stopped one stopped', () => {
  const t = i18n.t.bind(i18n);
  expect(figures(base, t, 'en', 600_000).join(' · ')).toContain('7');
  expect(reasonWords(goalOf({ status: 'stopped', reason: 'turns' }), t, 'en')).toBe(i18n.t('goals.reason.turns', { limit: '30' }));
  expect(reasonWords(goalOf({ status: 'paused', reason: 'person' }), t, 'en')).toBe(i18n.t('goals.reason.person'));
});

it('pauses an active goal and resumes a paused one, for whoever may drive the thread', async () => {
  const first = show(<GoalLine thread={threadOf(base)} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('goals.act.pause') }));
  await waitFor(() => expect(goals.updateGoal).toHaveBeenCalledOnce());
  expect(goals.updateGoal.mock.calls[0]?.[1]).toEqual({ action: 'pause' });
  first.unmount();
  show(<GoalLine thread={threadOf(goalOf({ status: 'paused', reason: 'person' }))} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('goals.act.resume') }));
  await waitFor(() => expect(goals.updateGoal).toHaveBeenCalledTimes(2));
  expect(goals.updateGoal.mock.calls[1]?.[1]).toEqual({ action: 'resume' });
});

it('leaves the acts to people who may drive the thread, and says so', async () => {
  show(<GoalLine thread={threadOf(base, ['thread.view'])} />);
  const pause = screen.getByRole('button', { name: i18n.t('goals.act.pause') });
  expect(pause).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(pause);
  expect(goals.updateGoal).not.toHaveBeenCalled();
});

for (const status of ['paused', 'stopped'] as const) {
  it(`does not resume a ${status} goal whose limit still holds until it is raised`, async () => {
    show(<GoalLine thread={threadOf(goalOf({ status, reason: 'turns', used: { ...base.used, turns: 30 } }))} />);
    const resume = screen.getByRole('button', { name: i18n.t('goals.act.resume') });
    expect(resume).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(resume);
    expect(goals.updateGoal).not.toHaveBeenCalled();
  });

  it(`lets a ${status} goal resume after its reached limit is raised`, async () => {
    show(<GoalLine thread={threadOf(goalOf({ status, reason: 'turns', budgets: { ...base.budgets, max_turns: 31 }, used: { ...base.used, turns: 30 } }))} />);
    const resume = screen.getByRole('button', { name: i18n.t('goals.act.resume') });
    expect(resume).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(resume);
    await waitFor(() => expect(goals.updateGoal).toHaveBeenCalledOnce());
    expect(goals.updateGoal.mock.calls[0]?.[1]).toEqual({ action: 'resume' });
  });
}

it('offers Edit and Clear in its menu, and clears on Clear', async () => {
  show(<GoalLine thread={threadOf(base)} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('goals.act.more') }));
  expect(screen.getByRole('menuitem', { name: i18n.t('goals.act.edit') })).toBeInTheDocument();
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('goals.act.clear') }));
  await waitFor(() => expect(goals.clearGoal).toHaveBeenCalledOnce());
});

it('leaves the slash command disabled for readers who may not drive', () => {
  const { result: reader } = renderHookIn(threadOf(null, ['thread.view']));
  expect(reader.current[0]?.off).toBe(i18n.t('goals.act.driveOnly'));
  const { result: free } = renderHookIn(threadOf(null));
  expect(free.current[0]?.off).toBeNull();
  expect(free.current[0]?.name).toBe('goal');
  const { result: ended } = renderHookIn(threadOf(goalOf({ status: 'met' })));
  expect(ended.current[0]?.off).toBeNull();
});

for (const status of ['active', 'paused', 'stopped'] as const) {
  it(`opens the existing ${status} goal from /goal instead of creating another one`, () => {
    const { result } = renderHookIn(threadOf(goalOf({ status })));
    const command = result.current[0];
    expect(command?.off).toBeNull();
    expect(command?.run('')).toBeNull();
    expect(command?.run('a different condition')).toBeNull();
    expect(editor.openGoalEditor).toHaveBeenCalledTimes(2);
    expect(editor.openGoalEditor).toHaveBeenCalledWith('thread-goal');
    expect(goals.setGoal).not.toHaveBeenCalled();
  });
}

it('runs /goal with words as a goal of those words, and with none opens the editor', async () => {
  const { result } = renderHookIn(threadOf(null));
  const command = result.current[0];
  expect(command?.run('')).toBeNull();
  expect(editor.openGoalEditor).toHaveBeenCalledWith('thread-goal');
  expect(command?.run('the tests pass')).toBeNull();
  await waitFor(() => expect(goals.setGoal).toHaveBeenCalledOnce());
  expect(goals.setGoal.mock.calls[0]?.[1]).toEqual({ objective: 'the tests pass', condition: 'the tests pass', notify_on_end: false, plan: false });
});

function renderHookIn(thread: Thread) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return renderHook(() => useGoalCommand(thread), {
    wrapper: ({ children }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>,
  });
}

it('the passive row shows the most urgent state alone, counts the rest, and opens them on request', async () => {
  show(<PassiveStrip entries={[
    { id: 'blocked', node: <p>Blocked by a lock</p> },
    { id: 'goal', node: <p>Goal line</p> },
    { id: 'review', node: <p>Review waiting</p> },
  ]} />);
  const visible = () => ['Blocked by a lock', 'Goal line', 'Review waiting'].filter((text) => {
    const found = screen.getByText(text);
    return found.closest('[data-passive]')?.classList.contains('hidden') !== true;
  });
  await waitFor(() => expect(visible()).toEqual(['Blocked by a lock']));
  const more = screen.getByRole('button', { name: i18n.t('thread.passive.more', { count: 2 }) });
  expect(more).toHaveTextContent('+2');
  await userEvent.click(more);
  expect(visible()).toEqual(['Blocked by a lock', 'Goal line', 'Review waiting']);
  expect(screen.getByRole('dialog', { name: i18n.t('thread.passive.more', { count: 2 }) })).toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(visible()).toEqual(['Blocked by a lock']));
  await waitFor(() => expect(more).toHaveFocus());
});

it('the passive row takes no room while nothing in it has anything to say', async () => {
  show(<PassiveStrip entries={[{ id: 'goal', node: null }, { id: 'review', node: null }]} />);
  const strip = document.querySelector<HTMLElement>('[data-passive-strip]');
  if (strip === null) throw new Error('The passive strip is missing');
  await waitFor(() => expect(strip.classList.contains('hidden')).toBe(true));
});


it('offers the same goal acts from its context menu, including permission reasons', async () => {
  const view = show(<GoalLine thread={threadOf(base)} />);
  await userEvent.pointer({ keys: '[MouseRight]', target: screen.getByRole('region', { name: i18n.t('goals.line.label') }) });
  expect(screen.getByRole('menuitem', { name: i18n.t('goals.act.edit') })).toBeInTheDocument();
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('goals.act.clear') }));
  await waitFor(() => expect(goals.clearGoal).toHaveBeenCalledOnce());
  view.unmount();
  show(<GoalLine thread={threadOf(base, ['thread.view'])} />);
  await userEvent.pointer({ keys: '[MouseRight]', target: screen.getByRole('region', { name: i18n.t('goals.line.label') }) });
  expect(screen.getByRole('menuitem', { name: i18n.t('goals.act.clear') })).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('goals.act.clear') }));
  expect(goals.clearGoal).toHaveBeenCalledOnce();
});


it('closes the passive card when its remaining states disappear', async () => {
  const view = show(<PassiveStrip entries={[
    { id: 'blocked', node: <p>Blocked by a lock</p> },
    { id: 'goal', node: <p>Goal line</p> },
  ]} />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('thread.passive.more', { count: 1 }) }));
  expect(screen.getByRole('dialog')).toBeInTheDocument();
  view.rerender(<PassiveStrip entries={[{ id: 'blocked', node: <p>Blocked by a lock</p> }, { id: 'goal', node: null }]} />);
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByText('Blocked by a lock')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('thread.passive.more', { count: 1 }) })).not.toBeInTheDocument();
});
