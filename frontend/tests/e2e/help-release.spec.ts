/** Decision guidance, exact setting help, and a real-target walk that never opens a port for the reader. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

const ENVIRONMENT = RUNS_ON.sandbox;
const GUIDES = [
  { id: 'how-approvals-work', title: 'How approvals work', key: 'approval.mode' },
  { id: 'let-agents-open-ports', title: 'Let agents open ports without asking', key: 'capability.policy' },
  { id: 'change-a-running-environment', title: 'Change an environment while it runs', key: 'env.resources' },
  { id: 'who-can-see-and-do-what', title: 'Who can see and do what', key: 'sharing.allow_holders' },
];

test.beforeEach(async () => { await scenario(); });

test('each decision guide is searched by its setting and opens with its real-screen tutorial', async ({ page }) => {
  await signIn(page);
  for (const guide of GUIDES) {
    await page.goto('/help');
    await page.getByRole('searchbox', { name: 'Search the help' }).fill(guide.key);
    await page.getByRole('button', { name: new RegExp(`^${guide.title}`) }).click();
    await expect(page.getByRole('heading', { name: guide.title, level: 1 })).toBeVisible();
    await expect(page.getByTestId('help-tutorials').getByRole('button', { name: 'Start tutorial', exact: true }).first()).toBeVisible();
  }
  await page.goto('/help');
  await page.getByRole('searchbox', { name: 'Search the help' }).fill('how do I approve');
  await expect(page.getByRole('button', { name: /^How approvals work/ })).toBeVisible();
  await page.goto('/help');
  await page.getByRole('tab', { name: 'What is new', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Changes included in this build' })).toBeVisible();
});

function businessWrites(page: Page): string[] {
  const writes: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method()) && /\/(?:ports|capabilities|inputs|approvals|bindings)(?:\/|$)/.test(path)) writes.push(`${request.method()} ${path}`);
  });
  return writes;
}

test('a port guide starts beside its actual panel; leaving and resuming preserve the next step without a port mutation', async ({ page }) => {
  await signIn(page);
  await page.goto(`/resources/environments/${ENVIRONMENT}?tab=ports`);
  await expect(page.getByTestId('ports-panel')).toBeVisible();
  const writes = businessWrites(page);
  await page.getByTestId('ports-panel').getByRole('button', { name: 'Let agents open ports without asking', exact: true }).click();
  await page.getByTestId('help-tutorials').getByRole('button', { name: 'Start tutorial', exact: true }).click();
  const player = page.getByTestId('tutorial-player');
  await expect(player.getByText('Step 1 of 4', { exact: true })).toBeVisible();
  await expect(page.getByTestId('tutorial-frame')).toBeVisible();
  await player.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(player.getByText('Step 2 of 4', { exact: true })).toBeVisible();
  await player.getByRole('button', { name: 'Leave tutorial', exact: true }).click();
  await expect(player).toHaveCount(0);
  await page.getByTestId('help-tutorials').getByRole('button', { name: 'Resume tutorial', exact: true }).click();
  await expect(page.getByTestId('tutorial-player').getByText('Step 2 of 4', { exact: true })).toBeVisible();
  expect(writes).toEqual([]);
});

test('a missing real target continues with an example, and a setting entry keeps its authored example', async ({ page }) => {
  await signIn(page);
  await page.goto('/help/how-approvals-work');
  await page.getByTestId('help-tutorials').getByRole('button', { name: 'Start tutorial', exact: true }).click();
  const player = page.getByTestId('tutorial-player');
  await expect(player.getByTestId('tutorial-example')).toBeVisible();
  await expect(player.getByRole('button', { name: 'Continue with example', exact: true })).toBeEnabled();
  await player.getByRole('button', { name: 'Continue with example', exact: true }).click();
  await expect(player.getByText('Step 2 of 3', { exact: true })).toBeVisible();
  await player.getByRole('button', { name: 'Leave tutorial', exact: true }).click();
  await page.goto('/help/setting%3Arequests.expiry_hours');
  await expect(page.getByRole('heading', { name: 'Example value', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Shipped value', exact: true })).toBeVisible();
  await expect(page.getByText('requests.expiry_hours', { exact: true })).toBeVisible();
});

test('an exact thread source link reads its existing setting editor under current authority', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}?setting=collab.attribution`);
  const source = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(source.locator('[data-setting="collab.attribution"]')).toBeVisible();
  await expect(source.locator('[data-setting]')).toHaveCount(1);
});
