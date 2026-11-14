/** The model of a place is chosen by name from the models of the provider in force there; a name the provider does not list can still be typed. */
import { QueryClient } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import type * as SettingsApi from '@/api/settings';
import { putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n } from '@/i18n';
import { detailAgent, detailUnitProvider } from './resource-detail-fixtures';
import { AgentModelChoice } from '@/features/settings/AgentModelChoice';
import type { ProviderModel, ResolvedSetting } from '@/generated/api-types';
import { show } from './support';

const state = vi.hoisted(() => ({ rows: [] as { key: string; value: unknown }[], models: [] as unknown[], descriptor: {} as unknown }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), putSetting: vi.fn(), targetSettingsQuery: () => ({ queryKey: ['agent-model-place'], queryFn: () => Promise.resolve(state.rows) }) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), providerQuery: (id: string) => ({ queryKey: ['parameter-provider', id], queryFn: () => Promise.resolve({ ...detailUnitProvider, id, agent: null, kind: 'api', protocols: ['openai-responses'] }) }), agentQuery: (id: string) => ({ queryKey: ['parameter-agent', id], queryFn: () => Promise.resolve({ ...detailAgent, id, descriptor: state.descriptor }) }), providerModelsQuery: (id: string) => ({ queryKey: ['agent-model-models', id], queryFn: () => Promise.resolve(state.models) }) }));

const target = { kind: 'node', id: 'org' } as const;
const models = [{ id: 'm1', model_name: 'fast-1', display_name: 'Fast model', enabled: true }, { id: 'm2', model_name: 'slow-1', display_name: 'Slow model', enabled: false }] as unknown as ProviderModel[];

function draw(value: string, changed = vi.fn()) {
  show(<AgentModelChoice shape={{ type: 'text', maxLength: null, pattern: null }} label="Model" value={value} target={target} onChange={changed} />);
  return changed;
}

beforeEach(() => {
  vi.clearAllMocks();
  state.descriptor = {};
  state.rows = [{ key: 'agent.default_provider_id', value: 'provider-1' }];
  state.models = models;
});

it('offers the enabled models of the provider in force, by name, and writes the one chosen', async () => {
  const user = userEvent.setup();
  const changed = draw('');
  await user.click(screen.getByRole('combobox', { name: 'Model' }));
  expect(await screen.findByRole('option', { name: 'Fast model' })).toBeVisible();
  expect(screen.queryByRole('option', { name: 'Slow model' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('option', { name: 'Fast model' }));
  expect(changed).toHaveBeenCalledWith('fast-1');
});

it('keeps a name the provider does not list possible, by typing it', async () => {
  const user = userEvent.setup();
  const changed = draw('');
  await user.click(screen.getByRole('combobox', { name: 'Model' }));
  await user.type(screen.getByRole('combobox', { name: 'Model' }), 'brand-new');
  await user.click(await screen.findByRole('option', { name: 'Use “brand-new”' }));
  expect(changed).toHaveBeenCalledWith('brand-new');
});

it('says beside the field that the provider does not offer the model that is saved', async () => {
  draw('gone');
  expect(await screen.findByText('This provider does not offer gone.')).toBeVisible();
});

it('takes a typed name where no provider is in force, and asks for no identifier', async () => {
  const user = userEvent.setup();
  state.rows = [{ key: 'agent.default_provider_id', value: null }];
  const changed = draw('');
  const field = screen.getByRole('combobox', { name: 'Model' });
  expect(field).toBeEnabled();
  expect(field).toHaveAttribute('placeholder', 'Type a model name');
  await user.type(field, 'some-model');
  await user.click(await screen.findByRole('option', { name: 'Use “some-model”' }));
  expect(changed).toHaveBeenCalledWith('some-model');
});


const parameters = [
  { key: 'thinking', title: 'Thinking effort', description: 'How hard the model thinks.', type: 'enum', options: { from_model: 'efforts' }, default: { from_model: 'default_effort' }, scopes: ['org'], apply: 'next_turn' },
  { key: 'service_tier', title: 'Speed', description: '', type: 'enum', options: { from_model: 'service_tiers' }, default: { from_model: 'default_service_tier' }, scopes: ['org'], apply: 'next_turn', eligible_when: { model_has: 'service_tiers' } },
  { key: 'context', title: 'Context', description: '', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } }, default: { from_model: 'default_context' }, scopes: ['org'], apply: 'new_session', eligible_when: { model_has: 'context_max' } },
  { key: 'quiet', title: 'Quiet output', description: '', type: 'boolean', default: false, scopes: ['org'], apply: 'next_turn' },
];
const model: ProviderModel = { ...models[0]!, groups: [], source: 'manual', token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false },
  parameters: { efforts: ['low', 'high'], default_effort: 'low', service_tiers: [{ id: 'priority', label: 'Fast' }], context_window: 128_000, context_max: 256_000,
    offered: { service_tiers: [{ id: 'unopened', label: 'Unopened speed' }] },
    request_options: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } } },
  parameter_forms: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } },
  option_constraints: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } },
};

function parameterDraft(initial: Record<string, unknown> = {}) {
  state.descriptor = { parameters };
  state.models = [model];
  state.rows = [{ key: 'agent.default_provider_id', value: 'provider-1' }, { key: 'agent.default_agent_id', value: 'profile-1' }, { key: 'agent.model', value: model.model_name }];
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === 'agent.params');
  if (def === undefined) throw new Error('The registry must declare agent parameters.');
  const setting: ResolvedSetting = { key: def.key, value: initial, own: { value: initial, locked: false, revision: 4 }, source: { scope: 'org', node: null },
    writable: true, locked_by: null, parts: [] };
  vi.mocked(putSetting).mockImplementation((_target, _key, change) => Promise.resolve({ ...setting, value: change.value, own: { value: change.value, locked: false, revision: 5 } }));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  const view = show(<ul><SettingRow def={def} setting={setting} target={target} place={{ name: 'Acme', kind: 'org', owner: null, access: null }} /></ul>, cache);
  return { ...view, cache, def };
}

it("edits a place model’s declared effort speed context and options as a draft in the composer controls", async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({ quiet: true, retained: 'keep this value' });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  const effort = await screen.findByRole('combobox', { name: 'Thinking effort' });
  await user.click(effort);
  expect(screen.queryByRole('option', { name: 'max' })).not.toBeInTheDocument();
  await user.click(await screen.findByRole('option', { name: /High/ }));
  const speed = screen.getByRole('radiogroup', { name: i18n.t('composer.model.speed.title') });
  await user.click(within(speed).getByRole('radio', { name: /Fast/ }));
  expect(screen.queryByText('Unopened speed')).not.toBeInTheDocument();
  expect(screen.getByRole('radiogroup', { name: i18n.t('composer.model.context.title') })).toBeVisible();
  expect(screen.getByRole('switch', { name: 'Quiet output' })).toBeChecked();
  await user.click(screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary' }));
  fireEvent.change(await screen.findByRole('spinbutton', { name: 'Temperature' }), { target: { value: '0.5' } });
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { quiet: true, retained: 'keep this value', thinking: 'high', service_tier: 'priority',
    'provider_options:openai-responses': { temperature: 0.5 } }, expectedRevision: 4 }));
});

it('keeps a parameter draft and removes unsupported controls when the chosen model changes', async () => {
  const user = userEvent.setup();
  const { def, cache } = parameterDraft({ retained: 'keep this value' });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  await user.click(await screen.findByRole('switch', { name: 'Quiet output' }));
  state.rows = state.rows.map((row) => row.key === 'agent.model' ? { ...row, value: 'plain' } : row);
  act(() => {
    cache.setQueryData(['agent-model-models', 'provider-1'], [model, { ...model, id: 'plain', model_name: 'plain', parameters: {}, option_constraints: {}, parameter_forms: {} }]);
    cache.setQueryData(['agent-model-place'], state.rows);
  });
  await waitFor(() => expect(screen.queryByRole('radiogroup', { name: i18n.t('composer.model.speed.title') })).not.toBeInTheDocument());
  expect(screen.queryByRole('combobox', { name: 'Thinking effort' })).not.toBeInTheDocument();
  expect(screen.getByRole('switch', { name: 'Quiet output' })).toBeChecked();
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { retained: 'keep this value', quiet: true }, expectedRevision: 4 }));
});


it('chooses the separate effort default from the current model and writes only that revisioned setting', async () => {
  const user = userEvent.setup();
  state.rows = [{ key: 'agent.default_provider_id', value: 'provider-1' }, { key: 'agent.model', value: model.model_name }];
  state.models = [model];
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === 'agent.effort');
  if (def === undefined) throw new Error('The registry must declare reasoning effort.');
  const setting: ResolvedSetting = { key: def.key, value: 'low', own: null, source: { scope: 'default', node: null }, writable: true, locked_by: null, parts: [] };
  vi.mocked(putSetting).mockResolvedValue({ ...setting, value: 'high', own: { value: 'high', locked: false, revision: 1 } });
  show(<ul><SettingRow def={def} setting={setting} target={target} place={{ name: 'Acme', kind: 'org', owner: null, access: null }} /></ul>);
  const input = await screen.findByRole('combobox', { name: def.title });
  await waitFor(() => expect(input).toBeEnabled());
  await user.click(input);
  await user.click(await screen.findByRole('option', { name: /High/ }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 'high', expectedRevision: null }));
});
