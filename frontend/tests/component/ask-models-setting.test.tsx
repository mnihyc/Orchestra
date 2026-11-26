/** Settings use the current native catalogue at system scope and effective inherited identities at an org. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { ApiError } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import type * as Settings from '@/api/settings';
import type { SettingsTarget, SettingWrite } from '@/api/settings';
import { SettingValue } from '@/features/settings/SettingValue';
import { SettingRow } from '@/features/settings/SettingRow';
import { editableValue } from '@/features/settings/settingParts';
import { shapeOf, withGroup } from '@/features/settings/settingsModel';
import type { Provider, ProviderModel, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiAskModel, askModels } from '../unit/ask-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ providers: vi.fn<(scope: string, root: string | null) => Promise<Provider[]>>(), models: vi.fn<(id: string) => Promise<ProviderModel[]>>(), detail: vi.fn<(id: string) => Promise<Provider>>(),
  put: vi.fn<(target: SettingsTarget, key: string, write: SettingWrite) => Promise<ResolvedSetting>>() }));
vi.mock('@/api/capabilityBackends', async (load) => {
  const choices = async (id: string, offering?: string) => ({ items: (await calls.models(id)).filter((row) => offering === undefined || row.model_name === offering).map((row) => ({
    backend: { route: id === subscription.id ? 'ask.delegation' : 'provider.generate', offering: { provider: id, offering: row.model_name }, config: null },
    offering_name: row.display_name, route_text: { en: 'Model', 'zh-CN': '模型' }, program_text: {},
    eligible: row.enabled, reason: row.enabled ? null : 'model_unavailable', reason_text: {},
  })), next_cursor: null });
  return { ...await load<object>(),
    capabilityProviderQuery: (reader: string, id: string) => ({ queryKey: ['ask-policy-account', reader, id], queryFn: () => calls.detail(id), retry: false }),
    selectedCapabilityBackendQuery: (reader: string, id: string, place: string, capability: string, offering: string) => ({
      queryKey: ['ask-policy-selected', reader, id, place, capability, offering], queryFn: () => choices(id, offering), retry: false }),
    capabilityBackendsQuery: (reader: string, id: string, place: string, capability: string, q: string) => ({
      queryKey: ['ask-policy-offers',reader,id,place,capability,q], initialPageParam: undefined, queryFn: () => choices(id), getNextPageParam: () => undefined, retry: false }),
  };
});
vi.mock('@/features/settings/ProviderChoice', async (load) => ({ ...await load<object>(),
  ProviderDefaultChoice: ({ label, onChange, disabled }: { label: string; onChange: (value: unknown) => void; disabled: boolean }) =>
    <button type="button" disabled={disabled} aria-label={label} onClick={() => onChange(provider.id)}>Choose account</button>,
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), putSetting: calls.put }));
const provider = planProvider({ id: 'api-provider', name: 'Team API', kind: 'api', protocols: ['openai-responses'] });
const subscription = planProvider({ id: 'subscription-provider' });
const identity = apiAskModel(provider.id, 'enabled');
const policySchema = { type: 'array', uniqueItems: true, items: { type: 'object', required: ['backend'],
  additionalProperties: false, properties: { backend: { type: 'object' } } } };
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
  it.each(['provider', 'models'] as const)(`keeps inherited identities and retries a failed %s read at an org (${locale})`, async (failed) => {
    await setLocale(locale); const change = vi.fn();
    if (failed === 'provider') calls.detail.mockRejectedValueOnce(new NetworkError('offline')).mockResolvedValue(provider);
    else calls.models.mockRejectedValueOnce(new NetworkError('offline')).mockResolvedValue([model()]);
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'node', id: 'org' }} shape={policyShape} label="Ask models" value={[identity]} onChange={change} />);
    await screen.findByRole('alert');
    expect(screen.getByRole('list', { name: 'Ask models' }).children).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(screen.getByRole('list', { name: 'Ask models' }).children).toHaveLength(1);
    expect(change).not.toHaveBeenCalled();
    expect(calls.providers).not.toHaveBeenCalled();
  });
  it(`keeps private policy resource details out of the ordinary failure display (${locale})`, async () => {
    await setLocale(locale);
    calls.detail.mockRejectedValue(new ApiError({ type: '', title: 'Denied', code: 'authz.denied', status: 403, detail: 'Hidden provider name', request_id: 'policy-details' }, null));
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'node', id: 'org' }} shape={policyShape} label="Ask models" value={[identity]} onChange={vi.fn()} />);
    expect(await screen.findByText(i18n.t('settings.compact.managed'))).toBeVisible();
    expect(screen.queryByText('Hidden provider name')).not.toBeInTheDocument();
    expect(screen.getByRole('list', { name: 'Ask models' }).children).toHaveLength(1);
  });
  it(`uses the shared ordered picker and enabled API candidates at system scope (${locale})`, async () => {
    await setLocale(locale); const change = vi.fn();
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'system' }} shape={policyShape} label="Ask models" value={[]} onChange={change} />);
    expect(calls.models).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: i18n.t('modelList.add') }));
    fireEvent.click(screen.getByRole('button', { name: i18n.t('capabilityBackend.provider') }));
    fireEvent.click(screen.getByRole('combobox', { name: i18n.t('capabilityBackend.route') }));
    const enabled = await screen.findByRole('option', { name: /Enabled API model/ });
    expect(screen.getByRole('option', {name:/Disabled API model/})).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(enabled); expect(change).toHaveBeenCalledWith([identity]);
    expect(screen.queryByRole('textbox', { name: /JSON/i })).not.toBeInTheDocument();
  });
  it(`does not load an unrestricted catalogue or add/reorder at an org (${locale})`, async () => {
    await setLocale(locale); const change = vi.fn();
    show(<SettingValue def={{ key: 'ask.models', merge: 'intersect' }} setting={setting} target={{ kind: 'node', id: 'org' }} shape={policyShape} label="Ask models" value={[identity, apiAskModel('outside', 'outside')]} onChange={change} />);
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
    const second = apiAskModel(provider.id, 'second'), initial = [identity, second];
    const target: SettingsTarget = system ? { kind: 'system' } : { kind: 'node', id: 'org' };
    const current: ResolvedSetting = { ...setting, value: initial, own: system ? { value: initial, revision: 3, locked: false } : null,
      parts: setting.parts.map((part) => ({ ...part, value: initial })) };
    calls.models.mockResolvedValue([model(), { ...model(), id: 'second-row', model_name: second.backend.offering.offering, display_name: 'Second API model' }]);
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
