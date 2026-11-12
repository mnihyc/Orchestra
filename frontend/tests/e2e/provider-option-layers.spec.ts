/** Click through the shipping endpoint/group editors in both languages, widths and themes. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id } from '../doubles/fixtures';
import { bodySent, replySent, scenario, signIn } from './support';

const PROVIDER = id(0x22, 1);
const providerPath = `/api/v1/providers/${PROVIDER}`;

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`provider option layers ${language}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    const groupTitle = words.models.groups.optionsTitle.replace('{{name}}', 'frontier');
    for (const width of [1280, 400]) {
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/resources/providers/${PROVIDER}?tab=models`);
      const groups = page.getByRole('region', { name: words.models.groups.title, exact: true });
      const frontier = groups.getByRole('listitem').filter({ hasText: 'frontier' });
      await frontier.getByRole('button', { name: words.models.groups.options, exact: true }).click();
      const dialog = page.getByRole('dialog', { name: groupTitle });
      const field = dialog.getByRole('textbox', { name: words.providerOptions.schemaTitle, exact: true });
      const schema = { 'openai-responses': { type: 'object', properties: { temperature: { type: 'number', maximum: width === 400 ? 0.4 : 0.5 } } } };
      await field.fill('{bad');
      await expect(dialog.getByRole('button', { name: words.resources.edit.save, exact: true })).toHaveAttribute('aria-disabled', 'true');
      await field.fill(JSON.stringify(schema));
      const guide = dialog.getByRole('button', { name: words.models.groups.optionsGuide, exact: true });
      await expect(guide).toHaveAttribute('data-help-heading', 'request-options');
      const box = await dialog.boundingBox();
      if (box === null) throw new Error('the actual group editor is not visible');
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.x + box.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`group-options-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      // The native write reply is authoritative; the editor reopens that acknowledged value.
      const refreshed = page.waitForResponse((response) => response.request().method() === 'PATCH'
        && new URL(response.url()).pathname === providerPath && response.status() === 200).then(replySent);
      const write = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname === providerPath);
      await dialog.getByRole('button', { name: words.resources.edit.save, exact: true }).click();
      expect(await bodySent(await write)).toMatchObject({ model_group_options: { frontier: { request_options: schema } } });
      expect(await refreshed).toMatchObject({ model_group_options: { frontier: { request_options: schema } } });
      await expect(dialog).toHaveCount(0);
      await frontier.getByRole('button', { name: words.models.groups.options, exact: true }).click();
      const again = page.getByRole('dialog', { name: groupTitle });
      expect(JSON.parse(await again.getByRole('textbox', { name: words.providerOptions.schemaTitle, exact: true }).inputValue())).toEqual(schema);
      await again.getByRole('button', { name: words.common.cancel, exact: true }).click();

      // The same model editor keeps its own declaration and the actual membership control.
      const modelRow = page.getByRole('row').filter({ has: page.getByText('GPT-5.4', { exact: true }) });
      await modelRow.locator('button[aria-haspopup="menu"]').last().click();
      await page.getByRole('menuitem', { name: words.models.list.edit, exact: true }).click();
      const modelEditor = page.getByRole('dialog', { name: words.models.editor.title, exact: true });
      // Documented controls lead; the unchanged full schema remains behind Raw JSON.
      const modelOptions = modelEditor.getByRole('group', { name: words.providerOptions.schemaTitle, exact: true });
      await modelOptions.locator('summary').filter({ hasText: words.providerFields.raw }).click();
      await expect(modelOptions.getByRole('textbox', { name: words.providerFields.raw, exact: true })).toBeVisible();
      await expect(modelEditor.getByRole('button', { name: words.models.groups.optionsGuide, exact: true })).toHaveAttribute('data-help-heading', 'request-options');
      const addGroup = modelEditor.getByLabel(words.models.groups.add, { exact: true });
      await addGroup.fill(`screen-${width}`); await addGroup.press('Enter');
      await expect(modelEditor.getByRole('list', { name: words.models.groups.of, exact: true })).toContainText(`screen-${width}`);
      await modelEditor.getByRole('button', { name: words.common.cancel, exact: true }).click();

      // Endpoint declarations use the same manager field through its actual descriptor.
      await page.goto(`/resources/providers/${PROVIDER}?tab=key`);
      const endpoints = page.getByRole('region', { name: words.providers.key.endpoints, exact: true });
      const advanced = endpoints.locator('details').first();
      if (await advanced.getAttribute('open') === null) await advanced.locator('summary').click();
      const endpointOptions = endpoints.locator('[data-option-schema-editor]');
      const raw = endpointOptions.locator('details');
      if (await raw.getAttribute('open') === null) await raw.locator('summary').click();
      const options = endpointOptions.getByRole('textbox', { name: words.providerFields.raw, exact: true });
      await options.fill(JSON.stringify(schema));
      const endpointWrite = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname === providerPath);
      const endpointReply = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname === providerPath);
      await endpoints.getByRole('button', { name: words.providers.key.saveEndpoints, exact: true }).click();
      expect(await bodySent(await endpointWrite)).toMatchObject({ upstreams: [{ options: { request_options: schema } }] });
      const accepted = await endpointReply;
      expect(accepted.status()).toBe(200);
      expect(await replySent(accepted)).toMatchObject({ upstreams: [{ options: { request_options: schema } }], model_group_options: { frontier: { request_options: schema } } });
      await expect(endpoints.getByRole('button', { name: words.providers.key.saveEndpoints, exact: true })).not.toHaveAttribute('aria-busy', 'true');
      await info.attach(`endpoint-options-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
