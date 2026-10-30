/** The two reach entrances share the served cut, and Management checks a release without running an update. */
import { expect, test } from '@playwright/test';
import type { AgentReachOverview, BackupRestoreList, InstallationRecords } from '../../src/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { begin, showView } from './inspector-support';
import { signIn } from './support';

test.beforeEach(async () => { await begin(); });

function reach(): AgentReachOverview {
  return { network: { mode: 'restricted', required_mode: 'restricted', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: ['packages.example'], sources: [], revision: 'r1', always: [{ authority: 'orch.example', port: 443, protocol: 'wss', purpose: 'gateway' }], always_revision: 'a1', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
    secrets: { names: ['DEPLOY_SECRET_NAME'], source: { id: ACME.web, kind: 'project', name: 'Web', redacted: false } },
    mounts: [{ mount: { id: '00000000-0000-4000-8001-000000000099', name: 'Build data', address: 'acme.build-data' }, path: '/mnt/build-data', mode: 'read_only' }],
    credentials: [{ thread: { id: ACME.mainThread, kind: 'thread', name: 'Main thread', redacted: false }, placement: 'credential_shared', expires_at: '2026-10-16T08:00:00Z' }], browser_profiles: [{ name: 'team-sign-in' }] };
}

test('the environment Overview and thread Environment panel show the same per-thread reach', async ({ page }) => {
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.route(`**/api/v1/environments/${RUNS_ON.workspace}/agent-reach`, async (route) => { await route.fulfill({ json: reach() }); });
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  const overview = page.getByRole('region', { name: 'What agents here can reach', exact: true });
  await expect(overview).toContainText('DEPLOY_SECRET_NAME');
  await expect(overview).toContainText('/mnt/build-data');
  await expect(overview).toContainText('The environment holds a copy of the model credential.');
  await expect(overview).toContainText('Copy expires');
  await expect(overview).toContainText('team-sign-in');
  await page.goto(`/thread/${ACME.mainThread}`);
  await showView(page, /^Environment$/);
  const panel = page.getByTestId('environment-panel').getByRole('region', { name: 'What agents here can reach', exact: true });
  await expect(panel).toContainText('DEPLOY_SECRET_NAME');
  await expect(panel).toContainText('/mnt/build-data');
  await expect(panel).toContainText('The environment holds a copy of the model credential.');
  await expect(panel).toContainText('team-sign-in');
});

test('a cut response on a phone shows missing permissions and loses old facts when the read is refused', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  let denied = false;
  const cut = { ...reach(), secrets: { names: null, source: null }, mounts: null, browser_profiles: null };
  await page.route(`**/api/v1/environments/${RUNS_ON.workspace}/agent-reach`, async (route) => {
    if (denied) await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', title: 'Denied', code: 'authz.denied', status: 403, detail: 'The environment is no longer readable.', request_id: 'reach-lost' } });
    else await route.fulfill({ json: cut });
  });
  await signIn(page);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  const block = page.getByRole('region', { name: 'What agents here can reach', exact: true });
  await expect(block.getByText(/^You need .+ to see this\.$/)).toHaveCount(3);
  await expect(block).not.toContainText('DEPLOY_SECRET_NAME');
  await expect(block.getByRole('link', { name: 'Open browsers', exact: true })).toHaveCount(0);
  await expect(block.getByRole('link', { name: 'Main thread', exact: true })).toBeVisible();
  denied = true;
  await block.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(block.getByRole('button', { name: /Try again/ })).toBeVisible();
  await expect(block.getByRole('link', { name: 'Main thread', exact: true })).toHaveCount(0);
  await expect(block).toContainText('Last read');
});

test('Management sends one explicit release check, refreshes its record through the registry and offers only a command to copy', async ({ page }) => {
  let records: InstallationRecords = { release: null, backup: null, backup_error: null, backup_warning: true };
  const writes: string[] = [];
  await page.route('**/api/v1/admin/installation-records', async (route) => { await route.fulfill({ json: records }); });
  await page.route('**/api/v1/admin/releases/check', async (route) => {
    writes.push(route.request().method());
    const release = { checked_at: '2026-10-14T17:00:00Z', installed_version: '0.1.0', offered_version: '0.2.0', newer: true, source: 'https://releases.example/latest/download/version.json', error: null, command: './update.sh v0.2.0' };
    records = { ...records, release };
    await route.fulfill({ json: release });
  });
  await signIn(page);
  await page.goto('/admin/overview');
  await expect(page.getByText('No release check recorded.', { exact: true })).toBeVisible();
  await expect(page.getByText('Off by default', { exact: true })).toBeVisible();
  expect(writes).toEqual([]);
  await page.getByRole('button', { name: 'Check now', exact: true }).click();
  await expect(page.getByText('Release 0.2.0 is available', { exact: true })).toBeVisible();
  await expect(page.getByText('./update.sh v0.2.0', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Copy update command', exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: /^(Install|Update now|Run update)$/ })).toHaveCount(0);
  expect(writes).toEqual(['POST']);
});

test('the restore list uses the verified manifest, distinguishes unknown from zero, and hides stale locations after a refused read', async ({ page }) => {
  const backup: BackupRestoreList['backup'] = { completed_at: '2026-10-14T18:00:00Z', database_at: '2026-10-14T17:59:00Z', release: '0.1.0', schema: '1', dump_name: 'verified-db.dump', bytes: 2048, hash: 'a'.repeat(64), cause: 'manual', database_only: true, last_failure: null };
  const list: BackupRestoreList = { backup, manifest_name: 'operator-verified.manifest.json', manifest_hash: 'b'.repeat(64),
    included: [{ category: 'database', count: 1, environments_requiring_inspection: null, reason: 'The matching database dump.', locations: [] }],
    excluded: [{ category: 'browser_profiles', count: null, environments_requiring_inspection: 1, reason: 'Inspect its host.', locations: [{ object_id: 'env', host_id: 'host', machine_id: null, runtime_id: null, driver: 'docker', storage: 'workspace', mount: null, location: '/host/profile-volume', location_known: true, copy_available: null }] }, { category: 'private_overlays', count: 0, environments_requiring_inspection: 0, reason: 'None recorded at this snapshot.', locations: [] }] };
  let answer: 'list' | 'refused' | 'none' = 'list';
  await page.route('**/api/v1/admin/installation-records', async (route) => { await route.fulfill({ json: { release: null, backup, backup_error: null, backup_warning: false } satisfies InstallationRecords }); });
  await page.route('**/api/v1/admin/backup-restore-list', async (route) => {
    if (answer === 'refused') await route.fulfill({ status: 503, json: { type: 'orch:error:source.offline', code: 'source.offline', status: 503, title: 'Manifest unavailable', detail: 'The protected manifest no longer verifies.', request_id: 'manifest-lost' } });
    else await route.fulfill({ json: answer === 'list' ? list : null });
  });
  await signIn(page);
  await page.goto('/admin/overview');
  await page.getByRole('button', { name: 'Restore list…', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Restore list…', exact: true });
  await expect(dialog.getByText('operator-verified.manifest.json', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Count not reported', { exact: true })).toBeVisible();
  await expect(dialog.getByText('0 items recorded', { exact: true })).toBeVisible();
  await expect(dialog.getByText('/host/profile-volume', { exact: true })).toBeVisible();
  await expect(dialog.getByText('Availability not checked', { exact: true })).toBeVisible();
  await expect(dialog.getByText('A separate copy was found', { exact: true })).toHaveCount(0);
  answer = 'refused';
  await dialog.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(dialog.getByRole('button', { name: /Try again/ })).toBeVisible();
  await expect(dialog.getByText('/host/profile-volume', { exact: true })).toHaveCount(0);
  answer = 'none';
  await dialog.getByRole('button', { name: /Try again/ }).click();
  await expect(dialog.getByText('No verified restore list', { exact: true })).toBeVisible();
  await expect(page.getByText('verified-db.dump', { exact: true })).toBeVisible();
});
