/** Template and host choices use readable authorized names through the native revisioned setting row. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { resourceRows, type ResourceRows } from '@/api/listed';
import type * as SettingsApi from '@/api/settings';
import { putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import type { Host, ResolvedSetting, SettingDef, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailHost, detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ hosts: vi.fn<() => Promise<ResourceRows<Host>>>(), templates: vi.fn<() => Promise<ResourceRows<Template>>>(),
  hostName: vi.fn<(id: string) => Promise<Host>>(), templateName: vi.fn<(id: string) => Promise<Template>>() }));
vi.mock('@/api/resources', async (load) => { const actual = await load<typeof ResourcesApi>(); return { ...actual,
  hostsQuery: () => ({ queryKey: ['named-host-choices'], queryFn: reads.hosts, staleTime: Infinity }),
  hostQuery: (id: string) => ({ queryKey: ['selected-host', id], queryFn: () => reads.hostName(id), staleTime: Infinity }),
  templateQuery: (id: string) => ({ queryKey: ['selected-template', id], queryFn: () => reads.templateName(id), staleTime: Infinity }),
  templatesQuery: (kind: string) => ({ queryKey: ['named-template-choices', kind], queryFn: reads.templates, staleTime: Infinity }) }; });
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), putSetting: vi.fn() }));
vi.mock('@/features/settings/sourceAddress', () => ({ useSourceOrgAddress: () => null }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useLocation: () => ({ pathname: '/resources/example', searchStr: '' }) }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
const currentId = '0199c1a0-0000-7000-8000-000000000001', nextId = '0199c1a0-0000-7000-8000-000000000002';
const initialAppearance = useAppearance.getState(), wasDark = document.documentElement.classList.contains('dark'), previousTheme = document.documentElement.dataset.theme;
beforeEach(() => {
  vi.clearAllMocks(); vi.mocked(putSetting).mockReset();
  reads.hosts.mockResolvedValue(resourceRows([{ ...detailHost, id: currentId, name: 'Research host', access: { ...detailHost.access, permissions: ['host.view', 'host.use'] } },
    { ...detailHost, id: nextId, name: 'Build host', access: { ...detailHost.access, permissions: ['host.view', 'host.use'] } },
    { ...detailHost, id: 'read-only-host', name: 'Read-only host', access: { ...detailHost.access, permissions: ['host.view'] } }]));
  reads.templates.mockResolvedValue(resourceRows([{ ...detailTemplate, id: currentId, name: 'Research template', access: { ...detailTemplate.access, permissions: ['template.view', 'template.use'] } },
    { ...detailTemplate, id: nextId, name: 'Build template', access: { ...detailTemplate.access, permissions: ['template.view', 'template.use'] } },
    { ...detailTemplate, id: 'read-only-template', name: 'Read-only template', access: { ...detailTemplate.access, permissions: ['template.view'] } }]));
  reads.hostName.mockImplementation((id) => id === currentId || id === nextId ? Promise.resolve({ ...detailHost, id, name: id === currentId ? 'Research host' : 'Build host' }) : Promise.reject(new Error('The selected host is unavailable')));
  reads.templateName.mockImplementation((id) => id === currentId || id === nextId ? Promise.resolve({ ...detailTemplate, id, name: id === currentId ? 'Research template' : 'Build template' }) : Promise.reject(new Error('The selected template is unavailable')));
});
afterEach(async () => { cleanup(); useAppearance.setState(initialAppearance); document.documentElement.classList.toggle('dark', wasDark); if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme; await setLocale('en'); });

function draw(key: 'env.template_id' | 'env.host_id' | 'runtime.template_id' | 'runtime.host_id', writable = true, selected: unknown = currentId) {
  const kind = key.startsWith('env.') ? 'environment' : 'provider';
  const target = { kind, id: 'subject' } as const;
  const title = i18n.t(key.endsWith('.template_id') ? 'environments.overview.template' : 'providers.runtime.host');
  const def: SettingDef = { key, title, description: '', group: kind, scopes: [kind], class: 'parameter', merge: 'replace', schema: { type: 'string', format: 'uuid' }, default: null, example: nextId, text_key: null, keywords: [] };
  const setting: ResolvedSetting = { key, value: selected, own: { value: selected, revision: 7, locked: false }, source: { scope: kind === 'provider' ? 'provider_own' : 'environment_own', node: null }, locked_by: null, writable, parts: [] };
  vi.mocked(putSetting).mockImplementation((_target, _key, write) => Promise.resolve({ ...setting, value: write.value, own: { value: write.value, revision: 8, locked: false } }));
  return { ...show(<SettingRow def={withGroup(def)} setting={setting} target={target} place={{ name: 'Research', owner: null, kind: null, access: { permissions: [`${kind}.manage`] } }} />), target, title };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const key of ['env.template_id', 'env.host_id', 'runtime.template_id', 'runtime.host_id'] as const) {
  it(`selects ${key} by name with one guarded write (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    const user = userEvent.setup(), view = draw(key), template = key.endsWith('.template_id');
    const choice = await screen.findByRole('combobox', { name: view.title });
    await waitFor(() => expect(choice).toHaveTextContent(template ? 'Research template' : 'Research host'));
    expect(reads.hosts).not.toHaveBeenCalled(); expect(reads.templates).not.toHaveBeenCalled();
    expect(view.container).not.toHaveTextContent(currentId); expect(screen.queryByRole('textbox')).not.toBeInTheDocument(); expect(putSetting).not.toHaveBeenCalled();
    await user.click(choice);
    expect(screen.queryByRole('option', { name: template ? 'Read-only template' : 'Read-only host' })).not.toBeInTheDocument();
    await user.click(await screen.findByRole('option', { name: template ? 'Build template' : 'Build host' }));
    await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(view.target, key, { value: nextId, expectedRevision: 7 }));
    expect(await screen.findByText(i18n.t('settings.compact.saved'))).toBeVisible();
    expect(template ? reads.templates : reads.hosts).toHaveBeenCalledTimes(1); expect(template ? reads.hosts : reads.templates).not.toHaveBeenCalled();
  });
}

it('keeps a readable name read-only and conceals an unavailable identifier', async () => {
  const first = draw('runtime.template_id', false);
  expect(await screen.findByText('Research template')).toBeVisible(); expect(screen.queryByRole('combobox')).not.toBeInTheDocument(); expect(putSetting).not.toHaveBeenCalled(); first.unmount();
  const next = draw('env.host_id', false, 'hidden-host-id');
  expect(await screen.findByText(i18n.t('settings.resourceChoice.unavailable'))).toBeVisible(); expect(next.container).not.toHaveTextContent('hidden-host-id');
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument(); expect(putSetting).not.toHaveBeenCalled();
});

it('keeps host and template inventories asleep for a minute until their actual picker opens', async () => {
  const view = draw('env.host_id');
  const choice = await screen.findByRole('combobox', { name: view.title });
  await waitFor(() => expect(choice).toHaveTextContent('Research host'));
  expect(reads.hostName).toHaveBeenCalledExactlyOnceWith(currentId);
  vi.useFakeTimers();
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(reads.hosts).not.toHaveBeenCalled(); expect(reads.templates).not.toHaveBeenCalled();
    expect(reads.hostName).toHaveBeenCalledTimes(1); expect(reads.templateName).not.toHaveBeenCalled();
  } finally { vi.useRealTimers(); }
  await userEvent.click(choice);
  expect(await screen.findByRole('option', { name: 'Build host' })).toBeVisible();
  expect(reads.hosts).toHaveBeenCalledTimes(1);
});

it('chooses automatic host placement explicitly without sending the menu sentinel', async () => {
  const user = userEvent.setup(), view = draw('env.host_id');
  const choice = await screen.findByRole('combobox', { name: view.title }); await waitFor(() => expect(choice).toHaveTextContent('Research host'));
  await user.click(choice); await user.click(screen.getByRole('option', { name: i18n.t('providerRuntime.autoHost') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(view.target, 'env.host_id', { value: null, expectedRevision: 7 }));
});
