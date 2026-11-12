/** A known goal command stays visible when this thread cannot hold a goal. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as GoalsApi from '@/api/goals';
import { useSlashMenu } from '@/features/composer/SlashMenu';
import { useGoalCommand } from '@/features/goal/slash';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ set: vi.fn<typeof GoalsApi.setGoal>(), edit: vi.fn<(id: string) => void>() }));
vi.mock('@/api/goals', async (original) => ({ ...await original<typeof GoalsApi>(), setGoal: calls.set }));
vi.mock('@/features/goal/editor', () => ({ openGoalEditor: calls.edit }));
vi.mock('@/features/goal/plan', () => ({ useGoalPlans: () => false }));
const appearance = useAppearance.getState();
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

function Command({ thread }: { thread: Thread }) {
  const [text, setText] = useState('/goal');
  const commands = useGoalCommand(thread);
  const slash = useSlashMenu(text, commands, () => setText(''));
  return <div><textarea aria-label="Command" value={text} onChange={(event) => setText(event.target.value)}
    {...slash.boxProps} onKeyDown={(event) => { slash.onKeyDown(event); }} />{slash.list}{slash.problem}</div>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  for (const state of ['archived', 'side'] as const) it(`keeps /goal disabled with the ${state} reason in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    const native = snapshot().thread;
    const thread = { ...native, archived: state === 'archived', side_of: state === 'side' ? { id: 'main', kind: 'thread' as const, name: 'Main thread', redacted: false } : null };
    show(<Command thread={thread} />);
    const reason = i18n.t(state === 'archived' ? 'goals.act.archived' : 'goals.act.sideThread');
    const option = screen.getByRole('option');
    expect(option).toHaveTextContent('/goal'); expect(option).toHaveAttribute('aria-disabled', 'true');
    expect(option).toHaveTextContent(reason);
    await userEvent.click(option);
    const box = screen.getByRole('textbox', { name: 'Command' });
    await userEvent.click(box); await userEvent.keyboard('{Enter}');
    expect(screen.getByRole('alert')).toHaveTextContent(reason); expect(box).toHaveValue('/goal');
    expect(calls.set).not.toHaveBeenCalled(); expect(calls.edit).not.toHaveBeenCalled();
  });

  it(`keeps the current Interact reason and the usable main-thread editor in ${locale} / ${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme });
    const native = snapshot().thread;
    const view = show(<Command thread={{ ...native, archived: false, side_of: null, access: { ...native.access, permissions: ['thread.view'] } }} />);
    expect(screen.getByRole('option')).toHaveTextContent(i18n.t('goals.act.driveOnly'));
    view.rerender(<Command thread={{ ...native, archived: false, side_of: null, goal: null, access: { ...native.access, permissions: ['thread.view', 'thread.interact'] } }} />);
    expect(screen.getByRole('option')).not.toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(screen.getByRole('option'));
    expect(calls.edit).toHaveBeenCalledExactlyOnceWith(native.id); expect(calls.set).not.toHaveBeenCalled();
  });
}

// Browser path: thread → type /goal → inspect the disabled archived/side reason → Enter leaves it unchanged.
// 浏览器路径：打开线程 → 输入 /goal → 查看归档或旁路线程的禁用原因 → 回车后不创建目标。
// The bilingual width/theme walk is in implicit-actions.spec.ts.
