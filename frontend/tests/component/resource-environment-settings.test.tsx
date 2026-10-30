/** Environment details use the native registry editor once, with bounded rows and commands loaded on intent. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { putSetting, targetSettingsKey } from '@/api/settings';
import { UnitConfiguration } from '@/features/resources/UnitConfiguration';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailAgent } from './resource-detail-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ definitions: vi.fn<() => Promise<SettingDef[]>>(), values: vi.fn<() => Promise<ResolvedSetting[]>>(), template: vi.fn(), search: '' }));
vi.mock('@/api/settings', async (load) => { const actual = await load<typeof SettingsApi>(); return { ...actual, putSetting: vi.fn(),
  registryQuery: { queryKey: ['settings-registry'], queryFn: server.definitions, staleTime: Infinity },
  targetSettingsQuery: (target: SettingsApi.SettingsTarget) => ({ queryKey: actual.targetSettingsKey(target), queryFn: server.values, staleTime: Infinity }) }; });
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), templateQuery: (id: string) => ({ queryKey: ['detail-template-commands', id], queryFn: server.template, staleTime: Infinity }) }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/resources/environments/unit', searchStr: server.search }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const target = { kind: 'environment', id: 'unit' } as const;
const place = { name: 'Research workspace', kind: null, owner: detailAgent.owner, access: { permissions: ['environment.manage'] } };
const def = (index: number): SettingDef => ({ key: `env.detail_feature_${index}`, title: `Feature ${index}`, description: `Keep feature ${index} available.`,
  group: 'environment', class: 'parameter', scopes: ['environment'], merge: 'replace', schema: { type: 'integer', minimum: 1, maximum: 100 }, default: 1, example: 2, text_key: null, keywords: [] });
const value = (key: string, current: unknown, writable = true): ResolvedSetting => ({ key, value: current, own: null, source: { scope: 'default', node: null }, locked_by: null, writable, parts: [] });
const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollIntoView');
beforeEach(() => {
  vi.clearAllMocks(); server.search = '?tab=settings';
  const definitions = Array.from({ length: 27 }, (_, index) => def(index + 1));
  server.definitions.mockResolvedValue(definitions); server.values.mockResolvedValue(definitions.map((entry) => value(entry.key, 1)));
  server.template.mockResolvedValue({ spec: { after_up: [{ name: 'warm-cache', command: ['/opt/cache'], timeout_secs: 30 }] } });
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: vi.fn() });
});
afterEach(async () => { cleanup(); if (scroll === undefined) Reflect.deleteProperty(Element.prototype, 'scrollIntoView'); else Object.defineProperty(Element.prototype, 'scrollIntoView', scroll); await setLocale('en'); });
function draw(wantedKey?: string, failureDetail?: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return { ...show(<QueryClientProvider client={client}><UnitConfiguration compact wantedKey={wantedKey} failureDetail={failureDetail} pool="environment" id={target.id} placement="managed" lifecycle="unit" templateId="recipe" place={place} /></QueryClientProvider>), client };
}

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it.each(['en', 'zh-CN'] as const)('shows each setting once, saves through its native guarded row and uses the answer without re-reading (%s)', { timeout: 30_000 }, async (locale) => {
  await setLocale(locale); const user = userEvent.setup();
  const accepted: ResolvedSetting = { ...value(def(1).key, 4), own: { value: 4, locked: false, revision: 1 }, source: { scope: 'environment', node: null } };
  vi.mocked(putSetting).mockResolvedValue(accepted);
  const view = draw(), first = await screen.findByRole('spinbutton', { name: 'Feature 1' });
  expect(screen.getAllByRole('spinbutton')).toHaveLength(12); expect(screen.queryByRole('table')).not.toBeInTheDocument();
  expect(screen.getAllByText('Feature 1', { exact: true }).filter((label) => label.closest('.sr-only') === null)).toHaveLength(1);
  expect(first).toHaveAccessibleName('Feature 1'); expect(server.template).not.toHaveBeenCalled();
  fireEvent.change(first, { target: { value: '4' } }); fireEvent.blur(first);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def(1).key, { value: 4, expectedRevision: null }));
  expect(await screen.findByText(i18n.t('settings.compact.saved'))).toBeVisible();
  expect(view.client.getQueryData<ResolvedSetting[]>(targetSettingsKey(target))?.find((row) => row.key === def(1).key)).toEqual(accepted);
  expect(server.values).toHaveBeenCalledTimes(1); expect(server.definitions).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') })); expect(screen.getByRole('spinbutton', { name: 'Feature 13' })).toBeVisible();
  await user.type(screen.getByRole('searchbox', { name: i18n.t('settings.editor.search') }), 'Feature 27');
  expect(await screen.findByRole('spinbutton', { name: 'Feature 27' })).toBeVisible();
  await waitFor(() => expect(screen.getAllByRole('spinbutton')).toHaveLength(1)); expect(server.values).toHaveBeenCalledTimes(1);
});

it('does not read the command template until Commands is opened', async () => {
  server.values.mockResolvedValue([value('env.after_up', {}, false)]); server.definitions.mockResolvedValue([]);
  draw(); await screen.findByText(i18n.t('settings.editor.none')); expect(server.template).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('link', { name: i18n.t('unitConfiguration.commands') }));
  expect(await screen.findByText('/opt/cache')).toBeVisible(); expect(server.template).toHaveBeenCalledTimes(1); expect(putSetting).not.toHaveBeenCalled();
});

it('moves an exact ordinary setting link to its named native row', async () => {
  server.search = '?tab=settings&setting=env.detail_feature_27';
  draw('env.detail_feature_27'); expect(await screen.findByRole('spinbutton', { name: 'Feature 27' })).toBeVisible();
  await waitFor(() => expect(screen.getAllByRole('spinbutton')).toHaveLength(1)); expect(server.values).toHaveBeenCalledTimes(1); expect(server.template).not.toHaveBeenCalled();
});

it('opens Commands directly for its exact setting link', async () => {
  server.search = '?tab=settings&setting=env.after_up';
  server.values.mockResolvedValue([value('env.after_up', {}, false)]); server.definitions.mockResolvedValue([]);
  draw('env.after_up'); expect(await screen.findByText('/opt/cache')).toBeVisible(); expect(server.template).toHaveBeenCalledTimes(1); expect(putSetting).not.toHaveBeenCalled();
});

it('opens the command editor from its failure recovery without a reload or a stale hidden anchor', async () => {
  server.values.mockResolvedValue([value('env.after_up', {}, false)]); server.definitions.mockResolvedValue([]);
  draw(undefined, 'after-start command warm-cache exited with 7');
  const recovery = await screen.findByRole('link', { name: i18n.t('unitConfiguration.changeCommands') });
  expect(recovery).toHaveAttribute('href', expect.stringContaining('setting=env.after_up'));
  expect(server.template).not.toHaveBeenCalled(); await userEvent.click(recovery);
  expect(await screen.findByText('/opt/cache')).toBeVisible(); expect(server.values).toHaveBeenCalledTimes(1); expect(server.template).toHaveBeenCalledTimes(1);
});
