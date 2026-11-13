/** A deleted topic that is brought back stays in its project's list when the frames that say it was deleted come late: after the list was read again, and in order with the frames that say it came back. */
import { expect, test, type Page, type WebSocketRoute } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

/** What the page is sent over the socket is held from `hold()` to `release()`; what it sends is not. */
async function frames(page: Page) {
  const held: (() => void)[] = [];
  let holding = false;
  await page.routeWebSocket(/\/ws$/, (route: WebSocketRoute) => {
    const server = route.connectToServer();
    server.onMessage((message) => { if (holding) held.push(() => route.send(message)); else route.send(message); });
    route.onMessage((message) => server.send(message));
  });
  return {
    hold: () => { holding = true; },
    release: () => { holding = false; for (const send of held.splice(0)) send(); },
  };
}

for (const width of [1440, 400]) {
  test(`a topic deleted from a selection and brought back by Undo is in the list when the frames of both come after the list was read again (${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await scenario();
    const socket = await frames(page);
    await signIn(page);
    await page.goto(`/project/${ACME.web}?view=list&status=all`);
    const topic = page.getByRole('checkbox', { name: 'Select Upgrade deps', exact: true });
    await topic.check();
    await page.getByRole('region', { name: 'Selected topics' }).getByRole('button', { name: 'Delete', exact: true }).click();
    const review = page.getByRole('dialog', { name: 'Review selected places', exact: true });
    await expect(review).toContainText('Gone for good');
    // The feed is slow: what it says of the deletion and of the Undo comes after the answers of both acts and after the list was read again.
    socket.hold();
    await review.getByRole('button', { name: 'Apply to 1 ready place' }).click();
    const results = page.getByRole('dialog', { name: 'Results for selected places', exact: true });
    await expect(results.getByText('Succeeded', { exact: true })).toBeVisible();
    await results.getByRole('button', { name: 'Undo successful rows', exact: true }).click();
    await expect(results.getByRole('heading', { name: 'Undo results', exact: true })).toBeVisible();
    await expect(results.getByText('Succeeded', { exact: true })).toHaveCount(2);
    await results.getByRole('contentinfo').getByRole('button', { name: 'Close', exact: true }).click();
    // The answer of the Undo brought the list up to date.
    await expect(topic).toBeVisible();
    socket.release();
    // The frames come in the order they were sent: the deletion, then the Undo. The topic is there when they have been taken, and stays.
    await page.waitForTimeout(2_500);
    await expect(topic).toBeVisible();
  });
}
