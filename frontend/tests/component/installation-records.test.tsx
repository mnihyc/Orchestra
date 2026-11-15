/** Looking at release and backup records never runs an update or promotes a failed backup to a success. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { Installation } from '@/features/admin/Installation';
import { PublicAddressEditor } from '@/features/admin/PublicAddress';
import { ServerUpdateRow } from '@/features/admin/ServerUpdate';
import type { PublicAddressSnapshot, PublicAddressUpdated, PublicAddressUpdateBody } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import type { BackupRestoreList, DatabaseBackupRecord, InstallationRecords, ReleaseCheck, ResolvedSetting, ServerUpdateView, Job } from '@/generated/api-types';

import { show, skipOverflowFallback } from './support';
// This form verifies commands and drafts; a browser measures its overflow layout.
skipOverflowFallback();

const { read, settings, check, restore, serverRead, serverStart } = vi.hoisted(() => ({
  read: vi.fn<() => Promise<InstallationRecords>>(),
  settings: vi.fn<() => Promise<ResolvedSetting[]>>(),
  check: vi.fn<() => Promise<ReleaseCheck>>(),
  restore: vi.fn<() => Promise<BackupRestoreList | null>>(),
  serverRead: vi.fn<() => Promise<ServerUpdateView>>(),
  serverStart: vi.fn<(release: string) => Promise<Job>>(),
}));
vi.mock('@/api/admin', async (original) => ({ ...await original<object>(),
  installationRecordsQuery: { queryKey: ['admin', 'installation-records'], queryFn: read },
  systemSettingsQuery: { queryKey: ['admin', 'settings'], queryFn: settings },
  checkRelease: check,
  backupRestoreListQuery: { queryKey: ['admin', 'backup-restore-list'], queryFn: restore },
}));

vi.mock('@/api/serverUpdate', async (load) => ({ ...await load<object>(),
  serverUpdateQuery: { queryKey: ['admin', 'server-update'], queryFn: serverRead },
  startServerUpdate: serverStart,
}));

afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

const BACKUP: DatabaseBackupRecord = { completed_at: '2026-10-12T08:02:00Z', database_at: '2026-10-12T08:00:00Z', release: '0.1.0', schema: '1', dump_name: 'orch-20261012-backup.dump', bytes: 65_536, hash: 'a'.repeat(64), cause: 'pre_update', database_only: true, last_failure: { at: '2026-10-14T08:00:00Z', reason: 'The backup disk is full.' } };
const RELEASE: ReleaseCheck = { checked_at: '2026-10-12T09:00:00Z', installed_version: '0.1.0', offered_version: '0.2.0', newer: true, source: 'https://releases.example/latest/download/version.json', error: null, command: './update.sh v0.2.0' };
const empty = (): InstallationRecords => ({ release: null, backup: null, backup_error: null, backup_warning: true });

function setting(key: string, value: number): ResolvedSetting {
  return { key, value, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true, parts: [] };
}

function draw(release: ReleaseCheck | null = null) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  settings.mockResolvedValue([setting('release.check_days', 0), setting('backup.warn_days', 2)]);
  restore.mockResolvedValue(null);
  serverRead.mockResolvedValue({ ...serverView(), installed_version: release?.installed_version ?? RELEASE.installed_version,
    offered_version: release?.offered_version ?? null, update_available: release?.newer ?? false,
    availability: 'no_bundled_node', command: release?.command ?? './update.sh latest' });
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
    await act(async () => { client.setQueryData(['admin', 'installation-records'], { ...empty(), release: RELEASE }); client.setQueryData(['admin', 'server-update'], { ...serverView(), installed_version: RELEASE.installed_version, offered_version: RELEASE.offered_version, availability: 'no_bundled_node', command: RELEASE.command }); pending.resolve?.(RELEASE); await response; });
    expect(await screen.findByText('Release 0.2.0 is available')).toBeVisible();
    expect(screen.getByText('./update.sh v0.2.0')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Copy update command' })).toBeVisible();
    expect(screen.queryByRole('button', { name: /Install|Run update|Update now/ })).not.toBeInTheDocument();
    expect(await screen.findByText('Automatic checks are off')).toBeVisible();
  });

  it('dates a retained successful offer after a fetch failure, rather than claiming a new successful check', async () => {
    read.mockResolvedValue({ ...empty(), release: { ...RELEASE, error: 'The release source did not answer.' } });
    const { container } = draw(RELEASE);
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

type AddressSave = (body: PublicAddressUpdateBody) => Promise<PublicAddressUpdated>;
const addressSnapshot = (alternates: string[] = []): PublicAddressSnapshot => ({ public_url: 'https://before.example.test', alternate_urls: alternates, revision: 1, from_file_seed: true, using_example: false });
const addressReceipt = (primary: string): PublicAddressUpdated => ({ projection: 'receipt', public_url: primary, alternate_urls: [], revision: 2, using_example: false });
function addressHeld<T>() { let resolve!: (value: T) => void; let reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }

it.each(['en', 'zh-CN'] as const)('keeps an address draft and its applied revision when preparation fails in %s', async (locale) => {
  await setLocale(locale);
  const held = addressHeld<PublicAddressUpdated>(), save = vi.fn<AddressSave>(() => held.promise), open = vi.fn();
  show(<PublicAddressEditor snapshot={addressSnapshot()} onSave={save} onOpen={open} />);
  const primary = screen.getByRole('textbox', { name: i18n.t('publicAddress.primary') });
  await userEvent.clear(primary); await userEvent.type(primary, 'https://changed.example.test');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.preparing') }));
  expect(save).toHaveBeenCalledTimes(1);
  await userEvent.type(primary, '/still-typing');
  await act(async () => { held.reject(new ApiError({ type: 'orch:error:public_address.frontend_not_ready', title: 'Not ready', code: 'public_address.frontend_not_ready', status: 409, detail: 'The workers did not adopt the certificate', request_id: 'address-draft' }, null)); await held.promise.catch(() => undefined); });
  expect(primary).toHaveValue('https://changed.example.test/still-typing');
  expect(screen.getByText(i18n.t('publicAddress.appliedRevision', { revision: '1' }))).toBeVisible();
  expect(open).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('opens only the acting browser after address publication in %s', async (locale) => {
  await setLocale(locale);
  const open = vi.fn(), save = vi.fn<AddressSave>(() => Promise.resolve(addressReceipt('https://changed.example.test')));
  show(<PublicAddressEditor snapshot={addressSnapshot()} onSave={save} onOpen={open} />);
  const primary = screen.getByRole('textbox', { name: i18n.t('publicAddress.primary') });
  await userEvent.clear(primary); await userEvent.type(primary, 'https://changed.example.test');
  expect(open).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await waitFor(() => expect(open).toHaveBeenCalledExactlyOnceWith('https://changed.example.test'));
  expect(save).toHaveBeenCalledWith({ expected_revision: 1, public_url: 'https://changed.example.test', alternate_urls: [] });
});

it('keeps newer typing instead of navigating it away when a held address save succeeds', async () => {
  const held = addressHeld<PublicAddressUpdated>(), save = vi.fn<AddressSave>(() => held.promise), open = vi.fn();
  show(<PublicAddressEditor snapshot={addressSnapshot()} onSave={save} onOpen={open} />);
  const primary = screen.getByRole('textbox', { name: i18n.t('publicAddress.primary') });
  await userEvent.clear(primary); await userEvent.type(primary, 'https://changed.example.test');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await userEvent.clear(primary); await userEvent.type(primary, 'https://next.example.test');
  await act(async () => { held.resolve(addressReceipt('https://changed.example.test')); await held.promise; });
  expect(primary).toHaveValue('https://next.example.test');
  expect(open).not.toHaveBeenCalled();
  expect(screen.getByText(i18n.t('publicAddress.appliedRevision', { revision: '2' }))).toBeVisible();
});

it('finds an alternate in a large editable list without drawing the whole list or dropping unshown addresses', async () => {
  const user = userEvent.setup();
  const alternates = Array.from({ length: 1000 }, (_, index) => `https://alternate-${index}.example.test`);
  const save = vi.fn<AddressSave>(() => Promise.resolve(addressReceipt('https://changed.example.test')));
  show(<PublicAddressEditor snapshot={addressSnapshot(alternates)} onSave={save} onOpen={vi.fn()} />);
  expect(screen.getAllByRole('textbox').length).toBeLessThan(100);
  await user.click(screen.getByRole('searchbox', { name: i18n.t('publicAddress.search') }));
  await user.paste('alternate-999.');
  const last = await screen.findByDisplayValue('https://alternate-999.example.test');
  await user.clear(last); await user.paste('https://edited-last.example.test');
  const primary = screen.getByRole('textbox', { name: i18n.t('publicAddress.primary') });
  await user.clear(primary); await user.paste('https://changed.example.test');
  await user.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0]?.[0]?.alternate_urls).toEqual([...alternates.slice(0, -1), 'https://edited-last.example.test']);
});

it('retains a stale address draft while adopting the current revision for an explicit retry', async () => {
  const changed = { ...addressSnapshot(), public_url: 'https://someone-else.example.test', revision: 3 };
  const stale = new ApiError({ type: 'orch:error:conflict.stale_revision', title: 'Stale', code: 'conflict.stale_revision', status: 409, detail: 'Changed while preparing', request_id: 'stale-address' }, null);
  const save = vi.fn<AddressSave>().mockRejectedValueOnce(stale).mockResolvedValueOnce({ ...addressReceipt('https://mine.example.test'), revision: 4 });
  const refresh = vi.fn(() => Promise.resolve(changed)), open = vi.fn();
  show(<PublicAddressEditor snapshot={addressSnapshot()} onSave={save} onRefresh={refresh} onOpen={open} />);
  const primary = screen.getByRole('textbox', { name: i18n.t('publicAddress.primary') });
  await userEvent.clear(primary); await userEvent.type(primary, 'https://mine.example.test');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await waitFor(() => expect(refresh).toHaveBeenCalledTimes(1));
  expect(primary).toHaveValue('https://mine.example.test');
  expect(open).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('publicAddress.saveOpen') }));
  await waitFor(() => expect(open).toHaveBeenCalledExactlyOnceWith('https://mine.example.test'));
  expect(save.mock.calls[1]?.[0]?.expected_revision).toBe(3);
});


function serverView(): ServerUpdateView {
  return { installed_version: '1.2.2', offered_version: '1.2.3', update_available: true, availability: 'ready', command: './update.sh v1.2.3', job: null, observation: null };
}
function serverJob(): Job {
  return { id: 'server-job', kind: 'server_update', state: 'running', node_id: null, environment_id: null, provider_id: null, batch_id: null, result: null, error: null, log: null, created_by: null,
    created_at: '2026-11-14T23:00:00Z', started_at: '2026-11-14T23:00:01Z', finished_at: null,
    progress: { phase: 'update', phases: [{ name: 'commit', weight: 0, state: 'done' }, { name: 'update', weight: 100, state: 'running' }], percent: 30,
      bytes_done: 0, bytes_total: null, files_done: 0, files_total: null, rate_bps: null, eta_seconds: null, message: 'fetching' } };
}

it.each(['en', 'zh-CN'] as const)('starts a server update only on its explicit press and keeps one pending act in %s', async (locale) => {
  await setLocale(locale);
  const held = addressHeld<Job>(), start = vi.fn<() => Promise<Job>>(() => held.promise);
  show(<ServerUpdateRow view={serverView()} onStart={start} />);
  expect(screen.getByText('1.2.2')).toBeVisible(); expect(screen.getByText('1.2.3')).toBeVisible();
  expect(start).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('serverUpdate.update') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('serverUpdate.updating') }));
  expect(start).toHaveBeenCalledTimes(1);
  await act(async () => { held.resolve(serverJob()); await held.promise; });
  expect(document.querySelector('[data-job-state="running"]')).toBeVisible();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it.each(['no_bundled_node', 'node_offline', 'docker_unavailable'] as const)('names %s beside the unchanged server updater command without starting a job', (availability) => {
  const start = vi.fn<() => Promise<Job>>();
  show(<ServerUpdateRow view={{ ...serverView(), availability }} onStart={start} />);
  expect(screen.getByText(i18n.t(`serverUpdate.unavailable.${availability}`))).toBeVisible();
  expect(screen.getByText('./update.sh v1.2.3', { selector: 'code' })).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('serverUpdate.update') })).not.toBeInTheDocument();
  expect(start).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('keeps the updater job through restart and shows a recorded rollback after a fresh read in %s', async (locale) => {
  await setLocale(locale);
  const job = serverJob(), start = vi.fn<() => Promise<Job>>();
  const view = show(<ServerUpdateRow view={{ ...serverView(), job }} onStart={start} restarting />);
  expect(screen.getByText(i18n.t('serverUpdate.restarting'))).toBeVisible();
  expect(document.querySelector('[data-job-state="running"]')).toBeVisible();
  const finished = { ...job, state: 'failed' as const, finished_at: '2026-11-14T23:02:00Z' };
  view.rerender(<ServerUpdateRow view={{ ...serverView(), job: finished, observation: { stage: 'rolled_back', reason: 'health_check_failed', from_version: '1.2.2', to_version: '1.2.3' } }} onStart={start} />);
  expect(screen.getByText(i18n.t('serverUpdate.rolledBack', { version: '1.2.2' }))).toBeVisible();
  expect(screen.getByText(i18n.t('serverUpdate.reasons.health_check_failed'))).toBeVisible();
  expect(start).not.toHaveBeenCalled();
});
