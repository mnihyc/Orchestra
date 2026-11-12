/** A profile's saved config keys open from its normal tab and disclose names only. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { PEOPLE, scenario, signIn } from './support';

const CONFIG = { service_tier: 'private-fast-value', model_context_window: 64_000,
  model_reasoning_effort: 'low', model_reasoning_summary: 'concise', web_search: 'indexed',
  features: { local_option: false }, extra_dirs: ['private-path'], 'literal.dot': 'private-literal' };

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`a saved profile names its config keys through its normal preview ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page);
    // Seed the saved object so both its joined opening and direct reads see these keys.
    const login = await page.request.post('/api/v1/auth/login', { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
    expect(login.ok()).toBe(true);
    const session: unknown = await login.json();
    if (typeof session !== 'object' || session === null || !('access_token' in session) || typeof session.access_token !== 'string') throw new Error('The fixture returns an access token');
    const headers = { Authorization: `Bearer ${session.access_token}` };
    const read = await page.request.get(`/api/v1/agents/${RUNS_ON.codexOnLaptop}`, { headers });
    expect(read.ok()).toBe(true);
    const original: unknown = await read.json();
    if (typeof original !== 'object' || original === null || Array.isArray(original) || !('revision' in original) || typeof original.revision !== 'number') throw new Error('The fixture returns an agent revision');
    const saved = await page.request.patch(`/api/v1/agents/${RUNS_ON.codexOnLaptop}`, { headers, data: {
      expected_revision: original.revision, profile: { config: CONFIG, env: { TOKEN: 'private-env' } },
    } });
    expect(saved.ok()).toBe(true);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.goto(`/resources/agents/${RUNS_ON.codexOnLaptop}`);
    // The profile form is now the resource's Settings section.
    await page.getByRole('tab', { name: words.resourceDetails.layout.settings, exact: true }).click();
    const row = page.locator('[data-profile-config-keys]');
    await expect(page.getByText(words.agents.profile.configKeys, { exact: true })).toBeVisible();
    await expect(row.getByText('web_search', { exact: true })).toHaveCount(0);
    const more = row.getByRole('button', { name: words.ui.preview.showAll, exact: true });
    await more.focus();
    await page.keyboard.press('Enter');
    await expect(row.getByText('web_search', { exact: true })).toBeVisible();
    await expect(row.getByText('features.local_option', { exact: true })).toBeVisible();
    await expect(row.getByText('"literal.dot"', { exact: true })).toBeVisible();
    await expect(row).not.toContainText('private-fast-value');
    await expect(row).not.toContainText('private-env');
    await expect(row).not.toContainText('private-path');
    await row.getByRole('button', { name: words.ui.preview.showLess, exact: true }).click();
    await expect(row.getByText('web_search', { exact: true })).toHaveCount(0);
    await page.reload();
    await expect(page.getByRole('tab', { name: words.resourceDetails.layout.settings, exact: true, selected: true })).toBeVisible();
    await row.getByRole('button', { name: words.ui.preview.showAll, exact: true }).click();
    await expect(row.getByText('web_search', { exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`profile-config-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('profile-config-keys', { path: screenshot, contentType: 'image/png' });
  });
}
