/** A mount's managers set what plans preselect for it and say what each location's data is; others only read the hint. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type * as Tools from '@/api/resource-tools';
import type { CleanupPlan, CleanupRequest, CleanupStart, Job, Mount, MountLocation } from '@/generated/api-types';
import { EditMount } from '@/features/resources/mounts/Edit';
import { LocationDataKind, Locations } from '@/features/resources/mounts/Locations';
import { i18n, setLocale } from '@/i18n';
import { storageCleanupResult, storageReport } from '../doubles/storage-report';
import { show } from './support';

const calls = vi.hoisted(() => ({
  hint: vi.fn(), update: vi.fn(),
  plan: vi.fn<(request: CleanupRequest) => Promise<CleanupPlan>>(),
  start: vi.fn<(body: CleanupStart) => Promise<Job>>(),
  read: vi.fn<(id: string) => Promise<Job>>(),
}));
vi.mock('@/api/resource-tools', async (load) => ({
  ...await load<typeof Tools>(),
  setMountLocationDataKind: (id: string, machine: string, kind: string | null) => { calls.hint(id, machine, kind); return Promise.resolve(mount(['mount.manage'])); },
  updateMount: (id: string, body: unknown) => { calls.update(id, body); return Promise.resolve(mount(['mount.manage'])); },
}));
vi.mock('@/api/storage', () => ({ planCleanup: calls.plan, startCleanup: calls.start }));
vi.mock('@/api/exports', async (load) => ({ ...await load<object>(), readJob: calls.read }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => vi.fn(), useRouter: () => ({ state: { location: { pathname: '/resources/mounts' } }, history: { push: vi.fn() } }), Link: ({ children }: { children: ReactNode }) => <a href="#machine">{children}</a> }));

const LOCATION: MountLocation = {
  machine: { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d', name: 'build-1', online: true, platform_os: 'linux', placement: 'managed' },
  host: { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6e', name: 'build-1', address: 'build-1' },
  kind: 'volume', storage: null, source: null, data_kind: 'own', can_remove: false,
  created_by: { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6f', kind: 'human', username: 'bo', display_name: 'Bo', avatar: { text: 'B', color: 2 } },
  created_at: '2026-10-08T17:04:05.123Z',
};

function mount(permissions: string[]): Mount {
  return {
    id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b', name: 'datasets', address: 'acme.datasets', description: '', org_id: null, home: null, owner: null,
    target: '/mnt/datasets', read_only: true, enabled: true, default_fate: 'copy', locations: [LOCATION], locations_hidden: 0, kept: [], revision: 3,
    access: { permissions, administers: false, through: [] },
  };
}

afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) describe(locale, () => {
  it('lets a manager describe a location and take the description away', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    show(<LocationDataKind mount={mount(['mount.view', 'mount.manage'])} location={LOCATION} />);
    const choice = screen.getByRole('combobox', { name: t('resourceDetails.mount.dataKind') });
    expect(choice).toHaveTextContent(t('choiceCatalog.mountDataKind.own.name'));
    await user.click(choice);
    await user.click(await screen.findByRole('option', { name: (name) => name.startsWith(t('choiceCatalog.mountDataKind.synced.name')) }));
    expect(calls.hint).toHaveBeenCalledWith(mount([]).id, LOCATION.machine.id, 'synced');
    await user.click(choice);
    await user.click(await screen.findByRole('option', { name: (name) => name.startsWith(t('choiceCatalog.mountDataKind.none.name')) }));
    expect(calls.hint).toHaveBeenLastCalledWith(mount([]).id, LOCATION.machine.id, null);
  });

  it('shows the hint to a reader without offering to change it', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    show(<LocationDataKind mount={mount(['mount.view'])} location={LOCATION} />);
    expect(screen.getByText(t('choiceCatalog.mountDataKind.own.name'))).toBeVisible();
    expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  });

  it('preselects the default the mount has and saves the one chosen', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    show(<EditMount mount={mount(['mount.view', 'mount.manage'])} onClose={vi.fn()} />);
    const dialog = await screen.findByRole('dialog');
    const choice = within(dialog).getByRole('combobox', { name: t('resourceDetails.mount.defaultFate') });
    expect(choice).toHaveTextContent(t('choiceCatalog.mountFate.copy.name'));
    await user.click(choice);
    const options = await screen.findAllByRole('option');
    expect(options.map((option) => option.textContent)).toEqual([
      expect.stringContaining(t('choiceCatalog.mountFate.detach.name')),
      expect.stringContaining(t('choiceCatalog.mountFate.as_is.name')),
      expect.stringContaining(t('choiceCatalog.mountFate.copy.name')),
    ]);
    await user.click(options[1]!);
    await user.click(within(dialog).getByRole('button', { name: t('resources.edit.save') }));
    expect(calls.update).toHaveBeenCalledWith(mount([]).id, expect.objectContaining({ default_fate: 'as_is', expected_revision: 3 }));
  });

  it('lists data a copy displaced and purges a kept volume only once the mount is named', async () => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const row = mount(['mount.view', 'mount.use', 'mount.manage']);
    row.kept = [{ id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a70', machine: LOCATION.machine, host: LOCATION.host, kind: 'volume', storage: null, source: 'orch-mount-old', data_kind: null,
      file_count: 2, total_bytes: 2048, kept_by: LOCATION.created_by, kept_at: '2026-10-20T10:00:00.000Z', can_purge: true }];
    const record = storageReport().items[0];
    if (record === undefined) throw new Error('The storage fixture contains a retained record.');
    const keptId = `mount:${row.kept[0]!.id}`;
    const plan: CleanupPlan = {
      id: 'mount-cleanup-plan', expires_at: '2026-10-30T10:00:00.000Z', confirmation_name: row.name,
      people: [], other_people: 0, notify: false,
      items: [{ record: { ...record, id: keptId, kind: 'mount_data', name: 'orch-mount-old', resource: { kind: 'mount', id: row.id },
        facts: null, why: 'restore', bytes: 2048, until: null }, choices: ['keep', 'purge'], selected: 'purge' }],
    };
    const result = storageCleanupResult(plan, new Map());
    calls.plan.mockResolvedValue(plan); calls.start.mockResolvedValue(result); calls.read.mockResolvedValue(result);
    show(<Locations mount={row} />);
    expect(screen.getByText('orch-mount-old')).toBeVisible();
    await user.click(screen.getByRole('button', { name: t('resourceDetails.mount.purge') }));
    const dialog = await screen.findByRole('dialog');
    await within(dialog).findByRole('table', { name: t('storage.cleanup.plan') });
    expect(calls.plan).toHaveBeenCalledExactlyOnceWith({ scope: 'me', resource: { kind: 'mount', id: row.id }, items: [keptId] });
    const confirm = within(dialog).getByRole('button', { name: t('storage.clean') });
    expect(confirm).toBeDisabled();
    const name = within(dialog).getByLabelText(t('storage.cleanup.typeName', { name: 'datasets' }));
    await user.type(name, 'Datasets');
    expect(confirm).toBeDisabled();
    expect(calls.start).not.toHaveBeenCalled();
    await user.clear(name);
    await user.type(name, 'datasets');
    await user.click(confirm);
    await waitFor(() => expect(calls.start).toHaveBeenCalledExactlyOnceWith({ plan_id: plan.id,
      choices: [{ id: keptId, choice: 'purge' }], confirmation_name: 'datasets', notify: false }));
  });
});
