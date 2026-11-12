/** Reloaded goals open with their saved fields and update the existing goal. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { GoalEditorHost } from '@/features/goal/GoalEditor';
import { closeGoalEditor, openGoalEditor } from '@/features/goal/editor';
import type { ResolvedSetting, Thread, ThreadGoal } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const goals = vi.hoisted(() => ({ setGoal: vi.fn(), updateGoal: vi.fn() }));
const defaults = vi.hoisted(() => ({ read: vi.fn<() => Promise<ResolvedSetting[]>>() }));
vi.mock('@/api/goals', () => goals);
vi.mock('@/api/settings', () => ({ settingsQuery: () => ({ queryKey: ['goal-editor-settings'], queryFn: () => defaults.read() }) }));
vi.mock('@/features/goal/plan', () => ({ useGoalPlans: () => false }));

const BASE = {
  id: 'saved-goal', objective: 'Greet the team three times', condition: 'Three greetings are recorded',
  status: 'paused', limits: { max_turns: 30, max_minutes: null, max_credits: null }, notify_on_end: true,
} as unknown as ThreadGoal;

function threadOf(goal: ThreadGoal | null): Thread {
  return { id: 'thread-goal', goal } as unknown as Thread;
}

beforeEach(() => { vi.clearAllMocks(); defaults.read.mockReset(); defaults.read.mockResolvedValue([]); goals.updateGoal.mockResolvedValue(BASE); goals.setGoal.mockResolvedValue(BASE); });
afterEach(async () => { closeGoalEditor(); delete document.documentElement.dataset.theme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`does not call unread inherited goal limits unlimited at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    let finish: ((value: ResolvedSetting[]) => void) | undefined;
    defaults.read.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    openGoalEditor('thread-goal'); show(<GoalEditorHost thread={threadOf(null)} />);
    try {
      await waitFor(() => expect(defaults.read).toHaveBeenCalledOnce());
      expect(screen.queryByText(i18n.t('goals.editor.limitsNone'))).not.toBeInTheDocument();
      expect(screen.getByText(i18n.t('goals.editor.limitsChecking'))).toBeVisible();
      await userEvent.type(screen.getByLabelText(i18n.t('goals.editor.objective'), { exact: true }), 'Review the current work');
      await userEvent.type(screen.getByLabelText(i18n.t('goals.editor.condition'), { exact: true }), 'The requested review is recorded');
      expect(screen.getByRole('button', { name: i18n.t('goals.editor.set') })).toBeEnabled();
      expect(goals.setGoal).not.toHaveBeenCalled();
    } finally { finish?.([]); }
    expect(await screen.findByText(i18n.t('goals.editor.limitsInherited'))).toBeVisible();
  });
  it(`shows the failed defaults read and retries it without creating a goal at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    defaults.read.mockRejectedValueOnce(new Error('The defaults could not be read'));
    openGoalEditor('thread-goal'); show(<GoalEditorHost thread={threadOf(null)} />);
    await waitFor(() => expect(defaults.read).toHaveBeenCalledOnce());
    expect(screen.queryByText(i18n.t('goals.editor.limitsNone'))).not.toBeInTheDocument();
    expect(await screen.findByText(i18n.t('goals.editor.limitsUnavailable'))).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('goals.editor.limitsInherited'))).toBeVisible();
    expect(defaults.read).toHaveBeenCalledTimes(2); expect(goals.setGoal).not.toHaveBeenCalled();
  });
}

for (const status of ['active', 'paused', 'stopped'] as const) {
  it(`edits the saved ${status} goal rather than sending another create`, async () => {
    const saved = { ...BASE, status };
    openGoalEditor('thread-goal');
    show(<GoalEditorHost thread={threadOf(saved)} />);
    const dialog = screen.getByRole('dialog', { name: 'Edit goal' });
    expect(dialog).toBeVisible();
    expect(within(dialog).getByRole('button', { name: 'Help: Goals' })).toBeVisible();
    expect(screen.getByLabelText('Objective', { exact: true })).toHaveValue(saved.objective);
    expect(screen.getByLabelText('Done when', { exact: true })).toHaveValue(saved.condition);
    expect(screen.getByRole('checkbox', { name: 'Notify me when it ends' })).toBeChecked();
    await userEvent.clear(screen.getByLabelText('Done when', { exact: true }));
    await userEvent.type(screen.getByLabelText('Done when', { exact: true }), 'All three greetings are checked');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(goals.updateGoal).toHaveBeenCalledOnce());
    expect(goals.updateGoal.mock.calls[0]?.[1]).toEqual({
      condition: 'All three greetings are checked',
      limits: saved.limits,
      notify_on_end: true,
    });
    expect(goals.setGoal).not.toHaveBeenCalled();
  });
}

it('replaces an empty editor when the saved goal arrives', () => {
  openGoalEditor('thread-goal');
  const view = show(<GoalEditorHost thread={threadOf(null)} />);
  expect(screen.getByLabelText('Objective', { exact: true })).toHaveValue('');
  view.rerender(<GoalEditorHost thread={threadOf(BASE)} />);
  expect(screen.getByRole('dialog', { name: 'Edit goal' })).toBeVisible();
  expect(screen.getByLabelText('Objective', { exact: true })).toHaveValue(BASE.objective);
  expect(screen.getByLabelText('Done when', { exact: true })).toHaveValue(BASE.condition);
  expect(goals.setGoal).not.toHaveBeenCalled();
});
