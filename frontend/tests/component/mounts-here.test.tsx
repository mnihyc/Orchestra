/** A machine's mount locations retain scope, reported sizes, permission cuts, guarded edits and drafts. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MountsHere } from '@/features/resources/mounts/Here';
import type { MachineMounts, Mount, MountLocationSetBody } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Toaster } from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { show } from './support';

const { read, add, remove } = vi.hoisted(() => ({
  read: vi.fn<(machine: string) => Promise<MachineMounts>>(),
  add: vi.fn<(mount: string, machine: string, body: MountLocationSetBody) => Promise<Mount>>(),
  remove: vi.fn<(mount: string, machine: string, confirm?: string) => Promise<void>>(),
}));
vi.mock('@/api/resource-tools', () => ({
  machineMountsQuery: (machine: string) => ({ queryKey: ['mounts', 'machine', machine], queryFn: () => read(machine) }),
  setMountLocation: add, removeMountLocation: remove,
}));
vi.mock('@tanstack/react-router', () => ({
  Link: ({ to, params, children, className }: { to: string; params?: { id: string }; children: ReactNode; className?: string }) => <a href={to.replace('$id', params?.id ?? '')} className={className}>{children}</a>,
}));

const saved: Mount = { id: 'new', name: 'new-data', address: 'team.new-data', description: '', org_id: 'org', home: null,
  owner: null, target: '/mnt/new', read_only: true, enabled: true, default_fate: 'as_is', locations: [], kept: [], revision: 1, access: { permissions: ['mount.manage'], administers: false, participating: true, admin_only: [], through: [] } };
function projection(): MachineMounts {
  return { locations: [
    { mount: { id: 'data', name: 'data', address: 'team.data' }, target: '/mnt/data', kind: 'path', source: '/srv/data', storage: null, data_kind: null, size_bytes: 1024,
      environments: [{ id: 'environment', name: 'Workspace', status: 'running' }], can_remove: true },
    { mount: { id: 'volume', name: 'empty-volume', address: 'team.empty-volume' }, target: '/mnt/cache', kind: 'volume', source: 'owned-volume', storage: 'fast', data_kind: null, size_bytes: 0, environments: [], can_remove: true },
    { mount: { id: 'readable', name: 'readable-data', address: 'team.readable' }, target: '/mnt/readable', kind: 'path', source: null, storage: null, data_kind: null, size_bytes: null, environments: [], can_remove: false },
  ], available: [{ mount: { id: 'new', name: 'new-data', address: 'team.new-data' }, path: true, default_storage: true, storages: ['fast'] }] };
}
beforeEach(() => { vi.clearAllMocks(); read.mockResolvedValue(projection()); add.mockResolvedValue(saved); remove.mockResolvedValue(undefined); });
afterEach(async () => { await setLocale('en'); });

describe('Mounts here', () => {
  for (const language of ['en', 'zh-CN'] as const) it(`shows the machine's path, storage, size and readable attachments (${language})`, async () => {
    await setLocale(language);
    show(<MountsHere machine="node" name="Build host" />);
    const table = await screen.findByRole('table', { name: language === 'en' ? 'Mounts here' : '此处的挂载' });
    expect(read).toHaveBeenCalledWith('node');
    expect(within(table).getByText('/srv/data')).toBeVisible();
    expect(within(table).getByText('fast')).toBeVisible();
    expect(within(table).getByText('1 KiB')).toBeVisible();
    expect(within(table).getByText('0 B')).toBeVisible();
    expect(within(table).getByText(language === 'en' ? 'Not reported' : '尚未报告')).toBeVisible();
    expect(within(table).getByText(language === 'en' ? 'Source kept private' : '源路径不公开')).toBeVisible();
    expect(within(table).getByRole('link', { name: 'Workspace' })).toHaveAttribute('href', '/resources/environments/environment');
    expect(within(table).queryByRole('button', { name: /readable-data/ })).not.toBeInTheDocument();
  });

  it('chooses only offered manageable mounts and adds their location to this machine', async () => {
    const user = userEvent.setup();
    show(<MountsHere machine="node" name="Build host" />);
    await screen.findByRole('table');
    await user.click(screen.getByRole('button', { name: 'Add a location here…' }));
    const dialog = screen.getByRole('dialog', { name: 'Add a location here…' });
    await user.click(within(dialog).getByRole('combobox', { name: 'Mount' }));
    expect(screen.getAllByRole('option')).toHaveLength(1);
    const offered = screen.getByRole('option', { name: 'new-data' });
    expect(offered).toHaveAccessibleDescription('team.new-data');
    await user.click(offered);
    await user.type(within(dialog).getByRole('textbox', { name: 'Absolute directory path' }), '/srv/new');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(add).toHaveBeenCalledWith('new', 'node', { kind: 'path', source: '/srv/new' }));
  });

  it('sends only a storage offered for the selected mount when adding a host volume', async () => {
    const user = userEvent.setup();
    show(<MountsHere machine="node" name="Build host" />);
    await screen.findByRole('table');
    await user.click(screen.getByRole('button', { name: 'Add a location here…' }));
    const dialog = screen.getByRole('dialog', { name: 'Add a location here…' });
    await user.click(within(dialog).getByRole('combobox', { name: 'Mount' }));
    await user.click(screen.getByRole('option', { name: 'new-data' }));
    await user.click(within(dialog).getByRole('combobox', { name: 'Location kind' }));
    await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.mountLocation.volume.name') }));
    await user.click(within(dialog).getByRole('combobox', { name: 'Storage' }));
    expect(screen.getAllByRole('option')).toHaveLength(2);
    await user.click(screen.getByRole('option', { name: 'fast' }));
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(add).toHaveBeenCalledWith('new', 'node', { kind: 'volume', storage: 'fast' }));
  });

  it('requires the exact mount name before removing a volume from this machine', async () => {
    const user = userEvent.setup();
    show(<MountsHere machine="node" name="Build host" />);
    await user.click(await screen.findByRole('button', { name: i18n.t('resourceDetails.mount.here.remove', { name: 'empty-volume' }) }));
    const dialog = screen.getByRole('dialog', { name: i18n.t('resourceDetails.mount.removeLocation') });
    expect(within(dialog).getByText(/Build host/)).toBeVisible();
    const erase = within(dialog).getByRole('button', { name: i18n.t('resourceDetails.remove') });
    expect(erase).toBeDisabled();
    await user.type(within(dialog).getByRole('textbox', { name: 'Mount name to confirm' }), 'empty-volume');
    await user.click(erase);
    await waitFor(() => expect(remove).toHaveBeenCalledWith('volume', 'node', 'empty-volume'));
  });

  it('keeps a failed location draft and discards it when the machine changes', async () => {
    add.mockRejectedValue(new Error('The host no longer allows this location.'));
    const user = userEvent.setup();
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const view = render(<MountsHere machine="node" name="Build host" />, { wrapper: ({ children }) =>
      <QueryClientProvider client={cache}><TooltipProvider><Toaster>{children}</Toaster></TooltipProvider></QueryClientProvider> });
    await screen.findByRole('table');
    await user.click(screen.getByRole('button', { name: 'Add a location here…' }));
    const dialog = screen.getByRole('dialog', { name: 'Add a location here…' });
    await user.click(within(dialog).getByRole('combobox', { name: 'Mount' }));
    await user.click(screen.getByRole('option', { name: 'new-data' }));
    await user.type(within(dialog).getByRole('textbox', { name: 'Absolute directory path' }), '/srv/draft');
    await user.click(within(dialog).getByRole('button', { name: i18n.t('resources.edit.save') }));
    expect(await screen.findByText(i18n.t('errors.page'))).toBeVisible();
    expect(within(dialog).getByRole('textbox', { name: 'Absolute directory path' })).toHaveValue('/srv/draft');
    view.rerender(<MountsHere machine="other" name="Other host" />);
    await waitFor(() => expect(read).toHaveBeenCalledWith('other'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByDisplayValue('/srv/draft')).not.toBeInTheDocument();
  });

  it('shows a failed read as an error with no add action or false empty result', async () => {
    read.mockRejectedValue(new Error('Mount locations could not be read.'));
    show(<MountsHere machine="node" name="Build host" />);
    expect(await screen.findByText(i18n.t('errors.page'))).toBeVisible();
    expect(screen.getByRole('button', { name: 'Add a location here…' })).toBeDisabled();
    expect(screen.queryByText(i18n.t('resourceDetails.mount.here.none', { machine: 'Build host' }))).not.toBeInTheDocument();
  });
});
