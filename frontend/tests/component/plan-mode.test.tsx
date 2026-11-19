/** Plan mode: the composer's Plan switch sits beside the approval mode and only where the agent program can plan; a
 * proposed plan is answered with Proceed, or with Keep planning and the person's words. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ApprovalsApi from '@/api/approvals';
import { ApiError } from '@/api/client';
import { useDecision } from '@/features/approvals/Decision';
import { DecisionError } from '@/features/approvals/DecisionError';
import { PlanAnswer, planningAfter } from '@/features/approvals/PlanAnswer';
import { planDecision } from '@/features/approvals/Harness';
import { ComposerControls } from '@/features/composer/Controls';
import { PlanSwitch } from '@/features/composer/PlanSwitch';
import { useComposerStore } from '@/features/composer/Draft';
import type { Approval, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const answer = vi.hoisted(() => vi.fn<() => Promise<Approval>>());
vi.mock('@/api/approvals', async (original) => ({
  ...await original<typeof ApprovalsApi>(), decideApproval: answer,
}));

vi.mock('@/features/change/target', () => ({ openChange: vi.fn() }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: () => <button>Model and effort</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: () => <button>Approval mode</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context breakdown</button> }));

const thread = { id: 'thread-plan', name: 'Plan the fix', archived: false, state: 'idle', side_of: null, lock: null,
  owner: { display_name: 'Ada' }, binding: { agent: { name: 'Scripted agent' }, plan_mode: true },
  access: { permissions: ['thread.view', 'thread.manage', 'thread.interact'] },
} as unknown as Thread;

beforeEach(() => useComposerStore.setState({ planning: {} }));
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`Plan stands beside the approval mode and says what it does on hover, never changing the mode (${locale})`, async () => {
    await setLocale(locale);
    const changed = vi.fn();
    show(<ComposerControls thread={thread} attachment={<button>Add to message</button>} delivery={<button>Send</button>}
      plan={<PlanSwitch on={false} onChange={changed} />} />);
    const controls = within(screen.getByRole('group', { name: i18n.t('composer.row.label') }));
    const names = controls.getAllByRole('button').map((button) => button.textContent);
    expect(names.indexOf(i18n.t('composer.plan.label'))).toBe(names.indexOf('Approval mode') + 1);
    const plan = controls.getByRole('button', { name: i18n.t('composer.plan.label') });
    expect(plan).toHaveAttribute('aria-pressed', 'false');
    await userEvent.click(plan);
    expect(changed).toHaveBeenCalledExactlyOnceWith(true);
  });
}

it('proceeds at once, and keeps planning only with words, which it sends', async () => {
  const proceed = vi.fn();
  const keep = vi.fn();
  show(<PlanAnswer disabled={false} pending={null} onProceed={proceed} onKeepPlanning={keep} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('plan.keepPlanning') }));
  const words = screen.getByRole('textbox', { name: i18n.t('plan.words') });
  expect(screen.getByRole('button', { name: i18n.t('plan.keepPlanning') })).toBeDisabled();
  await userEvent.type(words, '  Split the second step  ');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('plan.keepPlanning') }));
  expect(keep).toHaveBeenCalledExactlyOnceWith('Split the second step');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('plan.proceed') }));
  expect(proceed).toHaveBeenCalledOnce();
});

it('answers a plan request with the options the program offered, and keeps the Plan switch in step', () => {
  expect(planDecision(true)).toEqual({ option_id: 'proceed' });
  expect(planDecision(false, 'Cover logout too')).toEqual({ option_id: 'keep_planning', note: 'Cover logout too' });
  planningAfter({ thread_id: 'thread-plan' }, false);
  expect(useComposerStore.getState().planning['thread-plan']).toBe(false);
  planningAfter({ thread_id: 'thread-plan' }, true);
  expect(useComposerStore.getState().planning['thread-plan']).toBe(true);
});

it('cannot be answered by somebody who may not', () => {
  const proceed = vi.fn();
  show(<PlanAnswer disabled pending={null} onProceed={proceed} onKeepPlanning={vi.fn()} />);
  expect(screen.getByRole('button', { name: i18n.t('plan.proceed') })).toBeDisabled();
  expect(screen.getByRole('button', { name: i18n.t('plan.keepPlanning') })).toBeDisabled();
});

for (const locale of ['en', 'zh-CN'] as const) {
  for (const [reason, key] of [
    ['that decision does not fit what the harness asked', 'approvals.answerMismatch'],
    ['the request offers no option "proceed"', 'approvals.answerOptionGone'],
    ['The plan has been withdrawn by the agent.', null],
  ] as const) {
    it(`shows a plan refusal reason directly beside the answer (${locale}, ${reason})`, async () => {
      await setLocale(locale);
      const error = new ApiError({ status: 400, code: 'request.invalid', request_id: 'plan-refusal',
        errors: [{ path: '', message: reason }], title: 'Invalid request', type: 'about:blank', detail: 'Answer refused' }, null);
      show(<DecisionError error={error} />);
      expect(screen.getByRole('alert')).toHaveTextContent(key === null ? reason : i18n.t(key));
    });
  }
}

function AnswerProbe({ proceed }: { proceed: boolean }) {
  const decision = useDecision('plan-card');
  return <><button onClick={() => decision.decide(planDecision(proceed, 'Revise the rollback'))}>Confirm</button>
    <output data-testid="pending">{String(decision.pending)}</output>
    {decision.error !== null && <DecisionError error={decision.error} />}</>;
}

for (const proceed of [true, false]) {
  it(`changes the Plan switch only after the plan decision is confirmed (${proceed})`, async () => {
    let confirm!: (approval: Approval) => void;
    answer.mockImplementationOnce(() => new Promise<Approval>((resolve) => { confirm = resolve; }));
    useComposerStore.setState({ planning: { 'thread-plan': proceed } });
    show(<AnswerProbe proceed={proceed} />);
    await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
    await waitFor(() => expect(screen.getByTestId('pending')).toHaveTextContent('true'));
    expect(useComposerStore.getState().planning['thread-plan']).toBe(proceed);
    confirm({ id: 'plan-card', kind: 'harness', thread_id: 'thread-plan',
      state: proceed ? 'approved' : 'denied', request: { request_kind: 'plan', detail: { plan: 'Read, then change' }, options: [] },
    } as unknown as Approval);
    await waitFor(() => expect(screen.getByTestId('pending')).toHaveTextContent('false'));
    expect(useComposerStore.getState().planning['thread-plan']).toBe(!proceed);
  });
}

it('keeps the Plan switch unchanged after a refused answer and displays the reason', async () => {
  const error = new ApiError({ status: 400, code: 'request.invalid', request_id: 'plan-refusal',
    errors: [{ path: '', message: 'that decision does not fit what the harness asked' }],
    title: 'Invalid request', type: 'about:blank', detail: 'Answer refused' }, null);
  answer.mockRejectedValueOnce(error);
  useComposerStore.setState({ planning: { 'thread-plan': true } });
  show(<AnswerProbe proceed />);
  await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('approvals.answerMismatch'));
  expect(useComposerStore.getState().planning['thread-plan']).toBe(true);
});
