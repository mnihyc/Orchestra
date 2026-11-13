/** The summarizer is a form: a kind, a provider, a model and counts that show their built-in values; what it writes omits what was left at them. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { ApiError } from '@/api/client';
import { putSetting } from '@/api/settings';
import { SettingsEditor } from '@/features/settings/Editor';
import type { Provider, ProviderModel, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const server = vi.hoisted(() => ({
  definitions: vi.fn<() => Promise<SettingDef[]>>(), values: vi.fn<() => Promise<ResolvedSetting[]>>(),
  provider: vi.fn<(id: string) => Promise<unknown>>(), models: vi.fn<(id: string) => Promise<unknown>>(),
}));
vi.mock('@/api/settings', async (load) => {
  const actual = await load<typeof SettingsApi>();
  return { ...actual, putSetting: vi.fn(), clearSetting: vi.fn(),
    registryQuery: { queryKey: ['settings-registry'], queryFn: server.definitions },
    targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values }),
  };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(),
  providerQuery: (id: string) => ({ queryKey: ['summarizer-provider', id], queryFn: () => server.provider(id) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['summarizer-provider-models', id], queryFn: () => server.models(id) }),
}));
// The provider list is the product's own resource picker, read elsewhere; here a choice is one press.
vi.mock('@/features/settings/ProviderChoice', async (load) => ({ ...await load<object>(),
  ProviderDefaultChoice: ({ value, onChange, error, providerKind }: { value: unknown; onChange: (next: unknown) => void; error?: string | undefined; providerKind?: string | undefined }) =>
    <div data-provider-choice={providerKind}><button type="button" onClick={() => onChange('provider-1')}>Choose provider</button><output aria-label="Provider chosen">{typeof value === 'string' ? value : ''}</output>{error !== undefined && <p role="alert">{error}</p>}</div>,
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/topic/topic', searchStr: '?tab=settings&section=defaults' }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const target = { kind: 'node', id: 'topic' } as const;
const place = { name: 'login', kind: 'topic', owner: { display_name: 'Ada Lovelace' }, access: { permissions: ['topic.view'] } };
const here = { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'login', redacted: false } } as const;
const summarizer: SettingDef = { key: 'auto.summarizer', title: 'Summarizer', description: 'Which summarizer is used, and its configuration.', group: 'automation', class: 'parameter',
  scopes: ['topic'], merge: 'replace', default: null, example: null, text_key: null, keywords: [],
  schema: { type: 'object', additionalProperties: true, required: ['kind'], properties: { kind: { type: 'string' } } } };
const row = (current: unknown, own: ResolvedSetting['own'] = null): ResolvedSetting => ({ key: summarizer.key, value: current, own, source: own === null ? { scope: 'default', node: null } : here, locked_by: null, writable: true, parts: [] });
const provider = { id: 'provider-1', name: 'Anthropic', kind: 'api', status: 'enabled', access: { permissions: ['provider.view', 'provider.use'] } } as unknown as Provider;
const models = [{ id: 'm1', model_name: 'small', display_name: 'Small model', enabled: true }, { id: 'm2', model_name: 'large', display_name: 'Large model', enabled: false }] as unknown as ProviderModel[];
const refused = () => new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Forbidden', status: 403, detail: 'Provider access is required.', request_id: 'provider-refused' }, null);
const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');

function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={cache}><SettingsEditor target={target} scope="topic" place={place} /></QueryClientProvider>);
}
const open = () => screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
const save = () => screen.getByRole('button', { name: i18n.t('common.save') });

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
  server.definitions.mockResolvedValue([summarizer]); server.values.mockResolvedValue([row(null)]);
  server.provider.mockResolvedValue(provider); server.models.mockResolvedValue(models);
  vi.mocked(putSetting).mockImplementation((_target, _key, body) => Promise.resolve({ ...row(body.value), own: { value: body.value, locked: false, revision: 1 } }));
});
afterEach(async () => {
  if (scroll === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView'); else Object.defineProperty(Element.prototype, 'scrollIntoView', scroll);
  await setLocale('en');
});

it('shows nothing set as "none here", never as a JSON value', async () => {
  const view = draw();
  const button = await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
  expect(button).toHaveAccessibleDescription(i18n.t('settings.compact.defaultNone'));
  expect(view.container).not.toHaveTextContent('"kind"');
  await userEvent.click(button);
  expect(screen.getByRole('combobox', { name: i18n.t('settingsEditors.summarizer.kind') })).toHaveTextContent(i18n.t('settingsEditors.summarizer.none'));
  expect(screen.getByText(i18n.t('settingsEditors.summarizer.noneHere'))).toBeVisible();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(save()).toBeDisabled();
});

it('writes the kind, the provider and the model, and leaves out what stays at its built-in value', async () => {
  const user = userEvent.setup();
  draw();
  await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
  await user.click(open());
  await user.click(screen.getByRole('combobox', { name: i18n.t('settingsEditors.summarizer.kind') }));
  await user.click(screen.getByRole('option', { name: i18n.t('settingsEditors.summarizer.model') }));
  // The provider picker offers API providers only; the counts show what an empty count means.
  expect(document.querySelector('[data-provider-choice]')).toHaveAttribute('data-provider-choice', 'api');
  expect(screen.getByRole('spinbutton', { name: i18n.t('settingsEditors.summarizer.answerTokens') })).toHaveAttribute('placeholder', '120');
  expect(screen.getByRole('spinbutton', { name: i18n.t('settingsEditors.summarizer.contextTokens') })).toHaveAttribute('placeholder', '32,000');
  expect(save()).toBeDisabled();
  await user.click(screen.getByRole('button', { name: 'Choose provider' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('settingsEditors.summarizer.modelName') }));
  // Only the provider's enabled models are offered.
  expect(await screen.findByRole('option', { name: 'Small model' })).toBeVisible();
  expect(screen.queryByRole('option', { name: 'Large model' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('option', { name: 'Small model' }));
  await waitFor(() => expect(save()).toBeEnabled());
  await user.click(save());
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, summarizer.key, { value: { kind: 'model', provider_id: 'provider-1', model_name: 'small' }, expectedRevision: null }));
});

it('writes a count and an instruction only when they are given, and takes them out again when they are emptied', async () => {
  const user = userEvent.setup();
  server.values.mockResolvedValue([row({ kind: 'model', provider_id: 'provider-1', model_name: 'small' }, { value: { kind: 'model', provider_id: 'provider-1', model_name: 'small' }, locked: false, revision: 4 })]);
  draw();
  await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
  await user.click(open());
  const answer = screen.getByRole('spinbutton', { name: i18n.t('settingsEditors.summarizer.answerTokens') });
  await user.type(answer, '200');
  await user.click(screen.getByText(i18n.t('settingsEditors.summarizer.instructions')));
  fireEvent.change(await screen.findByRole('textbox', { name: i18n.t('settingsEditors.summarizer.fields.title') }), { target: { value: 'Keep titles short.' } });
  await user.click(save());
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, summarizer.key, { value: { kind: 'model', provider_id: 'provider-1', model_name: 'small', max_output_tokens: 200, prompts: { title: 'Keep titles short.' } }, expectedRevision: 4 }));
});

it('says beside its field that a provider or a model cannot be used, and does not offer Save until it can', async () => {
  const user = userEvent.setup();
  server.provider.mockRejectedValue(refused());
  server.models.mockRejectedValue(refused());
  draw();
  await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
  await user.click(open());
  await user.click(screen.getByRole('combobox', { name: i18n.t('settingsEditors.summarizer.kind') }));
  await user.click(screen.getByRole('option', { name: i18n.t('settingsEditors.summarizer.model') }));
  await user.click(screen.getByRole('button', { name: 'Choose provider' }));
  expect(await screen.findByText(i18n.t('settingsEditors.summarizer.cannotUse'))).toBeVisible();
  expect(save()).toBeDisabled();
  expect(putSetting).not.toHaveBeenCalled();
});

it('names a model the provider does not offer', async () => {
  server.values.mockResolvedValue([row({ kind: 'model', provider_id: 'provider-1', model_name: 'gone' }, { value: { kind: 'model', provider_id: 'provider-1', model_name: 'gone' }, locked: false, revision: 2 })]);
  draw();
  await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: summarizer.title }) });
  await userEvent.click(open());
  expect(await screen.findByText(i18n.t('settingsEditors.summarizer.modelNotOffered', { model: 'gone' }))).toBeVisible();
  expect(save()).toBeDisabled();
});
