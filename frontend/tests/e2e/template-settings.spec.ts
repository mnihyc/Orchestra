/** Compact template creation and searchable shared settings at both widths, languages and themes. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';
import { templateSection } from './template-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`a short creation opens searchable settings and keeps Make here (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const words = locale === 'en' ? english : chinese;
    const name = `Web ${locale} ${theme} ${width}`;
    // Chromium omits streamed Request bodies in its network observer. Read a
    // clone before transport; send the original fetch arguments unchanged.
    await page.addInitScript(() => {
      const observed = window as typeof window & { __orchTemplateWrites: unknown[] };
      observed.__orchTemplateWrites = [];
      const send = window.fetch.bind(window);
      window.fetch = (input, init) => {
        if (input instanceof Request && input.method === 'POST' && new URL(input.url).pathname === '/api/v1/templates') {
          void input.clone().json().then((body: unknown) => { observed.__orchTemplateWrites.push(body); }, () => { observed.__orchTemplateWrites.push('Unreadable JSON'); });
        }
        return send(input, init);
      };
    });
    const writes: string[] = [];
    const reads: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (request.method() === 'POST' && path === '/api/v1/templates') writes.push(request.method());
      if (path.startsWith('/api/v1/')) reads.push(`${request.method()} ${path}`);
    });
    await page.goto('/resources/templates');
    await page.getByRole('button', { name: words.templates.list.add, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.templates.new.title, exact: true });
    const image = dialog.getByRole('textbox', { name: words.templates.fields.image.title, exact: true });
    await expect(image).toHaveValue('node:22-bookworm');
    await expect(dialog.getByRole('searchbox')).toHaveCount(0);
    await expect(dialog.getByRole('spinbutton')).toHaveCount(0);
    await expect(dialog.getByRole('switch', { name: 'JSON', exact: true })).toHaveCount(0);
    await expect(dialog.locator('[data-help-article="write-an-environment-template"]')).toHaveCount(1);
    await dialog.getByRole('textbox', { name: words.templates.new.name, exact: true }).fill(name);
    await expect(dialog.getByRole('button', { name: words.templates.new.submit, exact: true })).toBeInViewport();
    // A starter fits in one viewport rather than mounting the eleven-thousand-pixel editor.
    expect(await dialog.locator('form').evaluate((form) => form.scrollHeight <= window.innerHeight)).toBe(true);
    const createImage = info.outputPath(`template-create-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: createImage, fullPage: true, animations: 'disabled' });
    await info.attach(`template-create-${locale}-${theme}-${width}.png`, { path: createImage, contentType: 'image/png' });
    await dialog.getByRole('button', { name: words.templates.new.submit, exact: true }).click();
    await expect(page.getByRole('heading', { name, level: 1, exact: true })).toBeVisible();
    expect(writes).toEqual(['POST']);
    expect(await page.evaluate(() => (window as typeof window & { __orchTemplateWrites: unknown[] }).__orchTemplateWrites)).toEqual([expect.objectContaining({ name, spec: expect.objectContaining({ image: 'node:22-bookworm', resources: expect.objectContaining({ cpu_millis: 4000, memory_mb: 4096 }) }) })]);
    await page.getByRole('tab', { name: words.resourceDetails.layout.settings, exact: true }).click();
    const settings = page.locator('[data-template-settings]');
    const search = settings.getByRole('searchbox', { name: words.templateSettings.search, exact: true });
    await expect(search).toBeVisible();
    await expect(page.getByText(words.templates.editor.valid, { exact: true })).toBeVisible();
    const defaultPrefix = words.templates.editor.default.split('{{value}}')[0] ?? '';
    await expect(settings.getByText(new RegExp(`^${defaultPrefix}`))).toHaveCount(0);
    await expect(settings.getByRole('menuitem', { name: words.templates.editor.reset, exact: true })).toHaveCount(0);
    const before = reads.length;
    await search.fill(words.templates.fields.resources.memory_mb.title);
    await templateSection(page, words.templates.editor.sections.uses);
    const limits = page.getByRole('group', { name: words.templates.fields.resources.title, exact: true });
    const memory = limits.getByRole('spinbutton', { name: words.templates.fields.resources.memory_mb.title, exact: true });
    await expect(memory).toHaveValue('4096');
    await expect(settings.getByRole('textbox', { name: words.templates.fields.image.title, exact: true })).toHaveCount(0);
    expect(reads).toHaveLength(before);
    const row = memory.locator('xpath=ancestor::*[@data-setting-row][1]');
    await expect(row.locator('[data-template-source]')).toHaveText(words.templateSettings.source.template);
    const menu = words.settings.editor.menuOf.replace('{{name}}', words.templates.fields.resources.memory_mb.title);
    await row.getByRole('button', { name: menu, exact: true }).click();
    await expect(page.getByRole('menuitem', { name: words.templates.editor.reset, exact: true })).toBeVisible();
    await page.getByRole('menuitem', { name: words.templates.editor.reset, exact: true }).click();
    await expect(memory).toHaveValue('');
    await expect(row.locator('[data-template-source]')).toHaveText(words.templateSettings.source.automatic);
    await memory.fill('3072');
    await search.fill('');
    await templateSection(page, words.templates.editor.sections.uses);
    await expect(memory).toHaveValue('3072');
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    const settingsImage = info.outputPath(`template-settings-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: settingsImage, fullPage: true, animations: 'disabled' });
    await info.attach(`template-settings-${locale}-${theme}-${width}.png`, { path: settingsImage, contentType: 'image/png' });
    await page.getByRole('button', { name: words.templates.spec.save, exact: true }).click();
    await expect(page.getByText(words.templates.spec.saved.replace('{{name}}', name), { exact: true })).toBeVisible();
    await page.getByRole('button', { name: words.environments.make.here, exact: true }).click();
    const make = page.getByRole('dialog', { name: words.environments.make.title, exact: true });
    await expect(make.getByRole('combobox', { name: words.environments.make.template, exact: true })).toContainText(name);
    await expect(make.getByRole('switch', { name: words.temporary.name, exact: true })).not.toBeChecked();
    await make.getByRole('switch', { name: words.temporary.name, exact: true }).click();
    await expect(make.getByRole('switch', { name: words.temporary.name, exact: true })).toBeChecked();
  });
}
