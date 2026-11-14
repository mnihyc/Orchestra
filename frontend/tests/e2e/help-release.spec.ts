import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
/** Decision guidance, exact setting help, and a real-target walk that never opens a port for the reader. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

const ENVIRONMENT = RUNS_ON.workspace;
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
  await expect(page.getByRole('heading', { name: english.helpRelease.news.version, exact: true })).toBeVisible();
});

function businessWrites(page: Page): string[] {
  const writes: string[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method()) && /\/(?:ports|capabilities|inputs|approvals|bindings)(?:\/|$)/.test(path)) writes.push(`${request.method()} ${path}`);
  });
  return writes;
}

async function expectSettingKey(page: Page, key: string, label = english.helpRelease.copyKey): Promise<void> {
  await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
  // Registry identifiers are available through the copy action rather than body text.
  await page.getByRole('button', { name: label, exact: true }).click();
  await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(key);
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
  // Decision walks require an explicit owned target, even when started beside it.
  await player.getByRole('combobox', { name: 'Choose one of your environments' }).click();
  await page.getByRole('option', { name: /^Workspace(?: ·|$)/ }).click();
  await expect(page.getByTestId('tutorial-frame')).toBeVisible();
  await player.getByRole('button', { name: 'Next', exact: true }).click();
  await expect(player.getByText('Step 2 of 4', { exact: true })).toBeVisible();
  await player.getByRole('button', { name: 'Leave tutorial', exact: true }).click();
  await expect(player).toHaveCount(0);
  await page.getByTestId('ports-panel').getByRole('button', { name: 'Let agents open ports without asking', exact: true }).click();
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
  await page.goto('/help/setting%3Arequests.expiry_minutes');
  await expect(page.getByRole('heading', { name: 'Example value', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Shipped value', exact: true })).toBeVisible();
  await expectSettingKey(page, 'requests.expiry_minutes');
});

test('an exact thread source link reads its existing setting editor under current authority', async ({ page }) => {
  await signIn(page);
  await page.goto(`/thread/${ACME.mainThread}?setting=collab.attribution`);
  const source = page.getByRole('dialog', { name: 'Settings', exact: true });
  await expect(source.locator('[data-setting="collab.attribution"]')).toBeVisible();
  await expect(source.locator('[data-setting]')).toHaveCount(1);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`finds the native offline turn deadline help (${locale}/${theme}/${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const { title, description } = (locale === 'en' ? english : chinese).settingRegistry.threads.offline_deadline;
    await page.goto('/help');
    await page.getByRole('searchbox', { name: locale === 'en' ? 'Search the help' : '搜索帮助' }).fill('threads.offline_deadline');
    await page.getByRole('button', { name: new RegExp(`^${title}`) }).click();
    await expect(page.getByRole('article', { name: title, exact: true }).getByRole('heading', { name: title, level: 2, exact: true })).toBeVisible();
    await expect(page.getByText(description, { exact: true })).toBeVisible();
    await expectSettingKey(page, 'threads.offline_deadline', (locale === 'en' ? english : chinese).helpRelease.copyKey);
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`finds the running-thread guide in its authored work group (${locale}/${theme}/${width})`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    const title = locale === 'en' ? 'Follow a running thread' : '关注正在运行的线程';
    await page.goto('/help');
    await page.getByRole('tab', { name: new RegExp(`^${words.help.parts.articles}`) }).click();
    const group = page.getByRole('heading', { name: words.help.articleGroups.work, exact: true }).locator('..');
    await group.getByRole('button', { name: new RegExp(`^${title}`) }).click();
    await expect(page.getByRole('heading', { name: title, exact: true, level: 1 })).toBeVisible();
  });
}
