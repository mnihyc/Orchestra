/** A refusal of Switch, and the wizard it was made in, survive the page reading the thread again: after a reconnect every read of a person's
 * reach starts over (src/api/peopleChanges.ts resets each active one to no data and asks again), and the wizard must neither give way to its
 * loading view, losing its choices, its plan and the refusal, nor let its first picker open as it comes back. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { ThreadChange } from '@/features/change/ThreadChange';
import type { ErrorCode, ForkPlan, ForkRequest, PlanBlocker, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { Dialog } from '@/ui/Dialog';
import { show } from './support';

const state = vi.hoisted(() => ({ node: null as Thread | null, readNode: null as unknown as () => Promise<Thread | null>, answer: null as ForkPlan | null, read: vi.fn<(request: ForkRequest) => Promise<ForkPlan>>(), start: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: () => ({ queryKey: ['switch-node'], queryFn: () => state.readNode() }) }));
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
    expires_at: new Date(Date.now() + 60_000).toISOString(), preflight_id: 'review', locks: [], blockers, depends: [], turn_running: false, mounts: [], mcp: [],
    options: { active_turn: 'wait', wait_minutes: 10 }, history: { report: { fidelity: 'lossless', by_kind: {}, items_total: 0, dropped: [], warnings: [] } } } as unknown as ForkPlan;
}
beforeEach(() => {
  vi.clearAllMocks();
  state.node = { id: 'thread', kind: 'thread', name: 'Login', model: 'shared-route', model_display: 'Shared model', effort: null, state: 'idle', side_of: null, blocked: null, path: [], owner: { id: 'owner' }, binding: { provider: { id: 'current' }, agent: { id: 'agent' } } } as unknown as Thread;
  state.readNode = () => Promise.resolve(state.node);
  state.answer = answer();
  state.read.mockImplementation(() => Promise.resolve(state.answer!));
});
afterEach(async () => { await setLocale('en'); });

const problem = (status: number, code: ErrorCode, detail: string) => new ApiError({ type: `orch:error:${code}`, status, code, title: detail, detail, request_id: `refused-${code}` }, null);
const refusals = {
  'an old plan': { error: problem(409, 'preflight.expired', 'The plan has expired.'), told: () => screen.getByText(i18n.t('change.expired.made')) },
  'a refused person': { error: problem(403, 'authz.denied', 'You may not switch this thread.'), told: () => screen.getByRole('alert') },
  'a failed network': { error: new NetworkError('offline'), told: () => screen.getByRole('alert') },
} as const;

/** What the authority repair does to a read of a person's reach that is on screen: no data, and asked again. The answer comes when `arrive` is called. */
async function readAgain(client: QueryClient): Promise<() => Promise<void>> {
  const query = client.getQueryCache().find({ queryKey: ['switch-node'], exact: true });
  if (query === undefined) throw new Error('The wizard reads its thread');
  let answer: () => void = () => undefined;
  state.readNode = () => new Promise((resolve) => { answer = () => resolve(state.node); });
  act(() => { query.setState({ data: undefined, error: null, status: 'pending', fetchStatus: 'idle' }); });
  void client.refetchQueries({ queryKey: ['switch-node'], exact: true });
  // Let the page see the thread without data.
  await act(async () => { await new Promise((done) => setTimeout(done, 30)); });
  expect(query.state.status).toBe('pending');
  return async () => { await act(async () => { answer(); await new Promise((done) => setTimeout(done, 30)); }); };
}

for (const locale of ['en', 'zh-CN'] as const) for (const [name, refusal] of Object.entries(refusals)) for (const moment of ['after', 'while'] as const) {
  it(`keeps the wizard, the refusal of ${name} and the focus on Switch when the thread is read again ${moment} the refusal (${locale})`, async () => {
    await setLocale(locale);
    let refuse: () => void = () => undefined;
    state.start.mockImplementation(() => moment === 'after' ? Promise.reject(refusal.error) : new Promise((_, reject) => { refuse = () => reject(refusal.error); }));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    show(<ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" job={null} say={null} open onClose={() => undefined} />, client);
    await userEvent.click(await screen.findByRole('button', { name: 'Choose next provider' }));
    const apply = screen.getByRole('button', { name: i18n.t('change.confirm.switch') });
    await waitFor(() => expect(apply).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.click(apply);
    expect(state.start).toHaveBeenCalledOnce();
    if (moment === 'after') await waitFor(() => expect(refusal.told()).toBeVisible());
    expect(apply).toHaveFocus();
    // The page reads the thread again: for a moment it has none, and then it has it.
    const arrive = await readAgain(client);
    expect(screen.getByRole('dialog', { name: i18n.t('change.title.switch', { name: 'Login' }) })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Choose next provider' })).toBeVisible();
    expect(apply.isConnected).toBe(true); expect(apply).toHaveFocus();
    if (moment === 'while') act(() => refuse());
    await arrive();
    // The refusal is told, and stays until the person acts; the dialog is the same one, its button is the same button, and nothing opened.
    await waitFor(() => expect(refusal.told()).toBeVisible());
    expect(screen.getByRole('dialog', { name: i18n.t('change.title.switch', { name: 'Login' }) })).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('change.confirm.switch') })).toBe(apply);
    expect(apply).toHaveFocus();
    expect(screen.queryAllByRole('combobox', { expanded: true })).toHaveLength(0);
    expect(state.start).toHaveBeenCalledOnce();
  });
}

function Cover() {
  const [open, setOpen] = useState(false);
  return <><button type="button" onClick={() => setOpen(true)}>Cover the wizard</button><Dialog open={open} onOpenChange={setOpen} title="Cover">Something above</Dialog></>;
}

it('tells the old-plan notice while a layer covers the wizard and the plan is asked for again', async () => {
  state.start.mockRejectedValue(refusals['an old plan'].error);
  show(<><ThreadChange threadId="thread" kind="switch" toTurn={null} focus="provider" job={null} say={null} open onClose={() => undefined} /><Cover /></>);
  await userEvent.click(await screen.findByRole('button', { name: 'Choose next provider' }));
  const apply = screen.getByRole('button', { name: i18n.t('change.confirm.switch') });
  await waitFor(() => expect(apply).not.toHaveAttribute('aria-disabled', 'true'));
  await userEvent.click(apply);
  expect(await screen.findByText(i18n.t('change.expired.made'))).toBeVisible();
  // A layer above covers the wizard, which is kept without its reads (the plan is asked for again when it comes back, and takes a while).
  act(() => screen.getByRole('button', { name: 'Cover the wizard', hidden: true }).click());
  const cover = await screen.findByRole('dialog', { name: 'Cover' });
  let answer: () => void = () => undefined;
  state.read.mockImplementation(() => new Promise<ForkPlan>((resolve) => { answer = () => resolve(state.answer!); }));
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(cover).not.toBeInTheDocument());
  expect(await screen.findByText(i18n.t('change.expired.made'))).toBeVisible();
  expect(screen.getByText(i18n.t('changeReady.checking'))).toBeVisible();
  answer();
  await waitFor(() => expect(screen.getByText(i18n.t('change.expired.made'))).toBeVisible());
  expect(screen.getByRole('button', { name: i18n.t('change.confirm.switch') })).toBe(apply);
});
