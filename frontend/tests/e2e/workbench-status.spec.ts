/** The thread's status bar shows its actual environment, workspace and shells and opens the matching view. */
import { expect, test } from '@playwright/test';
import { begin, MAIN, openThread, showView, workspace } from './inspector-support.ts';
import { emit } from './support.ts';

test.use({ viewport: { width: 1440, height: 900 } });
test.beforeEach(async () => { await begin(); });

test('environment, branch and changed lines, isolation and shell count live in the status bar', async ({ page }) => {
  await openThread(page);
  // Status observes facts that the visible views read; opening a thread does not read hidden panels.
  await showView(page, 'Git');
  await showView(page, 'Changes');
  await showView(page, 'Terminal');
  const status = page.getByRole('contentinfo', { name: 'Status' });
  await expect(status).toContainText('Connected');
  await expect(status.getByRole('button', { name: /^Workspace:/ })).toHaveText('main +3 −1');
  await expect(status).toContainText('Run with provider');
  await status.getByRole('button', { name: /^Environment:/ }).click();
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveAttribute('data-active-container', 'environment');
  await expect(page.getByTestId('environment-panel')).toBeVisible();
  await status.getByRole('button', { name: /^Workspace:/ }).click();
  await expect(page.locator('[data-workbench-bar="right"]')).toHaveAttribute('data-active-container', 'code');
  await expect(page.getByTestId('changes-panel')).toBeVisible();
  await status.getByRole('button', { name: /^\d+ shells?$/ }).click();
  await expect(page.getByRole('tablist', { name: 'Bottom', exact: true }).getByRole('tab', { name: 'Terminal', exact: true })).toBeVisible();
});

for (const reason of ['not_a_repository', 'git_missing', 'offline', 'unknown']) {
  test(`Git status stays out of the bar when repository status is ${reason}`, async ({ page }) => {
    await workspace('/git', { status: { available: false, reason, branch: null, head: null, ahead: null, behind: null, changes: [], remotes: [], upstream: null, repository_dir: null, status_hash: null } });
    await emit(MAIN, { thread: { attention: { uncommitted: null, unpushed: null, failed: false, stale: false, reason, as_of: '2026-10-15T03:05:00.000Z' } } });
    // Git is a native live view; its initial status arrives on the authorized socket subscription.
    const statuses: { available?: boolean; reason?: string }[] = [];
    page.on('websocket', (socket) => socket.on('framereceived', ({ payload }) => {
      try {
        const frame = JSON.parse(String(payload)) as { op?: string; data?: { status?: { available?: boolean; reason?: string } } };
        if ((frame.op === 'event' || frame.op === 'sync') && frame.data?.status !== undefined) statuses.push(frame.data.status);
      } catch { /* Non-JSON frames do not carry Git status. */ }
    }));
    await openThread(page);
    await showView(page, 'Git');
    await expect.poll(() => statuses.some((status) => status.available === false && status.reason === reason)).toBe(true);
    const status = page.getByRole('contentinfo', { name: 'Status' });
    await expect(status.getByRole('button', { name: /^Environment:/ })).toBeVisible();
    await expect(status.getByRole('button', { name: /^Workspace:/ })).toHaveCount(0);
    // Nothing needs Git: the bar says nothing about a missing or unreadable repository.
    await expect(status).not.toContainText(/Git|repository/);
    await expect(status).not.toContainText('+0 −0');
  });
}
