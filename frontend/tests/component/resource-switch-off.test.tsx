/** Switching an agent profile or an environment off runs at once: the running count is one warning line, the uses open on Show. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as EnvironmentApi from '@/api/environments';
import type * as ResourceApi from '@/api/resources';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useAgentActs } from '@/features/resources/agents/acts';
import { EditAgent } from '@/features/resources/agents/Edit';
import { useEnvironmentActs } from '@/features/resources/environments/acts';
import type { AgentListRow, Environment, FactTarget, ResourceImpact } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailAgent, detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(target: FactTarget) => Promise<ResourceImpact>>(), updateAgent: vi.fn(), disableEnvironment: vi.fn() }));
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['switch-off', target], queryFn: () => calls.read(target), retry: false }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourceApi>(), updateAgent: calls.updateAgent,
  agentQuery: (id: string) => ({ queryKey: ['switch-off-agent', id], queryFn: () => detailAgent }), harnessKindsQuery: { queryKey: ['switch-off-kinds'], queryFn: () => [] } }));
vi.mock('@/api/environments', async (original) => ({ ...await original<typeof EnvironmentApi>(), disableEnvironment: calls.disableEnvironment,
  environmentQuery: (id: string) => ({ queryKey: ['switch-off-environment', id], queryFn: () => detailEnvironment }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

function impact(target: FactTarget, running: number): ResourceImpact {
  const empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  const thread = { object: { kind: 'thread' as const, id: '00000000-0000-4000-8000-000000000101' }, name: 'Repair redirects', relation: 'binding' as const, until: null };
  return { target, name: 'Resource', stopping: false, applications: [], application_complete: true,
    activity: { running: { ...empty, count: running, threads: running, visible: running > 0 ? [thread] : [] }, waiting: empty, stopped: empty },
    facts: { subject: { kind: 'resource', object: target.object }, as_of: new Date().toISOString(), dependents_complete: true, uses: null,
      dependents: [{ relation: 'binding', count: 4, visible: [thread], hidden: 3, until: null }] } };
}
const agentTarget: FactTarget = { object: { kind: 'agent', id: detailAgent.id }, model_id: null };
const environmentTarget: FactTarget = { object: { kind: 'environment', id: detailEnvironment.id }, model_id: null };

beforeEach(() => { calls.updateAgent.mockImplementation((_id: string, patch: { disabled?: boolean }) => Promise.resolve({ ...detailAgent, disabled: patch.disabled ?? detailAgent.disabled })); calls.disableEnvironment.mockResolvedValue({ ...detailEnvironment, disabled: true }); });
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

function pending(): (value: ResourceImpact) => void {
  let ready: ((value: ResourceImpact) => void) | undefined;
  calls.read.mockImplementation(() => new Promise<ResourceImpact>((resolve) => { ready = resolve; }));
  return (value) => { ready?.(value); };
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`switching a profile off saves before its running count arrives and then says what stops (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const resolve = pending();
    const close = vi.fn();
    show(<EditAgent agent={{ ...detailAgent, disabled: false }} onProfile={vi.fn()} onClose={close} />);
    await user.click(screen.getByRole('switch', { name: i18n.t('agents.edit.on') }));
    const save = screen.getByRole('button', { name: i18n.t('resources.edit.save') });
    expect(save).toBeEnabled();
    expect(screen.getByText(i18n.t('resourceImpact.stopTip'))).toBeVisible();
    await waitFor(() => expect(calls.read).toHaveBeenCalledWith(agentTarget));
    act(() => resolve(impact(agentTarget, 2)));
    expect(await screen.findByText(i18n.t('resourceImpact.running', { count: 2, number: '2' }))).toBeVisible();
    expect(screen.queryByText(i18n.t('resourceImpact.bound', { count: 4, number: '4' }))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceImpact.showUses') }));
    expect(screen.getByText(i18n.t('resourceImpact.bound', { count: 4, number: '4' }))).toBeVisible();
    await user.click(save);
    await waitFor(() => expect(calls.updateAgent).toHaveBeenCalledExactlyOnceWith(detailAgent.id, expect.objectContaining({ disabled: true })));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
  // A cold worker's first profile editor took over five seconds beside other suites on a shared machine.
  }, 15_000);
}

it('switching a profile on reads nothing and keeps running the waiting messages an explicit choice', async () => {
  const user = userEvent.setup();
  show(<EditAgent agent={{ ...detailAgent, disabled: true }} onProfile={vi.fn()} onClose={vi.fn()} />);
  await user.click(screen.getByRole('switch', { name: i18n.t('agents.edit.on') }));
  const waiting = screen.getByRole('checkbox', { name: i18n.t('resourceImpact.runWaiting') });
  expect(waiting).not.toBeChecked();
  await user.click(waiting);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
  await waitFor(() => expect(calls.updateAgent).toHaveBeenCalledExactlyOnceWith(detailAgent.id, expect.objectContaining({ disabled: false }), undefined, true));
  expect(calls.read).not.toHaveBeenCalled();
});

function EnvironmentHost({ environment }: { environment: Environment }) {
  const acts = useEnvironmentActs();
  return <><button type="button" onClick={() => acts.ask('disable', environment)}>open</button>{acts.dialogs}</>;
}

it('an environment Disable is available while its running count is read, and a count of none says nothing', async () => {
  const user = userEvent.setup();
  const resolve = pending();
  show(<EnvironmentHost environment={detailEnvironment} />);
  await user.click(screen.getByRole('button', { name: 'open' }));
  const confirm = await screen.findByRole('button', { name: i18n.t('environments.danger.disableConfirm', { name: detailEnvironment.name }) });
  expect(confirm).toBeEnabled();
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(environmentTarget));
  act(() => resolve(impact(environmentTarget, 0)));
  await waitFor(() => expect(screen.queryByText(i18n.t('resourceImpact.stopTip'))).not.toBeInTheDocument());
  expect(screen.queryByText(/stop now/)).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox', { name: new RegExp(i18n.t('resourceImpact.note')) }), 'Maintenance');
  await user.click(confirm);
  await waitFor(() => expect(calls.disableEnvironment).toHaveBeenCalledExactlyOnceWith(detailEnvironment.id, 'Maintenance'));
});

it('an environment Disable still runs when its running count cannot be read and keeps the stop warning', async () => {
  const user = userEvent.setup();
  calls.read.mockRejectedValue(new Error('The current dependencies could not be read.'));
  show(<EnvironmentHost environment={detailEnvironment} />);
  await user.click(screen.getByRole('button', { name: 'open' }));
  const confirm = await screen.findByRole('button', { name: i18n.t('environments.danger.disableConfirm', { name: detailEnvironment.name }) });
  await waitFor(() => expect(calls.read).toHaveBeenCalled());
  expect(screen.getByText(i18n.t('resourceImpact.stopTip'))).toBeVisible();
  await user.click(confirm);
  await waitFor(() => expect(calls.disableEnvironment).toHaveBeenCalledExactlyOnceWith(detailEnvironment.id));
});

function AgentHost({ agent }: { agent: AgentListRow }) {
  const acts = useAgentActs();
  const disable = acts.page(agent).menu.find((entry) => 'id' in entry && entry.id === 'disable');
  return <><button type="button" onClick={() => { if (disable !== undefined && 'onSelect' in disable) disable.onSelect?.(); }}>open</button>{acts.dialogs}</>;
}

it('a profile Disable from its menu is available before its count arrives', async () => {
  const user = userEvent.setup();
  pending();
  show(<AgentHost agent={{ ...detailAgent, disabled: false } as unknown as AgentListRow} />);
  await user.click(screen.getByRole('button', { name: 'open' }));
  const confirm = await screen.findByRole('button', { name: i18n.t('agents.danger.disableConfirm', { name: detailAgent.name }) });
  expect(confirm).toBeEnabled();
  expect(screen.getByText(i18n.t('resourceImpact.stopTip'))).toBeVisible();
  await user.click(confirm);
  await waitFor(() => expect(calls.updateAgent).toHaveBeenCalledExactlyOnceWith(detailAgent.id, { expected_revision: detailAgent.revision, disabled: true }));
});
