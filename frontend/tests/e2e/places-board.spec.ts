/** Failed board changes preserve the card and give the person the API's access and request details. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

/** The same pointer gesture as the project's existing successful status-drag case. */
async function dragIntoDone(page: Page): Promise<void> {
  const column = (name: RegExp) => page.getByRole('region', { name });
  const card = column(/Idle/).getByRole('listitem').filter({ hasText: 'Upgrade deps' });
  const from = await card.boundingBox();
  const to = await column(/Done/).boundingBox();
  if (from === null || to === null) throw new Error('the board is not drawn');
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 12, from.y + from.height / 2 + 12, { steps: 4 });
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await page.mouse.up();
}

test('a refused status drag says what is missing and whom to ask; a failed write exposes its request id and can be tried again', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}?view=board`);
  const idle = page.getByRole('region', { name: /Idle/ });
  const done = page.getByRole('region', { name: /Done/ });
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await deny('ada', [`topic.interact@${ACME.upgradeDeps}`]);
  await dragIntoDone(page);
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await expect(done.getByRole('link', { name: 'Upgrade deps', exact: true })).toHaveCount(0);
  await page.getByRole('navigation', { name: 'Where this is' }).getByRole('link', { name: 'Acme', exact: true }).click();
  const notices = page.getByRole('region', { name: /^Notifications/ });
  const refused = notices.getByRole('listitem').filter({ hasText: 'Upgrade deps could not be changed' });
  await refused.getByRole('button', { name: 'Details', exact: true }).click();
  const detail = page.getByRole('dialog', { name: 'Upgrade deps could not be changed' });
  await expect(detail).toContainText('Interact Topic');
  await expect(detail).toContainText('on Upgrade deps');
  await expect(detail).toContainText('Ask Grace Hopper.');
  // The dialog is closed by its own button: Escape closes the dialog and the notice it was opened from together.
  await detail.getByRole('button', { name: 'Close', exact: true }).click();
  await refused.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await deny('ada', []);
  await page.goto(`/project/${ACME.web}?view=board`);
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();

  const topicPath = `**/api/v1/topics/${ACME.upgradeDeps}`;
  await page.route(topicPath, (route) => route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', title: 'Change failed', status: 500, detail: 'The topic was not changed.', request_id: 'board-write-check' } }));
  await dragIntoDone(page);
  const failed = notices.getByRole('listitem').filter({ hasText: 'Upgrade deps could not be changed' });
  await failed.getByRole('button', { name: 'Details', exact: true }).click();
  await expect(detail).toContainText('Something went wrong on the server.');
  await detail.locator('summary').click();
  await expect(detail.getByText('board-write-check', { exact: true })).toBeVisible();
  await detail.getByRole('button', { name: 'Close', exact: true }).click();
  await failed.getByRole('button', { name: 'Dismiss', exact: true }).click();
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await page.unroute(topicPath);
  await dragIntoDone(page);
  await expect(done.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
});
