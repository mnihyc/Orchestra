/** UA-21: ordinary picker values are display names; changing one still sends its original identifier. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import { initialChoices, type Choices } from '@/features/change/choices';
import { RunsOn, useModelFor } from '@/features/change/RunsOn';
import type { Thread } from '@/generated/api-types';
import { setLocale } from '@/i18n';

import { show } from './support';

// These are only the reported facts read by this picker; provider usage/arrangement are independent views.
const reported = vi.hoisted(() => ({
  reads: vi.fn(),
  providers: [{ id: 'target-provider', name: 'Provider display name', status: 'ready', protocols: ['openai'], owner: { display_name: 'Owner' }, home: { id: 'topic', name: 'Team', redacted: false }, placement: 'managed', machine: null }],
  agents: [{ id: 'agent', name: 'Agent display name', disabled: false, descriptor: { kind: 'mock' }, harness_kind: 'private_harness_key' }],
  models: [
    { model_name: 'private_route_to', display_name: 'Readable Model', enabled: true, parameters: {} },
    { model_name: 'private_route_other', display_name: 'Second Model', enabled: true, parameters: {} },
    { model_name: 'private_route_missing', display_name: '', enabled: true, parameters: {} },
  ],
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  providersQuery: (holding: string) => ({ queryKey: ['display-providers', holding], initialData: reported.providers, queryFn: () => Promise.resolve(reported.providers) }),
  agentsQuery: () => ({ queryKey: ['display-agents'], initialData: reported.agents, queryFn: () => Promise.resolve(reported.agents) }),
  providerModelsQuery: () => ({ queryKey: ['display-models'], initialData: reported.models, queryFn: () => { reported.reads(); return Promise.resolve(reported.models); } }),
}));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/start/Arrangement', () => ({ ProviderArrangementChoice: () => null }));

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`uses one named provider/profile picker for an ordinary switch (${locale})`, async () => {
  await setLocale(locale);
  const thread = { model: 'private_route_from', model_display: 'Earlier Model', effort: null, binding: { provider: { id: 'source-provider' }, agent: { id: 'source-agent' } } } as unknown as Thread;
  const changed = vi.fn();
  function Form() {
    const [choices, setChoices] = useState<Choices>(initialChoices('replace'));
    return <RunsOn compact thread={thread} choices={choices} onChange={(patch) => { setChoices((current) => ({ ...current, ...patch })); changed(patch); }} />;
  }
  show(<Form />);
  const label = locale === 'en' ? 'Agent and provider' : '代理与提供方';
  const choice = await screen.findByRole('combobox', { name: label });
  expect(screen.getAllByRole('combobox')).toHaveLength(1);
  await userEvent.click(choice);
  const name = locale === 'en' ? 'Agent display name on Provider display name' : 'Agent display name，使用 Provider display name';
  await userEvent.click(screen.getByRole('option', { name }));
  expect(changed).toHaveBeenCalledWith({ providerId: 'target-provider', agentId: 'agent', model: null, effort: undefined, arrangement: null });
  expect(choice).toHaveTextContent(name);
  expect(screen.getByRole('combobox', { name: locale === 'en' ? 'Model' : '模型' })).toBeVisible();
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
  expect(changed).toHaveBeenCalledWith({ model: 'private_route_to', effort: null });
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps routing and harness keys out of the form and the open model list (${locale})`, async () => {
  await setLocale(locale);
  const thread = { model: 'private_route_from', model_display: 'Earlier Model', effort: null, binding: { provider: { id: 'source-provider', name: 'Source' }, agent: { id: 'agent' } } } as unknown as Thread;
  const changed = vi.fn();
  function Form() {
    const [choices, setChoices] = useState<Choices>({ ...initialChoices('replace'), providerId: 'target-provider', model: 'private_route_to' });
    return <RunsOn thread={thread} choices={choices} onChange={(patch) => { setChoices((current) => ({ ...current, ...patch })); changed(patch); }} />;
  }
  const { container } = show(<Form />);
  const label = locale === 'en' ? 'Model' : '模型';
  const picker = screen.getByRole('combobox', { name: label });
  expect(picker).toHaveTextContent('Readable Model');
  expect(container).not.toHaveTextContent('private_route');
  expect(container).not.toHaveTextContent('private_harness_key');
  const user = userEvent.setup();
  await user.click(picker);
  expect(screen.getByRole('option', { name: 'Readable Model' })).toBeVisible();
  expect(screen.getByRole('option', { name: locale === 'en' ? 'Model name unavailable' : '模型名称不可用' })).toBeVisible();
  expect(screen.queryByText('private_route_missing')).toBeNull();
  expect(screen.queryByText('private_route_to')).toBeNull();
  expect(screen.queryByText('private_route_other')).toBeNull();
  await user.click(screen.getByRole('option', { name: 'Second Model' }));
  expect(changed).toHaveBeenLastCalledWith({ model: 'private_route_other', effort: null });
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
