/** The same real picker uses declared current candidates for every capability, in both languages. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import type * as Client from '@/api/client';
import { ApiError } from '@/api/client';
import type { CapabilityBackendCandidate } from '@/generated/api-types';
import { CapabilityBackendChoice, CapabilityBackendSummary } from '@/features/settings/CapabilityBackendChoice';
import { changeBackend, selectedBackend } from '@/features/settings/capabilityBackendValue';
import { i18n, setLocale } from '@/i18n';
import { id, me, userRef } from '../doubles/fixtures';
import { inventoryRow } from '../doubles/everything-row';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

interface Query { capability?: string; place?: string; offering?: string; cursor?: string; q?: string }
interface Options { params?: { path?: { id?: string }; query?: Query } }
const calls = vi.hoisted(() => ({ choices: vi.fn<(query: Query) => Promise<{ items: CapabilityBackendCandidate[]; next_cursor: string | null }>>(), changed: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string, options?: Options) => {
    if (path === '/everything') return { data: { items: [provider].map((provider) => inventoryRow({ id: provider.id, name: provider.name, kind: 'provider', parent_id: 'resources', resource: { kind: 'provider', value: provider } })), next_cursor: null }, response: new Response() };
    if (path === '/providers/{id}') return { data: provider, response: new Response() };
    if (path === '/providers/{id}/capability-backends') return { data: await calls.choices(options?.params?.query ?? {}), response: new Response() };
    throw new Error(`Unexpected GET ${path}`);
  } } };
});
vi.mock('@/features/navigation/placeLabels', async (loadOriginal) => ({ ...await loadOriginal<object>(), useKnownPlaces: () => new Map() }));
vi.mock('@/features/resources/ResourceCreation', async (loadOriginal) => ({ ...await loadOriginal<object>(), ResourceCreateButton: () => null }));

const HOME = id(93, 11), PROVIDER = id(93, 12);
const provider = planProvider({ id: PROVIDER, name: 'Picture plan', owner: userRef(me(1, 'ada', 'Ada')), org_id: HOME,
  home: { id: HOME, kind: 'org', name: 'Acme', redacted: false } });
const key = 'agents.image_generation_backend';
const backend = { route: 'image_generation.delegation', config: null, offering: { provider: PROVIDER, offering: 'native-model' } };
function candidate(eligible: boolean, offering = 'native-model'): CapabilityBackendCandidate {
  return { backend: { ...backend, offering: { ...backend.offering, offering } }, offering_name: offering === 'native-model' ? 'Image model' : offering,
    route_text: { en: 'Delegated session', 'zh-CN': '委派会话' }, program_text: { en: 'Codex on ChatGPT', 'zh-CN': 'Codex（ChatGPT 登录）' }, eligible,
    reason: eligible ? null : 'fixture-free', reason_text: eligible ? {} : { en: 'Image generation is not offered on Free', 'zh-CN': 'Free 套餐不提供图像生成' } };
}
function Draw({ initial = null }: { initial?: unknown }) {
  const [value, setValue] = useState(initial);
  return <CapabilityBackendChoice settingKey={key} target={{ kind: 'node', id: HOME }} shape={{ type: 'json' }} value={value} label="Images" onChange={(next) => { calls.changed(next); setValue(next); }} />;
}
beforeEach(() => {
  vi.clearAllMocks();
  calls.choices.mockImplementation((query) => Promise.resolve({ items: query.offering === undefined ? [candidate(false, 'Free model'), candidate(true)] : [candidate(true)], next_cursor: null }));
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each([
  ['en', ['Web search provider', 'Web search route', 'Page fetch provider', 'Page fetch route']],
  ['zh-CN', ['网页搜索提供方', '网页搜索路径', '网页抓取提供方', '网页抓取路径']],
] as const)('names both pairs in the shared web backend form in %s', async (locale, names) => {
  await setLocale(locale);
  show(<form><CapabilityBackendChoice settingKey="agents.web_search_backend" target={{ kind: 'node', id: HOME }}
    shape={{ type: 'json' }} value={null} label="Web search backend" onChange={calls.changed} /></form>);
  expect(screen.getAllByRole('combobox')).toHaveLength(names.length);
  for (const name of names) expect(screen.getByRole('combobox', { name })).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('saves only the complete eligible route and exposes the disabled reason in %s', async (locale) => {
  await setLocale(locale);
  show(<Draw />);
  const user = userEvent.setup();
  const route = screen.getByRole('combobox', { name: i18n.t('capabilityBackend.route') });
  expect(route).toBeDisabled();
  await user.click(screen.getByRole('combobox', { name: i18n.t('capabilityBackend.provider') }));
  await user.click(await screen.findByRole('option', { name: 'Picture plan' }));
  expect(calls.changed).not.toHaveBeenCalled();
  await waitFor(() => expect(screen.queryByRole('dialog', { name: i18n.t('capabilityBackend.provider') })).not.toBeInTheDocument());
  expect(route).toBeEnabled();
  await user.click(route);
  await waitFor(() => expect(route).toHaveAttribute('aria-expanded', 'true'));
  const program = locale === 'en' ? 'Codex on ChatGPT' : 'Codex（ChatGPT 登录）';
  const name = i18n.t('capabilityBackend.delegated', { program });
  await screen.findByRole('option', { name: /Free model/ });
  const options = screen.getAllByRole('option');
  const refused = options.find((option) => option.textContent.includes('Free model'));
  expect(refused).toBeDefined();
  expect(refused).toHaveAttribute('aria-disabled', 'true');
  expect(within(refused!).getByText(locale === 'en' ? 'Image generation is not offered on Free' : 'Free 套餐不提供图像生成')).toBeVisible();
  const enabled = options.find((option) => option.textContent.includes('Image model'));
  expect(enabled).toHaveTextContent(name);
  await user.click(enabled!);
  await waitFor(() => expect(calls.changed).toHaveBeenCalledExactlyOnceWith(backend));
  await waitFor(() => expect(route).toHaveValue(`${name} · Image model`));
});

it.each(['en', 'zh-CN'] as const)('keeps paging and retry in the real route control in %s', async (locale) => {
  await setLocale(locale);
  calls.choices.mockImplementation((query) => {
    if (query.offering !== undefined) return Promise.resolve({ items: [candidate(true)], next_cursor: null });
    if (query.cursor !== undefined) return Promise.resolve({ items: [candidate(true, 'Late model')], next_cursor: null });
    return Promise.resolve({ items: [candidate(false, 'Free model')], next_cursor: 'native-page-2' });
  });
  show(<Draw initial={backend} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: i18n.t('capabilityBackend.route') }));
  const more = await screen.findByRole('button', { name: i18n.t('ui.pagination.more') });
  await user.click(more);
  const late = await screen.findByRole('option', { name: /Late model/ });
  await user.click(late);
  expect(calls.changed).toHaveBeenCalledExactlyOnceWith({ ...backend, offering: { ...backend.offering, offering: 'Late model' } });
  expect(calls.choices).toHaveBeenCalledWith(expect.objectContaining({ place: HOME, capability: 'image_generation', cursor: 'native-page-2' }));
});

it.each(['en', 'zh-CN'] as const)('hides a refused current name and never reuses the chosen model as eligibility in %s', async (locale) => {
  await setLocale(locale);
  calls.choices.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' }, null));
  const view = show(<CapabilityBackendSummary settingKey={key} value={backend} target={{ kind: 'node', id: HOME }} />);
  expect(await screen.findByText(i18n.t('settings.compact.managed'))).toBeVisible();
  expect(view.container).not.toHaveTextContent('Image model');
  expect(view.container).not.toHaveTextContent(PROVIDER);
  expect(calls.changed).not.toHaveBeenCalled();
});

it('preserves the other capability and opaque members in a shared backend value', () => {
  const old = { kind: 'api', provider_id: PROVIDER, search_service: 'search', fetch_service: 'fetch' };
  const next = changeBackend(old, 'agents.web_search_backend', 'web_search', { route: 'web_search.service-model', config: null, offering: { provider: PROVIDER, offering: 'search-new' } });
  expect(selectedBackend(next, 'page_fetch')?.offering?.offering).toBe('fetch');
  expect(selectedBackend(next, 'web_search')?.offering?.offering).toBe('search-new');
  const removed = changeBackend(next, 'agents.web_search_backend', 'page_fetch', null);
  expect(selectedBackend(removed, 'page_fetch')).toBeNull();
  expect(selectedBackend(removed, 'web_search')?.offering?.offering).toBe('search-new');
});
