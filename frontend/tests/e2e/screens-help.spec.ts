/** The screen-review lessons are usable from Help before the reader has a waiting request or listening program. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';

test.beforeEach(async () => { await scenario(); });

test('every decision lesson can finish through its labelled examples without an approval or port write', async ({ page }) => {
  await signIn(page);
  const writes: string[] = [];
  page.on('request', (request) => {
    if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method()) && /\/(?:approvals|capabilities|ports|inputs)(?:\/|$)/.test(new URL(request.url()).pathname)) writes.push(request.url());
  });
  for (const [article, title, steps] of [
    ['how-approvals-work', 'How approvals work', 3],
    ['let-agents-open-ports', 'Let agents open ports without asking', 4],
    ['change-a-running-environment', 'Change an environment while it runs', 3],
    ['who-can-see-and-do-what', 'Who can see and do what', 3],
  ] as const) {
    await page.goto(`/help/${article}`);
    const lesson = page.getByTestId('help-tutorials').locator('li').filter({ has: page.getByRole('heading', { name: title, level: 3, exact: true }) });
    await expect(lesson.getByRole('button', { name: 'Read the guide', exact: true })).toHaveCount(0);
    await lesson.getByRole('button', { name: /^(Start tutorial|Show again)$/ }).click();
    const player = page.getByTestId('tutorial-player');
    for (let step = 1; step < steps; step += 1) {
      await expect(player.getByTestId('tutorial-example')).toBeVisible();
      await player.getByRole('button', { name: 'Continue with example', exact: true }).click();
    }
    await player.getByRole('button', { name: 'Finish tutorial', exact: true }).click();
    await expect(player).toHaveCount(0);
  }
  expect(writes).toEqual([]);
});

test('Help does not describe itself as the current screen', async ({ page }) => {
  await signIn(page);
  await page.goto('/help');
  await expect(page.getByRole('tab', { name: /this screen/i })).toHaveCount(0);
  await expect(page.getByRole('tab', { name: 'Articles', exact: true })).toBeVisible();
});
