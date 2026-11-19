/** The confirmation explains every wait or refusal beside Switch and rejects an old plan. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { ThreadChange } from '@/features/change/ThreadChange';
import type { ForkPlan, ForkRequest, PlanBlocker, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const state = vi.hoisted(() => ({ node: null as Thread | null, answer: null as ForkPlan | null, read: vi.fn<(request: ForkRequest) => Promise<ForkPlan>>(), start: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: () => ({ queryKey: ['switch-node'], initialData: state.node, queryFn: () => Promise.resolve(state.node) }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), providersQuery: () => ({ queryKey: ['switch-providers'], queryFn: () => Promise.resolve([]) }), agentsQuery: () => ({ queryKey: ['switch-agents'], queryFn: () => Promise.resolve([]) }) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), planFork: (_id: string, request: ForkRequest) => state.read(request), startFork: state.start }));
vi.mock('@/features/change/RunsOn', async (original) => ({ ...await original<object>(), useModelFor: () => [], RunsOn: ({ onChange }: { onChange: (patch: { providerId: string | null; agentId: string | null; model: string | null }) => void }) => <>
  <button onClick={() => onChange({ providerId: 'next', agentId: 'agent', model: 'shared-route' })}>Choose next provider</button>
  <button onClick={() => onChange({ providerId: null, agentId: null, model: null })}>Keep current provider</button>
</> }));
vi.mock('@/features/change/Where', () => ({ Where: () => null }));
vi.mock('@/features/change/plan', async (original) => ({ ...await original<object>(), changesOf: () => [], staysOf: () => [], endsOf: () => [], targetModelName: () => 'Shared model' }));
vi.mock('@/features/change/Time', () => ({ Time: () => null }));
vi.mock('@/features/change/Depends', () => ({ Depends: () => null }));

function answer(blockers: PlanBlocker[] = []): ForkPlan {
  return { result: 'replace', source: { name: 'Login' }, from: { provider: { id: 'current', name: 'Current' } },
    to: { provider: { id: 'next', name: 'Next' }, agent: { id: 'agent' }, model: 'shared-route', effort: null, environment: { id: 'environment', name: 'Workspace' } },
    expires_at: new Date(Date.now() + 60_000).toISOString(), preflight_id: 'review', locks: [], blockers, depends: [], turn_running: false,
    options: { active_turn: 'wait', wait_minutes: 10 }, history: { report: { fidelity: 'lossless', by_kind: {}, items_total: 0, dropped: [], warnings: [] } } } as unknown as ForkPlan;
}
beforeEach(() => {
  vi.clearAllMocks();
  state.node = { id: 'thread', kind: 'thread', name: 'Login', model: 'shared-route', model_display: 'Shared model', effort: null, state: 'idle', side_of: null, blocked: null, path: [], owner: { id: 'owner' }, binding: { provider: { id: 'current' }, agent: { id: 'agent' } } } as unknown as Thread;
  state.answer = answer();
  state.read.mockImplementation(() => Promise.resolve(state.answer!));
});
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names a required background count on the disabled confirmation (${locale})`, async () => {
    await setLocale(locale);
    state.answer = answer([{ code: 'measurement_pending', message: 'Waiting for the source count.' }]);
    show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus={null} job={null} say={null} open onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole('button', { name: 'Choose next provider' }));
    const button = await screen.findByRole('button', { name: i18n.t('liveWork.waitCount') });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(button);
    expect(state.start).not.toHaveBeenCalled();
  });

  it(`says why nothing can be confirmed, then ignores a valid old plan when the choice is reset (${locale})`, async () => {
    await setLocale(locale);
    show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus={null} job={null} say={null} open onClose={() => undefined} />);
    const button = await screen.findByRole('button', { name: i18n.t('change.confirm.switch') });
    expect(button).toHaveAttribute('aria-disabled', 'true');
    expect(button).toHaveAccessibleDescription(i18n.t('changeReady.unchanged'));
    expect(screen.getByText(i18n.t('changeReady.unchanged'))).toBeVisible();
    expect(state.read).not.toHaveBeenCalled();
    expect(screen.queryByRole('list', { name: i18n.t('change.steps.label') })).toBeNull();
    expect(screen.queryByRole('button', { name: i18n.t('change.options.button') })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Choose next provider' }));
    await waitFor(() => expect(button).not.toHaveAttribute('aria-disabled', 'true'));
    expect(state.read).toHaveBeenCalledOnce();
    expect(screen.queryByText(i18n.t('change.lists.changes'))).toBeNull();
    expect(screen.queryByText(i18n.t('change.lists.stays'))).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Keep current provider' }));
    expect(button).toHaveAccessibleDescription(i18n.t('changeReady.unchanged'));
    expect(button).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(button);
    expect(state.start).not.toHaveBeenCalled();
  });

  for (const phase of ['checking', 'failed', 'expired', 'access'] as const) it(`names ${phase} beside the disabled action (${locale})`, async () => {
    await setLocale(locale);
    let finish: (() => void) | undefined;
    if (phase === 'checking') state.read.mockImplementation(() => new Promise<ForkPlan>((resolve) => { finish = () => resolve(answer()); }));
    if (phase === 'failed') state.read.mockRejectedValue(new Error('Unavailable'));
    if (phase === 'expired') state.answer = { ...answer(), expires_at: '2000-01-01T00:00:00Z' };
    if (phase === 'access') state.answer = answer([{ code: 'access', message: 'Permission required' }]);
    const view = show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus={null} job={null} say={null} open onClose={() => undefined} />);
    try {
      await userEvent.click(await screen.findByRole('button', { name: 'Choose next provider' }));
      await waitFor(() => expect(state.read).toHaveBeenCalledOnce());
      const button = screen.getByRole('button', { name: i18n.t('change.confirm.switch') });
      await waitFor(() => expect(button).toHaveAccessibleDescription(i18n.t(`changeReady.${phase}`)));
      expect(screen.getByText(i18n.t(`changeReady.${phase}`))).toBeVisible();
      expect(button).toHaveAttribute('aria-disabled', 'true');
      await userEvent.click(button);
      expect(state.start).not.toHaveBeenCalled();
    } finally { view.unmount(); finish?.(); }
  });
}

it('confirms the selected native preflight once without another wizard step or prompt', async () => {
  let finish: () => void = () => undefined;
  const pending = new Promise<void>((resolve) => { finish = resolve; });
  state.start.mockImplementation(async () => { await pending; return { id: 'job' }; });
  const view = show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" job={null} say={null} open onClose={() => undefined} />);
  try {
    await userEvent.click(await screen.findByRole('button', { name: 'Choose next provider' }));
    const apply = screen.getByRole('button', { name: i18n.t('change.confirm.switch') });
    await waitFor(() => expect(apply).not.toHaveAttribute('aria-disabled', 'true'));
    expect(state.start).not.toHaveBeenCalled();
    await userEvent.click(apply);
    expect(state.start).toHaveBeenCalledOnce();
    expect(state.start).toHaveBeenCalledWith('thread', { preflight_id: 'review' }, expect.any(String));
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(screen.queryByRole('list', { name: i18n.t('change.steps.label') })).toBeNull();
  } finally { view.unmount(); finish(); }
});
