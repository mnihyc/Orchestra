/** Switch/fork defaults explain their origins while rejected readable choices remain inspectable. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as ResourcesApi from '@/api/resources';
import { initialChoices, requestOf, type Choices } from '@/features/change/choices';
import { automaticProfileFor, RunsOn, useModelFor } from '@/features/change/RunsOn';
import type { Agent, Provider, ProviderModel, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { harnessName } from '../doubles/harness-names';
import { planProvider, OBSERVED } from '../unit/provider-plan-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ providers: [] as Provider[], agents: [] as Agent[], models: [] as ProviderModel[], modelError: null as ApiError | null, modelCached: false, modelRead: vi.fn(), change: vi.fn<(patch: Partial<Choices>) => void>() }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourcesApi>(),
  providersQuery: (holding: string) => ({ queryKey: ['implicit-providers', holding], initialData: server.providers, queryFn: () => Promise.resolve(server.providers) }),
  agentsQuery: () => ({ queryKey: ['implicit-agents'], initialData: server.agents, queryFn: () => Promise.resolve(server.agents) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['implicit-models', id], initialData: server.modelError === null || server.modelCached ? server.models : undefined, queryFn: () => { server.modelRead(); return server.modelError === null ? Promise.resolve(server.models) : Promise.reject(server.modelError); } }),
}));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/start/Arrangement', () => ({ ProviderArrangementChoice: () => null }));
function agent(id: string, name: string, disabled = false): Agent {
  return { id, name, disabled, address: `test.${id}`, harness_kind: 'fake', harness: harnessName('fake'), descriptor: { kind: 'fake' },
    descriptor_overridden: [], owner: null, org_id: null, home: null, machine: null, origin_machine: null, availability: [],
    profile: {}, revision: 1, created_at: OBSERVED, updated_at: OBSERVED, access: { permissions: ['agent.view', 'agent.use'], administers: false, through: [], admin_only: [], participating: true } };
}
const thread = { model: 'private_old_model', model_display: 'Earlier model', effort: 'low', binding: { provider: { id: 'source-provider' }, agent: { id: 'old' } } } as unknown as Thread;
function Form({ result = 'new', compact = false, initial }: { result?: Choices['result']; compact?: boolean; initial?: Choices }) {
  const [choices, setChoices] = useState(initial ?? initialChoices(result));
  const patch = (next: Partial<Choices>) => { server.change(next); setChoices((current) => ({ ...current, ...next })); };
  useModelFor(thread, choices, patch);
  return <><RunsOn thread={thread} choices={choices} compact={compact} onChange={patch} /><output data-testid="native-request">{JSON.stringify(requestOf(choices))}</output></>;
}
afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) describe(`run choices in ${locale}`, () => {
  const t = i18n.getFixedT(locale);
  beforeEach(async () => {
    await setLocale(locale); server.change.mockClear(); server.modelError = null; server.modelCached = false; server.modelRead.mockClear();
    server.agents = [agent('old', 'Earlier profile', true), agent('keeper', 'Keeper profile'), agent('other', 'Other profile'), { ...agent('unreported', 'Unreported profile'), descriptor: {} }];
    server.providers = [planProvider({ id: 'target-provider', name: 'Readable provider', protocols: [], agent: server.agents[1]! })];
    server.models = [
      { ...MODEL_ESTIMATE, id: 'old-model', model_name: 'private_old_model', display_name: 'Earlier model', enabled: false, source: 'manual', groups: [], parameters: {} },
      { ...MODEL_ESTIMATE, id: 'first-model', model_name: 'private_first_model', display_name: 'First model', enabled: true, source: 'manual', groups: [], parameters: { efforts: ['high', 'max'], default_effort: 'high' } },
      { ...MODEL_ESTIMATE, id: 'other-model', model_name: 'private_other_model', display_name: 'Other model', enabled: true, source: 'manual', groups: [], parameters: { efforts: ['high', 'max'], default_effort: 'max' } },
    ];
  });
  it('shows keeper/default origins and why previous choices cannot carry over in a fork', async () => {
    show(<Form />);
    await userEvent.click(screen.getByRole('combobox', { name: t('change.runs.provider'), exact: true }));
    await userEvent.click(screen.getByRole('option', { name: 'Readable provider', exact: true }));
    await screen.findByText(t('change.runs.automatic.profile.keeper'), { exact: false });
    expect(screen.getByText(t('change.runs.automatic.previousProfile.disabled'), { exact: false })).toBeVisible();
    expect(await screen.findByText(t('change.runs.automatic.model.first'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.previousModel'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.effort.default'), { exact: false })).toBeVisible();
    expect(screen.getByText(t('change.runs.automatic.previousEffort'), { exact: false })).toBeVisible();
    const request = JSON.parse(screen.getByTestId('native-request').textContent) as unknown;
    expect(request).toEqual({ result: 'new', to: { provider_id: 'target-provider', agent_id: 'keeper', model: 'private_first_model', effort: 'high' } });
    await userEvent.click(screen.getByRole('combobox', { name: t('change.runs.agent'), exact: true }));
    for (const [name, key] of [['Earlier profile', 'change.runs.unavailable.profileDisabled'], ['Other profile', 'change.runs.unavailable.keeper'], ['Unreported profile', 'start.agent.unreported']] as const) {
      const option = screen.getByRole('option', { name, exact: true });
      expect(option).toHaveAttribute('aria-disabled', 'true'); expect(option).toHaveAccessibleDescription(t(key));
      const calls = server.change.mock.calls.length; await userEvent.click(option); expect(server.change).toHaveBeenCalledTimes(calls);
    }
    await userEvent.keyboard('{Escape}');
  });
  it('keeps disabled models readable and clears automatic origins after an explicit choice', async () => {
    const target = server.providers[0]!;
    show(<Form initial={{ ...initialChoices('new'), providerId: target.id, agentId: 'keeper', automaticRuns: automaticProfileFor(target, server.agents, thread) }} />);
    await screen.findByText(t('change.runs.automatic.model.first'), { exact: false });
    const model = screen.getByRole('combobox', { name: t('change.runs.model'), exact: true }); await userEvent.click(model);
    const refused = screen.getByRole('option', { name: 'Earlier model', exact: true });
    expect(refused).toHaveAttribute('aria-disabled', 'true'); expect(refused).toHaveAccessibleDescription(t('change.runs.unavailable.modelDisabled'));
    await userEvent.click(screen.getByRole('option', { name: 'Other model', exact: true }));
    expect(screen.queryByText(t('change.runs.automatic.model.first'), { exact: false })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('combobox', { name: t('change.runs.effort'), exact: true }));
    await userEvent.click(screen.getByRole('option', { name: t('choiceCatalog.effort.max.name'), exact: true }));
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
    expect(screen.getByRole('combobox', { name: t('change.runs.model'), exact: true })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: t('change.runs.model'), exact: true })).toHaveTextContent(t('common.loading'));
    const retry = await screen.findByRole('button', { name: t('common.tryAgain'), exact: true });
    expect(screen.getByRole('combobox', { name: t('change.runs.model'), exact: true })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: t('change.runs.model'), exact: true })).toHaveTextContent(t('change.runs.modelUnknown'));
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
    await screen.findByRole('button', { name: t('common.tryAgain'), exact: true });
    const model = screen.getByRole('combobox', { name: t('change.runs.model'), exact: true });
    expect(model).toBeEnabled(); await userEvent.click(model);
    await userEvent.click(screen.getByRole('option', { name: 'Other model', exact: true }));
    expect(model).toHaveTextContent('Other model');
    expect(JSON.parse(screen.getByTestId('native-request').textContent) as unknown).toEqual({ result: 'replace', to: { provider_id: 'target-provider', agent_id: 'keeper', model: 'private_other_model', effort: 'high' } });
    expect(screen.queryByText(t('change.runs.automatic.model.first'), { exact: false })).not.toBeInTheDocument();
  });
  it('keeps an explicitly empty model list distinct from a failed or pending read', async () => {
    server.models = [];
    show(<Form initial={{ ...initialChoices('replace'), providerId: 'target-provider', agentId: 'keeper' }} />);
    const model = screen.getByRole('combobox', { name: t('change.runs.model'), exact: true });
    expect(model).toBeDisabled(); expect(await screen.findByText(t('start.model.none'))).toBeVisible();
    expect(screen.queryByRole('button', { name: t('common.tryAgain'), exact: true })).not.toBeInTheDocument();
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
  it('keeps refused pairs in a compact switch and marks a retained model correctly', async () => {
    server.providers[0]!.protocols = ['openai']; server.models[0]!.enabled = true;
    const target = server.providers[0]!;
    const initial = { ...initialChoices('replace'), providerId: target.id, agentId: 'keeper', automaticRuns: automaticProfileFor(target, server.agents, thread) };
    show(<Form result="replace" compact initial={initial} />);
    expect(await screen.findByText(t('change.runs.automatic.model.retained'), { exact: false })).toBeVisible();
    expect(screen.queryByText(t('change.runs.automatic.previousModel'), { exact: false })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('combobox', { name: t('switchChoice.picker'), exact: true }));
    const refused = screen.getByRole('option', { name: t('switchChoice.option', { agent: 'Earlier profile', provider: 'Readable provider' }), exact: true });
    expect(refused).toHaveAttribute('aria-disabled', 'true'); expect(refused).toHaveAccessibleDescription(expect.stringContaining(t('change.runs.unavailable.profileDisabled')));
    await userEvent.click(screen.getByRole('option', { name: t('switchChoice.option', { agent: 'Other profile', provider: 'Readable provider' }), exact: true }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: t('switchChoice.picker'), exact: true })).toHaveValue(t('switchChoice.option', { agent: 'Other profile', provider: 'Readable provider' })));
    expect(screen.queryByText(t('change.runs.automatic.profile.first'), { exact: false })).not.toBeInTheDocument();
    expect(server.change).toHaveBeenCalledWith({ providerId: 'target-provider', agentId: 'other', model: null, effort: undefined, arrangement: null, automaticRuns: {} });
  });
});
