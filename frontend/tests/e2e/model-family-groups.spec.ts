/** A real vendor family proposal enters the existing group editor and is written only on Save. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id } from '../doubles/fixtures';
import { scenario, signIn } from './support';
const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const PROVIDER = id(0x22, 1);
const MODEL = 'family-evidence-model';
async function control(path: string, body: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fixture refused ${path}: ${response.status}`);
}
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`vendor family groups ${language}/${theme}`, async ({ page }, info) => {
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
      await control('/model/entry', { provider_id: PROVIDER, model_name: MODEL, display_name: 'Family proposal model', groups: ['kept'], parameters: {} });
      await control('/resources/model-observation', { provider_id: PROVIDER, at: '2026-11-08T07:00:00Z', models: [{
        model_name: MODEL, display_name: 'Family proposal model', listed: true,
        parameters: { offered: { report: { line: 'vendor-family' } } }, origins: { '/parameters/offered/report/line': 'vendor' },
      }] });
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/resources/providers/${PROVIDER}?tab=models`);
      const row = page.getByRole('row').filter({ has: page.getByText('Family proposal model', { exact: true }) });
      const writes: string[] = [];
      const watch = (request: import('@playwright/test').Request) => {
        if (request.method() === 'PATCH' && new URL(request.url()).pathname.startsWith(`/api/v1/providers/${PROVIDER}/models/`)) writes.push(request.url());
      };
      page.on('request', watch);
      const act = row.getByRole('button', { name: words.providerFill.groupProposal.replace('{{name}}', 'vendor-family'), exact: true });
      await act.click();
      const dialog = page.getByRole('dialog', { name: words.models.editor.title, exact: true });
      const groups = dialog.getByRole('list', { name: words.models.groups.of, exact: true });
      await expect(groups).toContainText('kept'); await expect(groups).toContainText('vendor-family');
      expect(writes).toEqual([]);
      await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
      await expect(dialog).toHaveCount(0);
      expect(writes).toEqual([]);
      await act.click();
      const editor = page.getByRole('dialog', { name: words.models.editor.title, exact: true });
      const box = await editor.boundingBox();
      if (box === null) throw new Error('the actual group editor is not visible');
      expect(box.x).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`family-group-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      const sent = page.waitForRequest((request) => request.method() === 'PATCH' && new URL(request.url()).pathname.startsWith(`/api/v1/providers/${PROVIDER}/models/`));
      const acknowledged = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.startsWith(`/api/v1/providers/${PROVIDER}/models/`));
      await editor.getByRole('button', { name: words.providers.models.save, exact: true }).click();
      expect((await sent).postDataJSON()).toMatchObject({ groups: ['kept', 'vendor-family'] });
      expect((await acknowledged).status()).toBe(200);
      await expect(editor).toHaveCount(0);
      await expect(row.getByRole('list', { name: words.models.groups.of, exact: true })).toContainText('vendor-family');
      expect(writes).toHaveLength(1);
      page.off('request', watch);
    }
  });
}
