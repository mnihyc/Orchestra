/** The project setting uses provider names and keeps native revision, lock, failure and Undo semantics. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { clearSetting, putSetting, type SettingsTarget } from '@/api/settings';
import type * as Client from '@/api/client';
import { SettingRow } from '@/features/settings/SettingRow';
import { SettingSummary } from '@/features/settings/SettingSummary';
import type { KeyDef } from '@/features/settings/settingsModel';
import type { Provider, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { id, me, org, userRef } from '../doubles/fixtures';
import { show } from './support';
import { planProvider } from '../unit/provider-plan-fixtures';
import { inventoryRow } from '../doubles/everything-row';

interface ReadOptions { params?: { path?: { id?: string } } }
const calls = vi.hoisted(() => ({
  list: vi.fn<() => Promise<Provider[]>>(), read: vi.fn<(id: string) => Promise<Provider>>(), create: vi.fn(),
  created: null as Provider | null,
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  providerQuery: (id: string) => ({ queryKey: ['default-provider', id], staleTime: Infinity, queryFn: () => calls.read(id) }),
}));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string, options?: ReadOptions) => {
    if (path === '/everything') {
      const providers = await calls.list();
      return { data: { items: providers.map((provider) => inventoryRow({ id: provider.id, name: provider.name, kind: 'provider', parent_id: 'resources', resource: { kind: 'provider', value: provider } })), next_cursor: null }, response: new Response() };
    }
    if (path === '/providers/{id}') return { data: await calls.read(options?.params?.path?.id ?? ''), response: new Response() };
    if (path === '/nodes/{id}' && options?.params?.path?.id === HOME) return { data: org(HOME, 'acme', 'Acme', owner), response: new Response() };
    throw new Error(`Unexpected GET ${path}`);
  } } };
});
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), putSetting: vi.fn(), clearSetting: vi.fn() }));
vi.mock('@/features/navigation/placeLabels', () => ({ useKnownPlaces: () => new Map() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/ResourceCreation', () => ({ ResourceCreateButton: ({ kind, onCreated }: { kind: string; onCreated?: (id: string) => void }) => {
  return <button type="button" data-create-kind={kind} onClick={() => {
    calls.create();
    if (calls.created !== null) { calls.list.mockResolvedValue([calls.created]); onCreated?.(calls.created.id); }
  }}>{i18n.t('resourceCreation.providers')}</button>;
},
}));
const HOME = id(93, 1), PROJECT = id(93, 2), BETA = id(93, 3);
const KEY = 'agent.default_provider_id';
const FIRST = '00000000-0000-7000-8000-000000000001';
const SECOND = '00000000-0000-7000-8000-000000000002';
const SAMPLE = '00000000-0000-7000-8000-000000000099';
const owner = userRef(me(1, 'ada', 'Ada'));
const rows: Provider[] = [
  planProvider({ id: FIRST, name: 'Release AI', home: { id: HOME, kind: 'org', name: 'Acme', redacted: false }, owner, org_id: HOME }),
  planProvider({ id: SECOND, name: 'Review AI', home: { id: HOME, kind: 'org', name: 'Acme', redacted: false }, owner, org_id: HOME }),
];
const target: SettingsTarget = { kind: 'node', id: PROJECT };
const place = { name: 'Web', kind: 'project', owner: null, access: { permissions: ['project.manage', 'settings.manage'] } };
function definition(): KeyDef {
  return { key: KEY, title: i18n.t('day100.builtin.agent.default_provider_id.title'), description: i18n.t('day100.builtin.agent.default_provider_id.description'),
    group: 'agent', class: 'parameter', scopes: ['org', 'project', 'topic'], merge: 'replace', default: null, example: SAMPLE,
    schema: { type: 'string', format: 'uuid' }, text_key: null, keywords: [] };
}
function value(id = FIRST, own = false): ResolvedSetting {
  return { key: KEY, value: id, own: own ? { value: id, revision: 7, locked: false } : null,
    source: { scope: own ? 'project' : 'default', node: null }, locked_by: null, writable: true, parts: [] };
}
const appearance = useAppearance.getState();
beforeEach(() => {
  vi.clearAllMocks(); calls.created = null; calls.list.mockResolvedValue(rows);
  calls.read.mockImplementation((id) => {
    const row = rows.find((row) => row.id === id);
    return row === undefined ? Promise.reject(new Error('the provider is no longer readable')) : Promise.resolve(row);
  });
});
afterEach(async () => { useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`commits one named choice at project scope and undoes its acknowledged revision (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const accepted = value(SECOND, true);
    vi.mocked(putSetting).mockResolvedValue(accepted); vi.mocked(clearSetting).mockResolvedValue(value());
    const def = definition();
    const view = show(<ul><SettingRow def={def} setting={value()} target={target} place={place} /></ul>);
    const choose = await screen.findByRole('combobox', { name: def.title });
    await waitFor(() => expect(choose).toHaveValue('Release AI'));
    expect(calls.list).not.toHaveBeenCalled();
    expect(screen.queryByRole('textbox', { name: def.title })).not.toBeInTheDocument();
    expect(view.container).not.toHaveTextContent(FIRST);
    expect(view.container).not.toHaveTextContent(SAMPLE);
    const user = userEvent.setup();
    await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) }));
    expect(await screen.findByText(i18n.t('places.providerDefault.example'))).toBeVisible();
    expect(calls.list).not.toHaveBeenCalled(); expect(putSetting).not.toHaveBeenCalled();
    await user.keyboard('{Escape}'); await user.click(choose);
    await user.click(await screen.findByRole('option', { name: 'Review AI' }));
    await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, KEY, { value: SECOND, expectedRevision: null }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: def.title })).toHaveValue('Review AI'));
    expect(calls.read).toHaveBeenCalledExactlyOnceWith(FIRST);
    await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: def.title }) }));
    await waitFor(() => expect(clearSetting).toHaveBeenCalledExactlyOnceWith(target, KEY, 7));
  });

  it(`renders a locked inherited provider by name without loading the choice inventory (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const source = { scope: 'org' as const, node: { id: HOME, kind: 'org' as const, name: 'Acme', redacted: false } };
    // Even a local stored choice and manage authority cannot replace an ancestor's effective lock.
    const setting: ResolvedSetting = { ...value(), own: { value: SECOND, revision: 7, locked: false }, source, locked_by: source };
    const view = show(<ul><SettingRow def={definition()} setting={setting} target={target} place={place} /></ul>);
    expect(await screen.findByText('Release AI', { exact: true })).toBeVisible();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
    expect(calls.list).not.toHaveBeenCalled();
    expect(view.container).not.toHaveTextContent(FIRST);
    expect(screen.queryByText('Review AI', { exact: true })).not.toBeInTheDocument();
    expect(putSetting).not.toHaveBeenCalled();
  });

  it(`keeps failed inventory distinct from an empty one and offers its existing creation workflow (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    calls.list.mockRejectedValueOnce(new Error('the provider inventory is unavailable')).mockResolvedValue([]);
    const def = definition(); show(<ul><SettingRow def={def} setting={value()} target={target} place={place} /></ul>);
    expect(calls.list).not.toHaveBeenCalled();
    await userEvent.click(await screen.findByRole('combobox', { name: def.title }));
    const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
    expect(screen.queryByText(i18n.t('places.providerDefault.empty'))).not.toBeInTheDocument();
    await userEvent.click(retry);
    expect(await screen.findByText(i18n.t('places.providerDefault.empty'))).toBeVisible();
    const create = screen.getByRole('button', { name: i18n.t('resourceCreation.providers') });
    expect(create).toHaveAttribute('data-create-kind', 'providers');
    await userEvent.click(create);
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(putSetting).not.toHaveBeenCalled();
  });
}

it('retains a refused named choice and retries under the original own revision', async () => {
  await setLocale('en'); const def = definition();
  vi.mocked(putSetting).mockRejectedValueOnce(new Error('the setting could not be changed'))
    .mockResolvedValue({ ...value(SECOND, true), own: { value: SECOND, revision: 8, locked: false } });
  show(<ul><SettingRow def={def} setting={value(FIRST, true)} target={target} place={place} /></ul>);
  const choose = await screen.findByRole('combobox', { name: def.title });
  await waitFor(() => expect(choose).toHaveValue('Release AI'));
  const user = userEvent.setup(); await user.click(choose);
  await user.click(await screen.findByRole('option', { name: 'Review AI' }));
  const retry = await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('combobox', { name: def.title })).toHaveValue('Review AI');
  await user.click(retry);
  await waitFor(() => expect(putSetting).toHaveBeenCalledTimes(2));
  expect(putSetting).toHaveBeenNthCalledWith(1, target, KEY, { value: SECOND, expectedRevision: 7 });
  expect(putSetting).toHaveBeenNthCalledWith(2, target, KEY, { value: SECOND, expectedRevision: 7 });
  expect(await screen.findByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: def.title }) })).toBeEnabled();
});

it('keeps duplicate provider names distinguishable using readable homes', async () => {
  await setLocale('en'); calls.list.mockResolvedValue([
    { ...rows[0]!, name: 'Team AI' },
    { ...rows[1]!, name: 'Team AI', home: { id: BETA, kind: 'org', name: 'Beta', redacted: false } },
  ]);
  const def = definition(); show(<ul><SettingRow def={def} setting={value()} target={target} place={place} /></ul>);
  const choose = await screen.findByRole('combobox', { name: def.title });
  await userEvent.click(choose);
  expect(choose).toHaveValue('');
  expect(await screen.findByRole('option', { name: /Team AI.*Acme/ })).toBeVisible();
  expect(screen.getByRole('option', { name: /Team AI.*Beta/ })).toBeVisible();
  await userEvent.keyboard('{Escape}'); expect(choose).toHaveValue('Team AI (Acme)');
});

it('returns an empty-list creation answer to the picker and waits for an explicit default choice', async () => {
  await setLocale('en'); calls.list.mockResolvedValue([]);
  calls.created = { ...rows[1]!, access: { ...rows[1]!.access, permissions: ['provider.view', 'provider.use'] } };
  vi.mocked(putSetting).mockResolvedValue(value(SECOND, true));
  const def = definition(); show(<ul><SettingRow def={def} setting={value()} target={target} place={place} /></ul>);
  const user = userEvent.setup();
  expect(calls.list).not.toHaveBeenCalled();
  await user.click(await screen.findByRole('combobox', { name: def.title }));
  await user.click(await screen.findByRole('button', { name: i18n.t('resourceCreation.providers') }));
  const choose = screen.getByRole('combobox', { name: def.title });
  await waitFor(() => expect(choose).toBeEnabled());
  expect(putSetting).not.toHaveBeenCalled();
  await waitFor(() => expect(calls.list).toHaveBeenCalledTimes(2)); // Initial empty page, then native admission after creation.
  await user.click(choose);
  await user.click(await screen.findByRole('option', { name: 'Review AI' }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, KEY, { value: SECOND, expectedRevision: null }));
});

it('does not fetch an illustrative UUID, and a lost read-only provider exposes no raw identifier', async () => {
  await setLocale('en'); const def = definition();
  const sample = show(<SettingSummary def={def} value={SAMPLE} example />);
  expect(screen.getByText(i18n.t('places.providerDefault.example'))).toBeVisible();
  expect(calls.read).not.toHaveBeenCalled(); sample.unmount();
  const unavailable = show(<SettingSummary def={def} value={SAMPLE} />);
  expect(await screen.findByText(i18n.t('places.providerDefault.unavailable'))).toBeVisible();
  expect(unavailable.container).not.toHaveTextContent(SAMPLE);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  expect(calls.read).toHaveBeenNthCalledWith(2, SAMPLE);
});
