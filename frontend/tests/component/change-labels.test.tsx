/** UA-21: ordinary picker values are display names; changing one still sends its original identifier. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { initialChoices, type Choices } from '@/features/change/choices';
import { RunsOn, useModelFor } from '@/features/change/RunsOn';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';

// These are only the reported facts read by this picker; provider usage/arrangement are independent views.
const reported = vi.hoisted(() => ({
  reads: vi.fn(),
  usableIds: null as string[] | null,
  providers: [{ id: 'target-provider', name: 'Provider display name', status: 'ready', protocols: ['openai'], owner: { display_name: 'Owner' }, home: { id: 'topic', name: 'Team', redacted: false }, placement: 'managed', machine: null }],
  agents: [{ id: 'agent', name: 'Agent display name', disabled: false, descriptor: { kind: 'mock' }, harness_kind: 'private_harness_key' }],
  models: [
    { model_name: 'private_route_to', display_name: 'Readable Model', enabled: true, parameters: {} },
    { model_name: 'private_route_other', display_name: 'Second Model', enabled: true, parameters: {} },
    { model_name: 'private_route_missing', display_name: '', enabled: true, parameters: {} },
  ],
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  providersQuery: (holding: string) => {
    const providers=holding==='usable' && reported.usableIds!==null ? reported.providers.filter((provider)=>reported.usableIds?.includes(provider.id)) : reported.providers;
    return { queryKey: ['display-providers', holding], initialData: providers, queryFn: () => Promise.resolve(providers) };
  },
  agentsQuery: () => ({ queryKey: ['display-agents'], initialData: reported.agents, queryFn: () => Promise.resolve(reported.agents) }),
  providerModelsQuery: () => ({ queryKey: ['display-models'], initialData: reported.models, queryFn: () => { reported.reads(); return Promise.resolve(reported.models); } }),
}));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/start/Arrangement', () => ({ ProviderArrangementChoice: () => null }));

afterEach(async () => { reported.providers[0]!.status='ready';reported.usableIds=null;await setLocale('en'); });

/** What the thread's binding options answer: the same provider and agent as the lists above, with the models and agents it lists. */
function options() {
  const [provider] = reported.providers;
  return { providers: [{ provider: { id: provider!.id, name: provider!.name, address: 'team.owner', status: provider!.status }, usable: reported.usableIds === null || reported.usableIds.includes(provider!.id),
    reason: reported.usableIds === null || reported.usableIds.includes(provider!.id) ? null : 'no_use', owner: null,
    models: reported.models.map((model) => ({ ...model, allowed: true, reason: null })), agents: reported.agents.map((agent) => ({ agent: { id: agent.id, name: agent.name }, usable: true, reason: null, arrangements: [] })) }] };
}
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), threadBindingOptionsQuery: (id: string) => ({ queryKey: ['display-binding-options', id], queryFn: () => Promise.resolve(options()) }) }));
const earlier = { model: 'private_route_from', model_display: 'Earlier Model', effort: null, binding: { provider: { id: 'source-provider', name: 'Earlier provider' }, agent: { id: 'source-agent', name: 'Earlier agent' } } } as unknown as Thread;
const modelChip = () => screen.findByRole('button', { name: new RegExp(`^${i18n.t('composer.model.chip')}`) });

it('an exhausted provider remains selectable by its models in the ordinary switch, and the agent that goes with it is said',async()=>{
  reported.providers[0]!.status='exhausted';
  const changed=vi.fn();
  show(<RunsOn compact thread={earlier} choices={initialChoices('replace')} onChange={changed}/>);
  await userEvent.click(await modelChip());
  const option=await screen.findByRole('menuitemradio',{name:'Readable Model'});
  expect(option).not.toBeDisabled();
  await userEvent.click(option);
  expect(changed).toHaveBeenCalledWith({providerId:'target-provider',agentId:'agent',model:'private_route_to',effort:null,arrangement:null,
    automaticRuns:{profile:{providerId:'target-provider',agentId:'agent',by:'first',previous:'unavailable'},effort:{providerId:'target-provider',model:'private_route_to',value:null,by:'unreported',replaced:false}}});
  expect(await screen.findByText(i18n.t('composer.pick.agent.changed',{agent:'Agent display name',was:'Earlier agent',provider:'Provider display name'}))).toBeVisible();
});

it.each(['needs_setup','error','offline','disabled','no_use'] as const)('keeps %s providers unavailable even when selecting a replacement',async(reason)=>{
  reported.providers[0]!.status=reason==='no_use'?'exhausted':reason;
  if(reason==='no_use')reported.usableIds=[];
  const changed=vi.fn();
  show(<RunsOn compact thread={earlier} choices={initialChoices('replace')} onChange={changed}/>);
  await userEvent.click(await modelChip());
  const group=await screen.findByText(i18n.t('composer.pick.provider.off'));
  expect(group).toBeVisible();
  const row=screen.getByRole('menuitemradio',{name:/^Provider display name/});
  expect(row).toBeDisabled();
  await userEvent.click(row);
  expect(changed).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`has a model control and an agent control, and no pair of agent and provider, for an ordinary switch (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn();
  function Form() {
    const [choices, setChoices] = useState<Choices>(initialChoices('replace'));
    return <RunsOn compact thread={earlier} choices={choices} onChange={(patch) => { setChoices((current) => ({ ...current, ...patch })); changed(patch); }} />;
  }
  show(<Form />);
  await modelChip();
  expect(screen.getByRole('button', { name: new RegExp(`^${i18n.t('composer.row.agentName')}`) })).toBeVisible();
  expect(screen.queryAllByRole('combobox')).toHaveLength(0);
});

it('settles a switch when the new provider lists the same model as the current provider', async () => {
  const thread = { model: 'private_route_to', model_display: 'Readable Model', effort: null, binding: { provider: { id: 'source-provider' }, agent: { id: 'agent' } } } as unknown as Thread;
  const changed = vi.fn();
  function Form() {
    const [choices, setChoices] = useState<Choices>({ ...initialChoices('replace'), providerId: 'target-provider' });
    useModelFor(thread, choices, (patch) => { changed(patch); setChoices((current) => ({ ...current, ...patch })); });
    return <output>{choices.model ?? 'waiting'}</output>;
  }
  show(<Form />);
  await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('private_route_to'));
  expect(changed).toHaveBeenCalledOnce();
  expect(changed).toHaveBeenCalledWith({ model: 'private_route_to', effort: null, automaticRuns: { model: { providerId: 'target-provider', model: 'private_route_to', by: 'retained', replaced: false }, effort: { providerId: 'target-provider', model: 'private_route_to', value: null, by: 'unreported', replaced: false } } });
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps routing and harness keys out of the form and the open model list (${locale})`, async () => {
  await setLocale(locale);
  const thread = { model: 'private_route_from', model_display: 'Earlier Model', effort: null, binding: { provider: { id: 'source-provider', name: 'Source' }, agent: { id: 'source-agent', name: 'Source agent' } } } as unknown as Thread;
  const changed = vi.fn();
  function Form() {
    const [choices, setChoices] = useState<Choices>({ ...initialChoices('replace'), providerId: 'target-provider', model: 'private_route_to' });
    return <RunsOn thread={thread} choices={choices} onChange={(patch) => { setChoices((current) => ({ ...current, ...patch })); changed(patch); }} />;
  }
  const { container } = show(<Form />);
  const picker = await modelChip();
  expect(picker).toHaveTextContent('Readable Model');
  expect(container).not.toHaveTextContent('private_route');
  expect(container).not.toHaveTextContent('private_harness_key');
  const user = userEvent.setup();
  await user.click(picker);
  expect(await screen.findByRole('menuitemradio', { name: 'Readable Model' })).toBeVisible();
  expect(screen.getByRole('menuitemradio', { name: i18n.t('composer.model.displayUnknown') })).toBeVisible();
  expect(screen.queryByText('private_route_missing')).toBeNull();
  expect(screen.queryByText('private_route_to')).toBeNull();
  expect(screen.queryByText('private_route_other')).toBeNull();
  await user.click(screen.getByRole('menuitemradio', { name: 'Second Model' }));
  expect(changed).toHaveBeenLastCalledWith({ providerId: 'target-provider', agentId: 'agent', model: 'private_route_other', effort: null, arrangement: null,
    automaticRuns: { profile: { providerId: 'target-provider', agentId: 'agent', by: 'first', previous: 'unavailable' }, effort: { providerId: 'target-provider', model: 'private_route_other', value: null, by: 'unreported', replaced: false } } });
  expect(picker).toHaveTextContent('Second Model');
});

it('does not read model choices or apply a default when the plan is no longer shown', () => {
  reported.reads.mockClear();
  const thread = { model: 'private_route_from', model_display: 'Earlier Model', effort: null, binding: { provider: { id: 'source-provider' }, agent: { id: 'agent' } } } as unknown as Thread;
  const changed = vi.fn();
  function HiddenChoices() {
    useModelFor(thread, { ...initialChoices('replace'), providerId: 'target-provider' }, changed, false);
    return null;
  }
  show(<HiddenChoices />);
  expect(reported.reads).not.toHaveBeenCalled();
  expect(changed).not.toHaveBeenCalled();
});
