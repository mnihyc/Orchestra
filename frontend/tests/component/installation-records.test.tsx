/** Looking at release and backup records never runs an update or promotes a failed backup to a success. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { Installation } from '@/features/admin/Installation';
import type { BackupRestoreList, DatabaseBackupRecord, InstallationRecords, ReleaseCheck, ResolvedSetting } from '@/generated/api-types';

import { show } from './support';

const { read, settings, check, restore } = vi.hoisted(() => ({
  read: vi.fn<() => Promise<InstallationRecords>>(),
  settings: vi.fn<() => Promise<ResolvedSetting[]>>(),
  check: vi.fn<() => Promise<ReleaseCheck>>(),
  restore: vi.fn<() => Promise<BackupRestoreList | null>>(),
}));
vi.mock('@/api/admin', async (original) => ({ ...await original<object>(),
  installationRecordsQuery: { queryKey: ['admin', 'installation-records'], queryFn: read },
  systemSettingsQuery: { queryKey: ['admin', 'settings'], queryFn: settings },
  checkRelease: check,
  backupRestoreListQuery: { queryKey: ['admin', 'backup-restore-list'], queryFn: restore },
}));

afterEach(() => { vi.clearAllMocks(); });

const BACKUP: DatabaseBackupRecord = { completed_at: '2026-10-12T08:02:00Z', database_at: '2026-10-12T08:00:00Z', release: '0.1.0', schema: '1', dump_name: 'orch-20261012-backup.dump', bytes: 65_536, hash: 'a'.repeat(64), cause: 'pre_update', database_only: true, last_failure: { at: '2026-10-14T08:00:00Z', reason: 'The backup disk is full.' } };
const RELEASE: ReleaseCheck = { checked_at: '2026-10-12T09:00:00Z', installed_version: '0.1.0', offered_version: '0.2.0', newer: true, source: 'https://releases.example/latest/download/version.json', error: null, command: './update.sh v0.2.0' };
const empty = (): InstallationRecords => ({ release: null, backup: null, backup_error: null, backup_warning: true });

function setting(key: string, value: number): ResolvedSetting {
  return { key, value, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true, parts: [] };
}

function draw() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  settings.mockResolvedValue([setting('release.check_days', 0), setting('backup.warn_days', 2)]);
  restore.mockResolvedValue(null);
  const result = show(<QueryClientProvider client={client}><Installation /></QueryClientProvider>);
  return { client, ...result };
}

describe('installation records', () => {
  it('keeps checks off until asked and shows an absent backup without a completion claim', async () => {
    read.mockResolvedValue(empty());
    draw();
    expect(await screen.findByText('No database backup recorded')).toBeVisible();
    expect(await screen.findByText('Automatic checks are off')).toBeVisible();
    expect(screen.getByText('No release check recorded.')).toBeVisible();
    expect(screen.getByText('./update.sh backup')).toBeVisible();
    expect(check).not.toHaveBeenCalled();
    expect(screen.queryByText('Completed')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Restore list…' }));
    const dialog = screen.getByRole('dialog', { name: 'Restore list…' });
    expect(await within(dialog).findByText('No verified restore list')).toBeVisible();
    expect(within(dialog).queryByRole('button', { name: 'Copy manifest name' })).not.toBeInTheDocument();
  });

  it('retains the completed snapshot when a later attempt failed, names database-only coverage and the paired manifest', async () => {
    read.mockResolvedValue({ ...empty(), backup: BACKUP, backup_error: 'The latest local record is unreadable.' });
    const { container } = draw();
    restore.mockResolvedValue({ backup: BACKUP, manifest_name: 'verified-backup-list.manifest.json', manifest_hash: 'b'.repeat(64), included: [], excluded: [] });
    expect(await screen.findByText(BACKUP.dump_name)).toBeVisible();
    expect(container.querySelector(`time[datetime="${BACKUP.completed_at}"]`)).toBeVisible();
    expect(container.querySelector(`time[datetime="${BACKUP.database_at}"]`)).toBeVisible();
    expect(container.querySelector(`time[datetime="${BACKUP.last_failure?.at ?? ''}"]`)).toBeVisible();
    expect(screen.getByText('A later backup attempt failed')).toBeVisible();
    expect(screen.getByText('The backup disk is full.')).toBeVisible();
    expect(screen.getByText(/Database only · blobs, configuration, keys and execution volumes are not in it/)).toBeVisible();
    expect(screen.getByText('The last successful backup has passed the configured warning age.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Restore list…' }));
    expect(await screen.findByText('verified-backup-list.manifest.json')).toBeVisible();
    expect(screen.queryByRole('button', { name: /Restore now/ })).not.toBeInTheDocument();
  });

  it('looks only on the explicit check, keeps progress, and renders the server command as text after the new read', async () => {
    read.mockResolvedValue(empty());
    const pending: { resolve?: (value: ReleaseCheck) => void } = {};
    const response = new Promise<ReleaseCheck>((resolve) => { pending.resolve = resolve; });
    check.mockReturnValueOnce(response);
    const { client } = draw();
    const button = await screen.findByRole('button', { name: 'Check now' });
    expect(check).not.toHaveBeenCalled();
    await userEvent.click(button);
    expect(check).toHaveBeenCalledTimes(1);
    expect(button).toHaveAttribute('aria-busy', 'true');
    // The common act registry supplies the new read before a successful act resolves.
    await act(async () => { client.setQueryData(['admin', 'installation-records'], { ...empty(), release: RELEASE }); pending.resolve?.(RELEASE); await response; });
    expect(await screen.findByText('Release 0.2.0 is available')).toBeVisible();
    expect(screen.getByText('./update.sh v0.2.0')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy update command' })).toBeVisible();
    expect(screen.queryByRole('button', { name: /Install|Run update|Update now/ })).not.toBeInTheDocument();
    expect(await screen.findByText('Automatic checks are off')).toBeVisible();
  });

  it('dates a retained successful offer after a fetch failure, rather than claiming a new successful check', async () => {
    read.mockResolvedValue({ ...empty(), release: { ...RELEASE, error: 'The release source did not answer.' } });
    const { container } = draw();
    expect(await screen.findByText('The latest check failed')).toBeVisible();
    expect(screen.getByText('These are the dated facts from the last successful check.')).toBeVisible();
    expect(container.querySelector(`time[datetime="${RELEASE.checked_at}"]`)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Retry check' })).toBeVisible();
  });

  it('hides the old record after a refused read and offers Retry without altering the old backup time', async () => {
    read.mockResolvedValue({ ...empty(), backup: BACKUP });
    const { client } = draw();
    expect(await screen.findByText(BACKUP.dump_name)).toBeVisible();
    read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', title: 'Denied', code: 'authz.denied', status: 403, detail: 'Administration was taken away.', request_id: 'lost-admin' }, null));
    await act(async () => { await client.invalidateQueries({ queryKey: ['admin', 'installation-records'] }); });
    await waitFor(() => expect(screen.queryByText(BACKUP.dump_name)).not.toBeInTheDocument());
    read.mockResolvedValue(empty());
    await userEvent.click(screen.getByRole('button', { name: /Try again/ }));
    expect(await screen.findByText('No database backup recorded')).toBeVisible();
  });

  it('keeps legacy coverage unknown instead of promoting it to a whole-system backup', async () => {
    read.mockResolvedValue({ ...empty(), backup: { ...BACKUP, database_only: false, last_failure: null } });
    draw();
    expect(await screen.findByText('Coverage not recorded')).toBeVisible();
    expect(screen.queryByText(/^Database only ·/)).not.toBeInTheDocument();
  });
});
