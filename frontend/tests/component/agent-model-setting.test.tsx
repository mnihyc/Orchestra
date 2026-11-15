/** The model of a place is chosen by name from the models of the provider in force there; a name the provider does not list can still be typed. */
import { Activity } from 'react';
import { QueryClient } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import type * as SettingsApi from '@/api/settings';
import { putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n } from '@/i18n';
import { record } from '@/features/resources/UnitCommands';
import { detailAgent, detailUnitProvider } from './resource-detail-fixtures';
import { AgentModelChoice } from '@/features/settings/AgentModelChoice';
import type { ProviderModel, ResolvedSetting, SettingParameterFormBody } from '@/generated/api-types';
import { show } from './support';

const state = vi.hoisted((): { rows: { key: string; value: unknown }[]; models: unknown[]; descriptor: unknown } => ({ rows: [], models: [], descriptor: {} }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), putSetting: vi.fn(), parameterFormQuery: (body: SettingParameterFormBody) => ({ queryKey: ['native-default-form', body], queryFn: () => Promise.resolve(nativeFormReply(body)) }), targetSettingsQuery: () => ({ queryKey: ['agent-model-place'], queryFn: () => Promise.resolve(state.rows) }) }));
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
  { key: 'thinking', title: 'Thinking effort', description: 'How hard the model thinks.', group: 'model', type: 'enum', options: { from_model: 'efforts' }, default: { from_model: 'default_effort' }, scopes: ['org'], apply: 'next_turn' },
  { key: 'service_tier', title: 'Speed', description: '', group: 'model', type: 'enum', options: { from_model: 'service_tiers' }, default: { from_model: 'default_service_tier' }, scopes: ['org'], apply: 'next_turn', eligible_when: { model_has: 'service_tiers' } },
  { key: 'context', title: 'Context', description: '', group: 'model', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } }, default: { from_model: 'default_context' }, scopes: ['org'], apply: 'new_session', eligible_when: { model_has: 'context_max' } },
  { key: 'quiet', title: 'Quiet output', description: '', group: 'behaviour', type: 'boolean', default: false, scopes: ['org'], apply: 'next_turn' },
];
const model: ProviderModel = { ...models[0]!, groups: [], source: 'manual', token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 300, other_x100: 100, image_tokens: 1000, measured: false },
  parameters: { efforts: ['low', 'high'], default_effort: 'low', service_tiers: [{ id: 'priority', label: 'Fast' }], context_window: 128_000, context_max: 256_000,
    offered: { service_tiers: [{ id: 'unopened', label: 'Unopened speed' }] },
    request_options: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } } },
  parameter_forms: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } },
  option_constraints: { 'openai-responses': { type: 'object', additionalProperties: false, properties: { temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 } } } },
};

/** Served native projections; the API settings program proves their constraint and feature resolution. */
function nativeFormReply(body: SettingParameterFormBody): unknown[] {
  const quiet: Record<string, unknown> = { ...parameters[3], resolved_default: false };
  if ((body.model.efforts?.length ?? 0) === 0) return [quiet];
  const rows: Record<string, unknown>[] = [
    { ...parameters[0], resolved_options: [{ value: 'low', label: 'low' }, { value: 'high', label: 'high' }], resolved_default: 'low' },
    { ...parameters[1], resolved_options: [{ value: 'priority', label: 'Fast' }], resolved_default: null, allows_none: true },
    { ...parameters[2], resolved_default: 256_000, resolved_range: { min: 32_000, max: 256_000 } },
    quiet,
  ];
  const constraints = record(record(body.model.request_options?.native).properties);
  return rows.map((row) => {
    const key = row.key;
    const rule = typeof key === 'string' ? record(constraints[key]) : {};
    return Object.keys(rule).length === 0 ? row : { ...row, provider_constraint: rule, allows_none: false,
      ...(Object.hasOwn(rule, 'const') ? { resolved_default: rule.const } : {}) };
  });
}

function parameterDraft(initial: Record<string, unknown> = {}, inherited: Record<string, unknown> = {}, entries: ProviderModel[] = [model], following = false) {
  state.descriptor = { parameters };
  state.models = entries;
  state.rows = [{ key: 'agent.default_provider_id', value: 'provider-1' }, { key: 'agent.default_agent_id', value: 'profile-1' }, { key: 'agent.model', value: model.model_name }];
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === 'agent.params');
  if (def === undefined) throw new Error('The registry must declare agent parameters.');
  const setting: ResolvedSetting = { key: def.key, value: { ...inherited, ...initial }, own: { value: initial, locked: false, revision: 4 }, source: { scope: 'org', node: null },
    writable: true, locked_by: null, parts: [{ value: inherited, source: { scope: 'org', node: { id: 'parent', kind: 'org', name: 'Parent', redacted: false } }, ignored: false, locked: false }] };
  if (following) {
    setting.own = null;
    setting.inherited = { value: inherited, source: setting.parts[0]!.source };
  }
  vi.mocked(putSetting).mockImplementation((_target, _key, change) => Promise.resolve({ ...setting, value: change.value, own: { value: change.value, locked: false, revision: 5 } }));
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  const editor = <ul><SettingRow def={withGroup(def)} setting={setting} target={target} place={{ name: 'Acme', kind: 'org', owner: null, access: null }} /></ul>;
  const view = show(<Activity mode="visible">{editor}</Activity>, cache);
  return { ...view, cache, def, hide: (hidden: boolean) => view.rerender(<Activity mode={hidden ? 'hidden' : 'visible'}>{editor}</Activity>) };
}

it("edits a place model’s declared effort speed context and options as a draft in the composer controls", async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({ quiet: true, retained: 'keep this value' });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  const effort = await screen.findByRole('combobox', { name: 'Thinking effort' });
  await user.click(effort);
  expect(screen.queryByRole('option', { name: 'max' })).not.toBeInTheDocument();
  await user.click(await screen.findByRole('option', { name: /High/ }));
  const speed = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: i18n.t('composer.model.speed.standard') }) });
  await user.click(speed);
  expect(screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: 'Fast' }) })).toHaveAttribute('aria-pressed', 'true');
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
  await waitFor(() => expect(screen.queryByRole('button', { name: /^Speed:/u })).not.toBeInTheDocument());
  expect(screen.queryByRole('combobox', { name: 'Thinking effort' })).not.toBeInTheDocument();
  expect(await screen.findByRole('switch', { name: 'Quiet output' })).toBeChecked();
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
  show(<ul><SettingRow def={withGroup(def)} setting={setting} target={target} place={{ name: 'Acme', kind: 'org', owner: null, access: null }} /></ul>);
  const input = await screen.findByRole('combobox', { name: def.title });
  await waitFor(() => expect(input).toBeEnabled());
  await user.click(input);
  await user.click(await screen.findByRole('option', { name: /High/ }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 'high', expectedRevision: null }));
});


it('lets Standard override an inherited fast speed without copying other inherited parameters', async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({}, { service_tier: 'priority', quiet: true });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  const speed = await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: 'Fast' }) });
  expect(speed).toHaveAttribute('aria-pressed', 'true');
  await user.click(speed);
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { service_tier: null }, expectedRevision: 4 }));
});

it('starts the shared parameter draft from inheritance while saving only the chosen override', async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({}, { service_tier: 'priority', quiet: true }, [model], true);
  expect(screen.getByRole('radio', { name: 'As Parent decides' })).toBeChecked();
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  const speed = await screen.findByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: 'Fast' }) });
  expect(speed).toHaveAttribute('aria-pressed', 'true');
  expect(speed).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('radio', { name: 'Here' }));
  const ownSpeed = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: 'Fast' }) });
  expect(ownSpeed).toHaveAttribute('aria-pressed', 'true');
  await user.click(ownSpeed);
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { service_tier: null }, expectedRevision: null }));
});


it('shows provider-fixed native parameters without writing them into the place draft', async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({}, {}, [{ ...model, option_constraints: {
    native: { type: 'object', properties: { quiet: { type: 'boolean', const: true, 'x-orch-source': 'provider' }, service_tier: { type: 'string', const: 'priority' } } },
  } }]);
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  const fixed = await screen.findByRole('textbox', { name: 'Quiet output' });
  expect(fixed).toBeDisabled();
  expect(fixed).toHaveValue('true');
  expect(screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.model.speed.title'), value: 'Fast' }) })).toBeDisabled();
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  expect(putSetting).not.toHaveBeenCalled();
});

it('can clear an inherited request option without restoring the ancestor entry', async () => {
  const user = userEvent.setup();
  const { def } = parameterDraft({}, { 'provider_options:openai-responses': { temperature: 0.7 } });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  await screen.findByRole('combobox', { name: 'Thinking effort' });
  await user.click(screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary' }));
  expect(await screen.findByRole('spinbutton', { name: 'Temperature' })).toHaveValue(0.7);
  await user.click(screen.getByRole('button', { name: i18n.t('templates.editor.resetField', { name: 'Temperature' }) }));
  expect(screen.getByRole('spinbutton', { name: 'Temperature' })).toHaveValue(null);
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { 'provider_options:openai-responses': {} }, expectedRevision: 4 }));
});

it.each(['a hidden view', 'local filtering'] as const)('retains an invalid declared request-option draft through %s until it is corrected', async (interruption) => {
  const user = userEvent.setup();
  const form = { type: 'object', additionalProperties: false, properties: { vendor_options: { type: 'object', additionalProperties: true, title: 'Vendor options' } } };
  const retained = Object.fromEntries(Array.from({ length: 12 }, (_, at) => [`retained-${at}`, true]));
  const { def, hide } = parameterDraft(retained, {}, [{ ...model, parameters: { ...model.parameters, request_options: { 'openai-responses': form } }, parameter_forms: { 'openai-responses': form }, option_constraints: { 'openai-responses': form } }]);
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  await screen.findByRole('combobox', { name: 'Thinking effort' });
  await user.click(screen.getByText(i18n.t('providerOptions.title'), { selector: 'summary' }));
  const field = await screen.findByRole('textbox', { name: i18n.t('templates.editor.other') });
  fireEvent.change(field, { target: { value: '{' } });
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  if (interruption === 'a hidden view') { hide(true); hide(false); }
  else {
    const search = screen.getByRole('searchbox', { name: i18n.t('settings.editor.search') });
    await user.type(search, 'Quiet');
    expect(screen.queryByRole('textbox', { name: i18n.t('templates.editor.other') })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
    await user.clear(search);
  }
  expect(await screen.findByRole('textbox', { name: i18n.t('templates.editor.other') })).toHaveValue('{');
  expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  expect(putSetting).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('templates.editor.other') }), { target: { value: '{"vendor_option":true}' } });
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: { ...retained, 'provider_options:openai-responses': { vendor_options: { vendor_option: true } } }, expectedRevision: 4 }));
});
