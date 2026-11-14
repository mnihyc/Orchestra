/** The forwarded program choice uses the existing model dialog and the backend's enabled choices. */
import { expect, test } from '@playwright/test';
import type { NodeSettingSetBody, ParameterView, ThreadParameters } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN } from './inspector-support';
import { signIn } from './support';

const KEY = 'cyber_access_program';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`an API program choice persists and disappears when no longer enabled ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    let enabled = true;
    let choice: string | null = null;
    let writes = 0;
    const project = (parameters: ThreadParameters): ThreadParameters => ({ ...parameters,
      model: { ...parameters.model, options: parameters.model.options.map((model) => ({ ...model, parameters: {
        ...model.parameters, offered: { ...model.parameters.offered, access_programs: ['daybreakBlue'] },
        access_programs: enabled ? ['daybreakBlue'] : [], default_access_program: null,
      } })) },
      parameters: [...parameters.parameters.filter((row) => {
        const def = row.def;
        const options = typeof def === 'object' && def !== null && 'options' in def ? def.options : null;
        const isProgram = typeof options === 'object' && options !== null && 'from_model' in options && options.from_model === 'access_programs';
        return row.key !== KEY && !isProgram;
      }), ...(enabled ? [{
        key: KEY, def: { key: KEY, title: 'Access program', type: 'enum', options: { from_model: 'access_programs' } },
        value: choice, apply: 'next_turn', source: { scope: choice === null ? 'default' : 'thread', node: null }, locked_by: null, writable: true,
      } satisfies ParameterView] : [])],
    });
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const parameters = await response.json() as ThreadParameters;
      await route.fulfill({ response, json: project(parameters) });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening*`, async (route) => {
      const response = await route.fetch();
      const body = await response.json() as { parameters: ThreadParameters | null };
      await route.fulfill({ response, json: { ...body, parameters: body.parameters === null ? null : project(body.parameters) } });
    });
    await page.route((url) => url.pathname === `/api/v1/nodes/${MAIN}/settings/agent.params`, async (route) => {
      if (route.request().method() !== 'PUT') { await route.continue(); return; }
      const body = route.request().postDataJSON() as NodeSettingSetBody;
      expect(body.value).toMatchObject({ [KEY]: 'daybreakBlue' });
      expect(typeof body.expected_revision).toBe('number');
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      choice = 'daybreakBlue';
      writes += 1;
      await route.fulfill({ response });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${MAIN}`);
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const prefix = words.composer.model.change.split('{{model}}')[0] ?? '';
    const model = page.getByRole('button', { name: new RegExp(prefix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')) }).first();
    await model.click();
    const dialog = page.getByRole('dialog', { name: words.composer.model.title, exact: true });
    const access = dialog.getByRole('combobox', { name: words.composer.model.access.title, exact: true });
    await access.focus();
    await page.keyboard.press('Enter');
    const blue = locale === 'en' ? 'Daybreak Blue' : 'Daybreak 蓝';
    await page.getByRole('option', { name: blue, exact: true }).click();
    await expect.poll(() => writes).toBe(1);
    await expect(access).toHaveText(blue);
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(model).toBeFocused();
    await page.reload();
    await model.click();
    await expect(access).toHaveText(blue);
    expect(writes).toBe(1);
    await page.keyboard.press('Escape');
    enabled = false;
    await page.reload();
    await model.click();
    await expect(dialog.getByRole('combobox', { name: words.composer.model.access.title, exact: true })).toHaveCount(0);
    expect(writes).toBe(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('forwarded-access-program', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
