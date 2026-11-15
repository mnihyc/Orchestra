/** In the settings of a place, the agent features are a named switch for each feature an installed program carries, saved whole by the row's own Save, and starting from what the registry ships. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import type * as SettingsApi from '@/api/settings';
import { putSetting } from '@/api/settings';
import { SettingsEditor } from '@/features/settings/Editor';
import type { AgentDescriptor, HarnessKindInfo, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { agentDescriptor } from '../doubles/agent-descriptor';
import { harnessName } from '../doubles/harness-names';
import { show } from './support';

const server = vi.hoisted(() => ({ definitions: vi.fn<() => Promise<SettingDef[]>>(), values: vi.fn<() => Promise<ResolvedSetting[]>>(), kinds: vi.fn<() => Promise<HarnessKindInfo[]>>() }));
vi.mock('@/api/settings', async (load) => {
  const actual = await load<typeof SettingsApi>();
  return { ...actual, putSetting: vi.fn(), clearSetting: vi.fn(),
    registryQuery: { queryKey: ['settings-registry'], queryFn: server.definitions },
    targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values }),
  };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), harnessKindsQuery: { queryKey: ['agent-features-editor-kinds'], queryFn: server.kinds } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/topic/topic', searchStr: '?tab=settings&section=defaults' }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const target = { kind: 'node', id: 'topic' } as const;
const place = { name: 'login', kind: 'topic', owner: { display_name: 'Ada Lovelace' }, access: { permissions: ['topic.view'] } };
const here = { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'login', redacted: false } } as const;
const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
const features: SettingDef = { key: 'agent.features', title: 'Agent features', description: 'Which optional features of agents are switched on, by feature.', group: 'agent', class: 'policy',
  scopes: ['topic'], merge: 'map', default: { 'codex.cyber_access': true }, example: null, text_key: null, keywords: [],
  schema: { type: 'object', additionalProperties: { type: 'boolean' } } };
const row = (current: unknown, own: ResolvedSetting['own'] = null): ResolvedSetting => ({ key: features.key, value: current, own, source: own === null ? { scope: 'default', node: null } : here, locked_by: null, writable: true, parts: [] });
const codex = (): HarnessKindInfo => {
  const harness = harnessName('codex');
  const parameters = [{ key: 'cyber_access_program', title: 'Access program', description: 'An authorised program for security work, which the vendor grants to the account.', group: 'safety', type: 'enum',
    options: { from_model: 'access_programs' }, default: null, scopes: ['thread'], apply: 'next_turn', feature: 'codex.cyber_access' }];
  return { kind: 'codex', title: harness.name, builds: [], program: null, harness, descriptor: agentDescriptor(harness.name, { kind: 'codex', parameters: parameters as AgentDescriptor['parameters'] }) };
};

function draw() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={cache}><SettingsEditor target={target} scope="topic" place={place} /></QueryClientProvider>);
}

beforeEach(async () => {
  await setLocale('en'); vi.clearAllMocks();
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
  server.definitions.mockResolvedValue([features]); server.values.mockResolvedValue([row({ 'codex.cyber_access': true })]); server.kinds.mockResolvedValue([codex()]);
  vi.mocked(putSetting).mockImplementation((_target, _key, body) => Promise.resolve({ ...row(body.value), own: { value: body.value, locked: false, revision: 1 }, source: here }));
});
afterEach(async () => {
  if (scroll === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView'); else Object.defineProperty(Element.prototype, 'scrollIntoView', scroll);
  await setLocale('en');
});

it('says what is on by the program and the parameter, not by a key', async () => {
  const view = draw();
  const button = await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: features.title }) });
  // The value is the registry's own where no place set one, and says so before it.
  await waitFor(() => expect(button).toHaveAccessibleDescription(/Codex: Access program \(On\)/u));
  expect(view.container).not.toHaveTextContent('codex.cyber_access');
});

it('shows the access program on as shipped, and saves a place switching it off whole, by Save', async () => {
  const user = userEvent.setup();
  draw();
  await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: features.title }) }));
  const access = await screen.findByRole('switch', { name: 'Codex: Access program' });
  expect(access).toBeChecked();
  await user.click(access);
  expect(putSetting).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, features.key, { value: { 'codex.cyber_access': false }, expectedRevision: null }));
});
