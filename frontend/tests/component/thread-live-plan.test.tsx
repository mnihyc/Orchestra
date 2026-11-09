/** The current plan ticks in place; its slim line follows actual pending work, completion and the native pointer. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { PlanLine } from '@/features/timeline/PlanLine';
import { Plan } from '@/features/timeline/items/Plan';
import type { Item, PlanStep } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { plan, turn } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';
vi.mock('@/features/timeline/ItemActions', () => ({ ItemFrame: ({ children }: { children: ReactNode }) => children }));
const THREAD = 'live-plan';
const jump = vi.fn();
function item(steps: PlanStep[], seq: number): Extract<Item, { kind: 'plan' }> {
  const row = plan('plan-one', steps, { turn: 'current-turn' });
  if (row.kind !== 'plan') throw new Error('The fixture is a native plan');
  return { ...row, thread_id: THREAD, seq_created: 5, seq_updated: seq };
}
function seed(row: Item, current = 'current-turn') {
  const native = snapshot(null);
  threadWriter.reset(THREAD, { ...native, thread: { ...native.thread, id: THREAD, state: 'running', current_turn_id: current }, items: [row],
    turns: [{ ...turn(current, { ordinal: 1, state: 'running', inputId: 'input' }), thread_id: THREAD, seq_updated: 5 }] }, false);
}
function Harness() {
  const row = useThreadPart(THREAD, (view) => view.items.get('plan-one'));
  return <>{row?.kind === 'plan' && <Plan item={row} />}<PlanLine threadId={THREAD} onJump={jump} /></>;
}
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); jump.mockReset(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps one native card and follows the actual pending step and completion in %s', async (locale) => {
  await setLocale(locale);
  seed(item([{ text: 'Review first', status: 'pending' }, { text: 'Already checked', status: 'completed' }], 10));
  const view = show(<Harness />);
  const card = view.container.querySelector('[data-plan="plan-one"]');
  const line = screen.getByRole('button', { name: `${i18n.t('timeline.planLine.step', { index: '1', total: '2' })}: Review first` });
  await userEvent.click(line); expect(jump).toHaveBeenCalledWith('plan-one');
  act(() => threadWriter.apply(THREAD, { items: [item([{ text: 'Review first', status: 'completed' }, { text: 'Already checked', status: 'completed' }], 11)] }));
  expect(view.container.querySelector('[data-plan="plan-one"]')).toBe(card);
  expect(screen.getByRole('button', { name: i18n.t('timeline.planLine.done', { count: 2, number: '2' }) })).toBeVisible();
  const first = within(screen.getByRole('region', { name: i18n.t('timeline.plan.title') })).getByText('Review first').closest('li');
  if (first === null) throw new Error('A plan step is a list row');
  expect(first.querySelector('.lucide-check')).toBeInTheDocument();
  expect(within(first).getByText(i18n.t('timeline.plan.status.completed'))).toBeInTheDocument();
  const native = threadViewOf(THREAD).thread;
  if (native === null) throw new Error('The plan retains its native thread');
  act(() => threadWriter.apply(THREAD, { thread: { ...native, seq: 20, current_turn_id: null, state: 'idle' } }));
  expect(screen.queryByRole('button', { name: i18n.t('timeline.planLine.done', { count: 2, number: '2' }) })).toBeNull();
  expect(view.container.querySelector('[data-plan="plan-one"]')).toBe(card);
});

it('does not revive a plan from an earlier turn while another turn runs', () => {
  seed(item([{ text: 'Old step', status: 'in_progress' }], 10), 'new-current-turn');
  show(<PlanLine threadId={THREAD} onJump={jump} />);
  expect(screen.queryByRole('button')).toBeNull();
});
