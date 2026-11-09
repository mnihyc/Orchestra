/** Review a provisional format/catalogue proposal through the real creation dialog before its only write. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`provider key proposal ${language}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/resources/providers');
      await page.locator('[data-resource-kind="providers"]').getByRole('button', { name: words.providers.list.add, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: words.providers.add.title, exact: true });
      await dialog.getByRole('combobox', { name: words.resources.home.label, exact: true }).click();
      await page.getByRole('option', { name: 'Acme', exact: true }).click();
      const key = `fixture-model-private-${width}`;
      const requests: { method: string; url: string; body: string | null }[] = [];
      const record = (request: import('@playwright/test').Request) => {
        if (new URL(request.url()).pathname.startsWith('/api/v1/')) requests.push({ method: request.method(), url: request.url(), body: request.postData() });
      };
      page.on('request', record);
      const field = dialog.getByLabel(words.providerFill.key, { exact: true });
      await field.fill(key);
      await expect(field).toHaveAttribute('type', 'password');
      await expect(dialog.getByTestId('credential-proposal')).toContainText(words.providerFill.sources.format);
      await dialog.getByRole('button', { name: words.providerFill.use.replace('{{name}}', 'OpenAI'), exact: true }).click();
      await expect(dialog.getByLabel(words.resources.endpoint.address, { exact: true })).toHaveValue('https://api.openai.com/v1');
      await expect(dialog.getByText(words.providerFill.sources.catalogue, { exact: true })).toBeVisible();
      // Explicit overrides stay selected as the proposed key changes, and the body keeps the edited endpoint.
      await dialog.getByRole('combobox', { name: words.providerFill.role, exact: true }).click();
      await page.getByRole('option', { name: words.providerFill.adminKey, exact: true }).click();
      await dialog.getByRole('combobox', { name: words.providerFill.role, exact: true }).click();
      await page.getByRole('option', { name: words.providerFill.modelKey, exact: true }).click();
      await field.fill(`fixture-admin-private-${width}`);
      await expect(dialog.getByRole('combobox', { name: words.providerFill.role, exact: true })).toHaveText(words.providerFill.modelKey);
      await dialog.getByLabel(words.resources.endpoint.address, { exact: true }).fill('https://chosen.example.test/v1');
      await dialog.getByRole('button', { name: words.common.next, exact: true }).click();
      const review = dialog.getByRole('region', { name: words.providerFill.review, exact: true });
      await expect(review).toContainText(words.providerFill.modelKey);
      await expect(review).toContainText(words.providerFill.sources.choice);
      await expect(review).not.toContainText('private-');
      const name = `Reviewed account ${width}`;
      await dialog.getByLabel(words.providers.add.name, { exact: true }).fill(name);
      await dialog.getByRole('button', { name: words.common.back, exact: true }).click();
      await expect(dialog.getByLabel(words.providerFill.key, { exact: true })).toHaveValue(`fixture-admin-private-${width}`);
      await expect(dialog.getByLabel(words.resources.endpoint.address, { exact: true })).toHaveValue('https://chosen.example.test/v1');
      await dialog.getByRole('button', { name: words.common.next, exact: true }).click();
      await expect(dialog.getByLabel(words.providers.add.name, { exact: true })).toHaveValue(name);
      await expect(dialog.locator('[data-help-guide="add-a-provider"]')).toHaveAttribute('data-help-heading', 'key-proposals');
      expect(requests.some((request) => request.method === 'POST' && new URL(request.url).pathname === '/api/v1/providers')).toBe(false);
      expect(requests.some((request) => request.url.includes('private-') || request.body?.includes('private-') === true)).toBe(false);
      const bounds = await dialog.boundingBox();
      if (bounds === null) throw new Error('the actual proposal dialog is not visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`provider-key-review-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      const sent = page.waitForRequest((request) => request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/providers');
      const acknowledged = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/v1/providers');
      await dialog.getByRole('button', { name: words.providerCreation.create, exact: true }).click();
      expect((await sent).postDataJSON()).toMatchObject({ kind: 'api', name, api_key: `fixture-admin-private-${width}`,
        upstreams: [{ protocol: 'openai-responses', base_url: 'https://chosen.example.test/v1' }] });
      expect((await acknowledged).status()).toBe(200);
      await expect(dialog).toHaveCount(0);
      page.off('request', record);
    }
  });
}
