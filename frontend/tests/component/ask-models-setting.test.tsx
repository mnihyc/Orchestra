/** Settings use the current native catalogue at system scope and effective inherited identities at an org. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import type * as Resources from '@/api/resources';
import type * as Settings from '@/api/settings';
import type { SettingsTarget, SettingWrite } from '@/api/settings';
import { SettingValue } from '@/features/settings/SettingValue';
import { SettingRow } from '@/features/settings/SettingRow';
import { editableValue } from '@/features/settings/settingParts';
import { shapeOf, withGroup } from '@/features/settings/settingsModel';
import type { Provider, ProviderModel, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ providers: vi.fn<(scope: string, root: string | null) => Promise<Provider[]>>(), models: vi.fn<(id: string) => Promise<ProviderModel[]>>(), detail: vi.fn<(id: string) => Promise<Provider>>(),
  put: vi.fn<(target: SettingsTarget, key: string, write: SettingWrite) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(),
  providersQuery: (scope: string, root: string | null) => ({ queryKey: ['ask-candidate-providers', scope, root], queryFn: () => calls.providers(scope, root) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['ask-candidate-models', id], queryFn: () => calls.models(id) }),
  providerQuery: (id: string) => ({ queryKey: ['ask-candidate-provider', id], queryFn: () => calls.detail(id) }),
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), putSetting: calls.put }));
const provider = planProvider({ id: 'api-provider', name: 'Team API', kind: 'api', protocols: ['openai-responses'] });
const subscription = planProvider({ id: 'subscription-provider' });
const identity = { provider_id: provider.id, model: 'enabled' };
const policySchema = { type: 'array', uniqueItems: true, items: { type: 'object', required: ['provider_id', 'model'],
  additionalProperties: false, properties: { provider_id: { type: 'string' }, model: { type: 'string' } } } };
const policyShape = shapeOf(policySchema);
function model(): ProviderModel {
  const configured = askModels().models[0]?.model;
  if (!configured) throw new Error('The native fixture needs a model.');
  const { allowed: _allowed, reason: _reason, ...value } = configured;
  return { ...value, groups: value.groups ?? [], id: 'enabled-row', model_name: 'enabled', display_name: 'Enabled API model', enabled: true, source: 'manual' };
}
const setting: ResolvedSetting = { key: 'ask.models', value: [identity], own: null, source: { scope: 'system', node: null }, locked_by: null, writable: true,
  parts: [{ source: { scope: 'system', node: null }, value: [identity], locked: false, ignored: false }] };
function OrgDraft() {
  const [value, setValue] = useState(() => editableValue({ key: 'ask.models', merge: 'intersect' }, setting, policyShape));
  return <SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'node', id: 'org' }}
    shape={policyShape} label="Ask models" value={value} onChange={setValue} />;
}
beforeEach(() => {
  calls.providers.mockReset().mockResolvedValue([provider, subscription]); calls.detail.mockReset().mockResolvedValue(provider);
  calls.models.mockReset().mockResolvedValue([model(), { ...model(), id: 'disabled-row', model_name: 'disabled', display_name: 'Disabled API model', enabled: false }]);
  calls.put.mockReset();
});
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`uses the shared ordered picker and enabled API candidates at system scope (${locale})`, async () => {
    await setLocale(locale); const change = vi.fn();
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'system' }} shape={policyShape} label="Ask models" value={[]} onChange={change} />);
    await waitFor(() => expect(calls.models).toHaveBeenCalledWith(provider.id));
    expect(calls.providers).toHaveBeenCalledWith('usable', null); expect(calls.models).not.toHaveBeenCalledWith(subscription.id);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('modelList.add') }));
    const enabled = await screen.findByRole('menuitemradio', { name: 'Enabled API model' });
    expect(screen.queryByRole('menuitemradio', { name: 'Disabled API model' })).not.toBeInTheDocument();
    fireEvent.click(enabled); expect(change).toHaveBeenCalledWith([identity]);
    expect(screen.queryByRole('textbox', { name: /JSON/i })).not.toBeInTheDocument();
  });
  it(`does not load an unrestricted catalogue or add/reorder at an org (${locale})`, async () => {
    await setLocale(locale); const change = vi.fn();
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'node', id: 'org' }} shape={policyShape} label="Ask models" value={[identity, { provider_id: 'outside', model: 'outside' }]} onChange={change} />);
    expect(screen.getByText(i18n.t('ask.modelsFromAbove'))).toBeVisible(); expect(calls.providers).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: i18n.t('modelList.add') })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('modelList.remove', { model: i18n.t('modelList.entry', { number: '1' }) }) }));
    expect(change).toHaveBeenCalledWith([]);
  });
  it(`shows a child removal draft before saving while starting from its inherited effective list (${locale})`, async () => {
    await setLocale(locale); show(<OrgDraft />);
    expect(screen.getByRole('list', { name: 'Ask models' }).children).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('modelList.remove', { model: i18n.t('modelList.entry', { number: '1' }) }) }));
    expect(screen.getByRole('list', { name: 'Ask models' }).children).toHaveLength(0);
    expect(screen.getByText(i18n.t('ask.modelsFromAbove'))).toBeVisible();
  });
  for (const system of [true, false]) it(`opens and saves identities through the actual ${system ? 'system' : 'org'} setting row and its revision guard (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), title = locale === 'en' ? 'Ask models' : '问一问模型列表';
    const second = { provider_id: provider.id, model: 'second' }, initial = [identity, second];
    const target: SettingsTarget = system ? { kind: 'system' } : { kind: 'node', id: 'org' };
    const current: ResolvedSetting = { ...setting, value: initial, own: system ? { value: initial, revision: 3, locked: false } : null,
      parts: setting.parts.map((part) => ({ ...part, value: initial })) };
    calls.models.mockResolvedValue([model(), { ...model(), id: 'second-row', model_name: second.model, display_name: 'Second API model' }]);
    calls.put.mockImplementation((_target, _key, write) => Promise.resolve({ ...current, value: write.value, own: { value: write.value, revision: 4, locked: false } }));
    const def: SettingDef = { key: 'ask.models', title, description: title, class: 'policy', group: 'automation', scopes: ['system', 'org'], merge: 'intersect',
      default: null, example: null, text_key: null, keywords: [], schema: policySchema };
    show(<SettingRow def={withGroup(def)} setting={current} target={target} place={{ name: 'Acme', owner: null, access: { permissions: ['settings.manage'] }, kind: system ? null : 'org' }} />);
    expect(screen.queryByRole('list', { name: title })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: t('settings.compact.editValue', { name: title }) }));
    const list = await screen.findByRole('list', { name: title }); expect(list.children).toHaveLength(2);
    const name = t('modelList.entry', { number: system ? '2' : '1' });
    const action = screen.getByRole('button', { name: t(system ? 'modelList.moveUp' : 'modelList.remove', { model: name }) });
    fireEvent.click(action);
    const submit = system ? action : screen.getByRole('button', { name: t('modelList.remove', { model: t('modelList.entry', { number: '1' }) }) });
    fireEvent.keyDown(submit, { key: 'Enter', ctrlKey: true });
    await waitFor(() => expect(calls.put).toHaveBeenCalledWith(target, 'ask.models', { value: system ? [second, identity] : [second], expectedRevision: system ? 3 : null }));
    await waitFor(() => expect(screen.queryByRole('list', { name: title })).not.toBeInTheDocument());
    if (!system) expect(calls.providers).not.toHaveBeenCalled();
  });
}
