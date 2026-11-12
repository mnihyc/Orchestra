/** Failed board changes preserve the card and give the person the API's access and request details. */
import { expect, type Page, test } from '@playwright/test';

import { ACME, deny, scenario, signIn, arrived } from './support';

test.beforeEach(async () => { await scenario(); });

/** The same pointer gesture as the project's existing successful status-drag case. */
async function dragIntoDone(page: Page): Promise<void> {
  const column = (name: RegExp) => page.getByRole('region', { name });
  const card = column(/Idle/).getByRole('listitem').filter({ hasText: 'Upgrade deps' });
  await card.scrollIntoViewIfNeeded();
  const from = await card.boundingBox();
  if (from === null) throw new Error('the board card is not drawn');
  await page.mouse.move(from.x + from.width / 2, from.y + from.height / 2);
  await page.mouse.down();
  await page.mouse.move(from.x + from.width / 2 + 12, from.y + from.height / 2 + 12, { steps: 4 });
  // The board deliberately disables automatic scrolling during a carry; scroll to the real destination.
  await column(/Done/).scrollIntoViewIfNeeded();
  const to = await column(/Done/).boundingBox();
  if (to === null) throw new Error('the destination column is not drawn');
  await page.mouse.move(to.x + to.width / 2, to.y + to.height / 2, { steps: 12 });
  await page.mouse.up();
}

test('a refused status drag reviews missing access and a failed write preserves the card and can be tried again', async ({ page }) => {
  await signIn(page);
  await page.goto(`/project/${ACME.web}?view=board`);
  const idle = page.getByRole('region', { name: /Idle/ });
  const done = page.getByRole('region', { name: /Done/ });
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await deny('ada', [`topic.interact@${ACME.upgradeDeps}`]);
  const refusedWrite = arrived(page.waitForResponse((response) => response.request().method() === 'PATCH'
    && new URL(response.url()).pathname === `/api/v1/topics/${ACME.upgradeDeps}`));
  await dragIntoDone(page);
  const refusal = await refusedWrite;
  expect(refusal.status()).toBe(403);
  const refusedProblem: unknown = await refusal.json();
  expect(refusedProblem).toMatchObject({ code: 'authz.denied', missing: [{ permissions: ['topic.interact'], object: { name: 'Upgrade deps' } }] });
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await expect(done.getByRole('link', { name: 'Upgrade deps', exact: true })).toHaveCount(0);
  await page.getByRole('navigation', { name: 'Where this is' }).getByRole('link', { name: 'Acme', exact: true }).click();
  const notices = page.getByRole('region', { name: /^Notifications/ });
  const refused = notices.getByRole('listitem').filter({ hasText: 'Upgrade deps could not be changed' });
  await refused.getByRole('button', { name: 'Details', exact: true }).click();
  const detail = page.getByRole('dialog', { name: 'Upgrade deps could not be changed' });
  // Authorized recovery facts now open from the error state's one Review access action.
  await detail.getByRole('button', { name: 'Review access', exact: true }).click();
  const access = page.getByRole('dialog', { name: 'Review access', exact: true });
  await expect(access).toContainText('You can’t take part in Upgrade deps.');
  await expect(access).toContainText('Ask Grace Hopper.');
  await expect(access.getByText('Grace Hopper', { exact: true })).toBeVisible();
  await access.getByRole('button', { name: 'Close', exact: true }).click();
  // Details acknowledges its toast; closing the dialog returns to the page without another notice to dismiss.
  await detail.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(refused).toHaveCount(0);
  await deny('ada', []);
  await page.goto(`/project/${ACME.web}?view=board`);
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();

  const topicPath = `**/api/v1/topics/${ACME.upgradeDeps}`;
  await page.route(topicPath, (route) => route.fulfill({ status: 500, contentType: 'application/problem+json', json: { type: 'orch:error:internal', code: 'internal', title: 'Change failed', status: 500, detail: 'The topic was not changed.', request_id: 'board-write-check' } }));
  const failedWrite = arrived(page.waitForResponse((response) => response.request().method() === 'PATCH'
    && new URL(response.url()).pathname === `/api/v1/topics/${ACME.upgradeDeps}`));
  await dragIntoDone(page);
  const failure = await failedWrite;
  expect(failure.status()).toBe(500);
  const failedProblem: unknown = await failure.json();
  expect(failedProblem).toMatchObject({ code: 'internal', request_id: 'board-write-check' });
  const failed = notices.getByRole('listitem').filter({ hasText: 'Upgrade deps could not be changed' });
  await failed.getByRole('button', { name: 'Details', exact: true }).click();
  await expect(detail).toContainText('The server could not complete this request. Try again.');
  // ErrorState deliberately omits protocol identifiers from interface copy; retain the diagnostic assertion on the actual reply above.
  await detail.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(failed).toHaveCount(0);
  await expect(idle.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
  await page.unroute(topicPath);
  const acceptedWrite = page.waitForResponse((response) => response.request().method() === 'PATCH'
    && new URL(response.url()).pathname === `/api/v1/topics/${ACME.upgradeDeps}`);
  await dragIntoDone(page);
  expect((await acceptedWrite).status()).toBe(200);
  await expect(done.getByRole('link', { name: 'Upgrade deps', exact: true })).toBeVisible();
});
