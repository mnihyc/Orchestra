/** Switch/fork defaults explain their origins while rejected readable choices remain inspectable. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as ResourcesApi from '@/api/resources';
import type * as StartApi from '@/api/start';
import { initialChoices, requestOf, type Choices } from '@/features/change/choices';
import { automaticProfileFor, RunsOn, useModelFor } from '@/features/change/RunsOn';
import type { Agent, ModelOption, Provider, ProviderModel, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { harnessName } from '../doubles/harness-names';
import { planProvider, OBSERVED } from '../unit/provider-plan-fixtures';
import { provider as bindingProvider } from '../unit/start-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const server = vi.hoisted(() => ({ providers: [] as Provider[], agents: [] as Agent[], models: [] as ProviderModel[], modelError: null as ApiError | null, modelCached: false, modelRead: vi.fn(), change: vi.fn<(patch: Partial<Choices>) => void>(), binding: () => ({ providers: [] as unknown[] }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  providersQuery: (holding: string) => ({ queryKey: ['implicit-providers', holding], initialData: server.providers, queryFn: () => Promise.resolve(server.providers) }),
  agentsQuery: () => ({ queryKey: ['implicit-agents'], initialData: server.agents, queryFn: () => Promise.resolve(server.agents) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['implicit-models', id], initialData: server.modelError === null || server.modelCached ? server.models : undefined, queryFn: () => { server.modelRead(); return server.modelError === null ? Promise.resolve(server.models) : Promise.reject(server.modelError); } }),
}));
vi.mock('@/api/start', async (original) => ({ ...await original<typeof StartApi>(),
  threadBindingOptionsQuery: () => ({ queryKey: ['implicit-binding'], queryFn: () => Promise.resolve(server.binding()) }) }));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/start/Arrangement', () => ({ ProviderArrangementChoice: () => null }));
function agent(id: string, name: string, disabled = false): Agent {
  return { id, name, disabled, address: `test.${id}`, harness_kind: 'fake', harness: harnessName('fake'), descriptor: { kind: 'fake' },
    descriptor_overridden: [], owner: null, org_id: null, home: null, machine: null, origin_machine: null, availability: [],
    profile: {}, revision: 1, created_at: OBSERVED, updated_at: OBSERVED, access: { permissions: ['agent.view', 'agent.use'], administers: false, through: [], admin_only: [], participating: true } };
}
const thread = { model: 'private_old_model', model_display: 'Earlier model', effort: 'low', binding: { provider: { id: 'source-provider' }, agent: { id: 'old' } } } as unknown as Thread;
function Form({ result = 'new', compact = false, initial, on = thread }: { result?: Choices['result']; compact?: boolean; initial?: Choices; on?: Thread }) {
  const [choices, setChoices] = useState(initial ?? initialChoices(result));
  const patch = (next: Partial<Choices>) => { server.change(next); setChoices((current) => ({ ...current, ...next })); };
  useModelFor(on, choices, patch);
  return <><RunsOn thread={on} choices={choices} compact={compact} onChange={patch} /><output data-testid="native-request">{JSON.stringify(requestOf(choices))}</output></>;
}
/** What the thread could be bound to: the models of the target provider, as the model list groups them under its name. */
function listed(): unknown {
  const models: ModelOption[] = server.models.map((one) => ({ ...MODEL_ESTIMATE, model_name: one.model_name, display_name: one.display_name, allowed: one.enabled, reason: one.enabled ? null : 'disabled', parameters: one.parameters }));
  const base = bindingProvider('target-provider', { models });
  return { providers: [{ ...base, provider: { ...base.provider, name: 'Readable provider' } }] };
}
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) describe(`run choices in ${locale}`, () => {
  const t = i18n.getFixedT(locale);
  beforeEach(async () => {
    await setLocale(locale); server.change.mockClear(); server.modelError = null; server.modelCached = false; server.modelRead.mockClear(); server.binding = () => listed() as { providers: unknown[] };
    server.agents = [agent('old', 'Earlier profile', true), agent('keeper', 'Keeper profile'), agent('other', 'Other profile'), { ...agent('unreported', 'Unreported profile'), descriptor: {} }];
    server.providers = [planProvider({ id: 'target-provider', name: 'Readable provider', protocols: [], agent: server.agents[1]! })];
    server.models = [
      { ...MODEL_ESTIMATE, id: 'old-model', model_name: 'private_old_model', display_name: 'Earlier model', enabled: false, source: 'manual', groups: [], parameters: {} },
      { ...MODEL_ESTIMATE, id: 'first-model', model_name: 'private_first_model', display_name: 'First model', enabled: true, source: 'manual', groups: [], parameters: { efforts: ['high', 'max'], default_effort: 'high' } },
      { ...MODEL_ESTIMATE, id: 'other-model', model_name: 'private_other_model', display_name: 'Other model', enabled: true, source: 'manual', groups: [], parameters: { efforts: ['high', 'max'], default_effort: 'max' } },
    ];
  });
  /** The model list of the run choices, opened. */
  const openModels = async () => userEvent.click(screen.getByRole('button', { name: new RegExp(`^${t('composer.model.chip')}`) }));
  it('shows keeper/default origins and why previous choices cannot carry over in a fork', async () => {
    show(<Form />);
    // The provider is the model's: choosing its first model chooses it, with the agent that goes with it.
    await openModels();
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'First model' }));
    await screen.findByText(t('change.runs.automatic.profile.keeper'), { exact: false });
    expect(screen.getByText(t('change.runs.automatic.previousProfile.disabled'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.effort.default'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.previousEffort'), { exact: false })).toBeVisible();
    const request = JSON.parse(screen.getByTestId('native-request').textContent) as unknown;
    expect(request).toEqual({ result: 'new', to: { provider_id: 'target-provider', agent_id: 'keeper', model: 'private_first_model', effort: 'high' } });
    // A provider that runs its keeper agent only has nothing to choose among, and says so.
    const agentChip = screen.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) });
    expect(agentChip).toHaveTextContent('Keeper profile'); expect(agentChip).toBeDisabled();
    expect(agentChip).toHaveAttribute('aria-description', t('composer.pick.agent.only'));
  });
  it('keeps disabled models readable and clears automatic origins after an explicit choice', async () => {
    const target = server.providers[0]!;
    show(<Form initial={{ ...initialChoices('new'), providerId: target.id, agentId: 'keeper', automaticRuns: automaticProfileFor(target, server.agents, thread) }} />);
    await screen.findByText(t('change.runs.automatic.model.first'), { exact: false });
    await openModels();
    const refused = await screen.findByRole('menuitemradio', { name: 'Earlier model' });
    expect(refused).toBeDisabled(); expect(refused.closest('li')).toHaveTextContent(t('composer.model.reason.disabled', { provider: 'Readable provider' }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Other model' }));
    expect(screen.queryByText(t('change.runs.automatic.model.first'), { exact: false })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: new RegExp(`^${t('change.runs.effort')}`) }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: t('choiceCatalog.effort.max.name') }));
    expect(screen.queryByText(t('change.runs.automatic.effort.retained'), { exact: false })).not.toBeInTheDocument();
    expect(screen.queryByText(t('change.runs.automatic.effort.default'), { exact: false })).not.toBeInTheDocument();
    expect(screen.queryByText('private_first_model')).not.toBeInTheDocument();
  });
  it('distinguishes unreported effort facts from an explicitly empty effort list', async () => {
    const initial = { ...initialChoices('replace'), providerId: 'target-provider', agentId: 'keeper' };
    server.models[1]!.parameters = {};
    const unknown = show(<Form initial={initial} />);
    expect(await screen.findByText(t('change.runs.automatic.effort.unreported'), { exact: false })).toBeVisible();
    expect(screen.queryByText(t('change.runs.automatic.effort.none'), { exact: false })).not.toBeInTheDocument();
    unknown.unmount(); server.models[1]!.parameters = { efforts: [] };
    show(<Form initial={initial} />);
    expect(await screen.findByText(t('change.runs.automatic.effort.none'), { exact: false })).toBeVisible();
    expect(screen.queryByText(t('change.runs.automatic.effort.unreported'), { exact: false })).not.toBeInTheDocument();
  });
  it('offers Retry after a failed model read without pretending no model is supported or applying a fallback', async () => {
    server.modelError = new ApiError({ type: 'orch:error', code: 'source.offline', status: 503, title: '', detail: '', request_id: 'model-read' }, null);
    show(<Form initial={{ ...initialChoices('replace'), providerId: 'target-provider', agentId: 'keeper' }} />);
    const retry = await screen.findByRole('button', { name: t('common.tryAgain') });
    expect(screen.queryByText(t('start.model.none'))).not.toBeInTheDocument();
    expect(screen.queryByText(t('change.runs.automatic.model.first'), { exact: false })).not.toBeInTheDocument();
    expect(server.change).not.toHaveBeenCalled(); expect(server.modelRead).toHaveBeenCalledTimes(1);
    server.modelError = null; await userEvent.click(retry);
    expect(await screen.findByText(t('change.runs.automatic.model.first'), { exact: false })).toBeVisible();
    expect(server.modelRead).toHaveBeenCalledTimes(2);
  });
  it('keeps an explicit cached choice usable while reporting a failed refresh for native planning', async () => {
    server.modelCached = true; server.modelError = new ApiError({ type: 'orch:error', code: 'source.offline', status: 503, title: '', detail: '', request_id: 'model-refresh' }, null);
    show(<Form initial={{ ...initialChoices('replace'), providerId: 'target-provider', agentId: 'keeper', model: 'private_first_model', effort: 'high' }} />);
    await screen.findByRole('button', { name: t('common.tryAgain') });
    const model = screen.getByRole('button', { name: new RegExp(`^${t('composer.model.chip')}`) });
    expect(model).toBeEnabled(); expect(model).toHaveTextContent('First model');
    await userEvent.click(model);
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Other model' }));
    expect(model).toHaveTextContent('Other model');
    expect(JSON.parse(screen.getByTestId('native-request').textContent) as unknown).toEqual({ result: 'replace', to: { provider_id: 'target-provider', agent_id: 'keeper', model: 'private_other_model', effort: 'high' } });
    expect(screen.queryByText(t('change.runs.automatic.model.first'), { exact: false })).not.toBeInTheDocument();
  });
  it('keeps an explicitly empty model list distinct from a failed or pending read', async () => {
    server.models = [];
    show(<Form initial={{ ...initialChoices('replace'), providerId: 'target-provider', agentId: 'keeper' }} />);
    expect(await screen.findByText(t('start.model.none'))).toBeVisible();
    expect(screen.queryByRole('button', { name: t('common.tryAgain') })).not.toBeInTheDocument();
    expect(server.change).not.toHaveBeenCalled();
  });
  it('uses an unnamed unavailable-profile reason when the previous profile is not readable', async () => {
    server.agents = server.agents.filter((candidate) => candidate.id !== 'old');
    server.providers[0]!.protocols = ['openai'];
    const provider = server.providers[0]!;
    show(<Form initial={{ ...initialChoices('new'), providerId: provider.id, agentId: 'keeper', automaticRuns: automaticProfileFor(provider, server.agents, thread) }} />);
    expect(await screen.findByText(t('change.runs.automatic.profile.first'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.previousProfile.unavailable'), { exact: false })).toBeVisible();
    expect(screen.queryByText('Earlier profile')).not.toBeInTheDocument();
    expect(screen.queryByText(t('change.runs.automatic.previousProfile.disabled'), { exact: false })).not.toBeInTheDocument();
  });
  it('keeps the agent that was chosen when another model of the provider the thread is on is chosen with it', async () => {
    server.providers[0]!.protocols = ['openai'];
    const here = { ...thread, binding: { provider: { id: 'target-provider' }, agent: { id: 'keeper' } } } as unknown as Thread;
    show(<Form result="replace" compact on={here} initial={{ ...initialChoices('replace'), agentId: 'other', model: 'private_first_model', effort: 'high' }} />);
    await openModels();
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'Other model' }));
    expect(JSON.parse(screen.getByTestId('native-request').textContent) as unknown).toEqual({ result: 'replace', to: { agent_id: 'other', model: 'private_other_model', effort: 'high' } });
  });
  it('keeps the agent just chosen after switching provider and choosing another model', async () => {
    server.providers[0]!.protocols = ['openai'];
    server.agents[0]!.disabled = false;
    show(<Form result="replace" compact />);
    await openModels();
    await userEvent.click(await screen.findByRole('menuitemradio', { name: 'First model' }));
    await userEvent.click(screen.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) }));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Other profile' }));
    await openModels();
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Other model' }));
    expect(JSON.parse(screen.getByTestId('native-request').textContent) as unknown).toEqual({ result: 'replace', to: {
      provider_id: 'target-provider', agent_id: 'other', model: 'private_other_model', effort: 'high',
    } });
    expect(screen.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) })).toHaveTextContent('Other profile');
    expect(document.querySelector('[data-agent-notice]')).toBeNull();
  });
  it('keeps refused agents in a simple switch off in the agent list, and keeps a retained model', async () => {
    server.providers[0]!.protocols = ['openai']; server.models[0]!.enabled = true;
    const target = server.providers[0]!;
    const initial = { ...initialChoices('replace'), providerId: target.id, agentId: 'keeper', automaticRuns: automaticProfileFor(target, server.agents, thread) };
    show(<Form result="replace" compact initial={initial} />);
    // The thread's own model is kept where the provider lists it, and a simple switch says nothing more of it than the agent that went with the provider.
    await waitFor(() => expect(JSON.parse(screen.getByTestId('native-request').textContent) as unknown).toMatchObject({ to: { model: 'private_old_model' } }));
    expect(screen.queryByText(t('change.runs.automatic.model.retained'), { exact: false })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) }));
    const refused = screen.getByRole('menuitemradio', { name: /^Earlier profile/ });
    expect(refused).toBeDisabled(); expect(refused.closest('li')).toHaveTextContent(t('composer.pick.agent.why.disabled'));
    await userEvent.click(screen.getByRole('menuitemradio', { name: 'Other profile' }));
    await waitFor(() => expect(screen.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) })).toHaveTextContent('Other profile'));
    expect(screen.queryByText(t('change.runs.automatic.profile.first'), { exact: false })).not.toBeInTheDocument();
    // The agent is the person's now, so the line that said where it came from goes; what the model and the effort came from stays.
    const last = server.change.mock.lastCall?.[0];
    expect(last).toMatchObject({ agentId: 'other' });
    expect(last?.automaticRuns).not.toHaveProperty('profile');
  });
});
