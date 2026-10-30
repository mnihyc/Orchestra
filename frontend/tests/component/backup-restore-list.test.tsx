/** Verified inventory keeps unknown counts/copies distinct from zero and drops cached facts on a failed read. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { BackupRestoreList } from '@/features/admin/BackupRestoreList';
import type { BackupRestoreList as RestoreList } from '@/generated/api-types';
import { show } from './support';

const { read } = vi.hoisted(() => ({ read: vi.fn<() => Promise<RestoreList | null>>() }));
vi.mock('@/api/admin', async (original) => ({ ...await original<object>(), backupRestoreListQuery: { queryKey: ['admin', 'backup-restore-list'], queryFn: read } }));
afterEach(() => { vi.clearAllMocks(); });

const LIST: RestoreList = { backup: { completed_at: '2026-10-12T08:00:00Z', database_at: '2026-10-12T07:59:00Z', release: '0.1.0', schema: '1', dump_name: 'db.dump', bytes: 1024, hash: 'a'.repeat(64), cause: 'manual', database_only: true, last_failure: null }, manifest_name: 'actual-protected-copy.json', manifest_hash: 'b'.repeat(64),
  included: [{ category: 'database', count: 1, environments_requiring_inspection: null, reason: 'The verified database dump.', locations: [] }],
  excluded: [{ category: 'browser_profiles', count: null, environments_requiring_inspection: 2, reason: 'Inspect the hosts.', locations: [{ object_id: 'environment', host_id: 'host', machine_id: null, runtime_id: null, driver: 'docker', storage: 'workspace', mount: null, location: '/volumes/profiles', location_known: true, copy_available: null }] }, { category: 'private_overlays', count: 0, environments_requiring_inspection: 0, reason: 'No private overlays at this snapshot.', locations: [] }] };

function draw(expected = LIST.backup) {
  return show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><BackupRestoreList expected={expected} /></QueryClientProvider>);
}

it('uses the actual manifest identity and never turns an unknown count or a path into a verified copy', async () => {
  read.mockResolvedValue(LIST);
  draw();
  expect(await screen.findByText('actual-protected-copy.json')).toBeVisible();
  expect(screen.getByText('Count not reported')).toBeVisible();
  expect(screen.getByText('0 items recorded')).toBeVisible();
  expect(screen.getByText('/volumes/profiles')).toBeVisible();
  expect(screen.getByText('Availability not checked')).toBeVisible();
  expect(screen.queryByText('A separate copy was found')).not.toBeInTheDocument();
  expect(screen.getByText('2 environments need inspection')).toBeVisible();
});

it('hides the cached list after a failed verification and retries the authoritative read', async () => {
  read.mockResolvedValueOnce(LIST).mockRejectedValueOnce(new Error('The manifest no longer verifies.')).mockResolvedValue(null);
  draw();
  await screen.findByText('actual-protected-copy.json');
  await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(screen.queryByText('/volumes/profiles')).not.toBeInTheDocument());
  await userEvent.click(screen.getByRole('button', { name: /Try again/ }));
  expect(await screen.findByText('No verified restore list')).toBeVisible();
});

it('refuses to mix a newly verified dump with the previous Overview record', async () => {
  read.mockResolvedValue({ ...LIST, backup: { ...LIST.backup, hash: 'c'.repeat(64) } });
  draw();
  expect(await screen.findByText('The verified backup changed. Close this list, read the latest backup record and review again.')).toBeVisible();
  expect(screen.queryByText('/volumes/profiles')).not.toBeInTheDocument();
});
