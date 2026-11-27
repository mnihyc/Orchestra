/** The forwarded program choice uses the existing model dialog and the backend's enabled choices. */
import { expect, test } from '@playwright/test';
import type { NodeSettingSetBody, ParameterView, ThreadParameters } from '../../src/generated/api-types';
import { SPEED_DEF } from '../doubles/model';
import { language } from './ask-support';
import { begin, MAIN } from './inspector-support';

const KEY = 'cyber_access_program';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`an API program choice persists and disappears when no longer enabled ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    let enabled = true;
    let choice: string | null = null;
    let writes = 0;
    const project = (parameters: ThreadParameters): ThreadParameters => ({ ...parameters,
      model: { ...parameters.model, options: parameters.model.options.map((model) => ({ ...model, parameters: {
        ...model.parameters, offered: { ...model.parameters.offered, access_programs: ['daybreakBlue'] },
        service_tiers: [{ id: 'fast', label: 'Fast' }],
        access_programs: enabled ? ['daybreakBlue'] : [], default_access_program: null,
      } })) },
      parameters: [...parameters.parameters.filter((row) => {
        const def = row.def;
        const options = typeof def === 'object' && def !== null && 'options' in def ? def.options : null;
        const isProgram = typeof options === 'object' && options !== null && 'from_model' in options && options.from_model === 'access_programs';
        return row.key !== KEY && row.key !== SPEED_DEF.key && !isProgram;
      }), {
        key: SPEED_DEF.key, def: SPEED_DEF, value: null, apply: 'next_turn',
        source: { scope: 'default', node: null }, locked_by: null, writable: true,
      } satisfies ParameterView, ...(enabled ? [{
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
    const t = await language(page, request, locale, theme);
    await page.goto(`/thread/${MAIN}`);
    const row = page.getByRole('group', { name: t('composer.row.label'), exact: true });
    const access = row.locator('[data-composer-chip="access"] button');
    const model = row.locator('[data-composer-chip="model"] button');
    const subject = t('composer.model.access.title');
    const label = (value: string) => t('composer.chip.value', { what: subject, value });
    const none = label(t('composer.model.access.none'));
    const expectOnModelRow = async (words: string) => {
      await expect(access).toHaveAccessibleName(words);
      await expect(access).toHaveText(words, { useInnerText: true });
      await expect(async () => {
        const [m, a, r] = await Promise.all([model.boundingBox(), access.boundingBox(), row.boundingBox()]);
        if (!m || !a || !r) throw new Error('The model and access controls must both be laid out');
        expect(Math.abs(a.y - m.y)).toBeLessThanOrEqual(1);
        expect(a.x).toBeGreaterThanOrEqual(m.x + m.width - 1);
        expect(a.x + a.width).toBeLessThanOrEqual(r.x + r.width + 1);
      }).toPass();
    };
    await expect(row.locator('[data-composer-chip="speed"] button')).toBeVisible();
    await expectOnModelRow(none);
    const caption = access.getByText(none, { exact: true });
    expect(await caption.evaluate((text) => text.scrollWidth <= text.clientWidth + 1)).toBe(true);
    await access.click();
    const list = page.getByRole('dialog', { name: subject, exact: true });
    const blue = t('composer.model.access.names.daybreakBlue');
    await list.getByRole('menuitemradio', { name: blue, exact: true }).click();
    await expect.poll(() => writes).toBe(1);
    await expectOnModelRow(label(blue));
    await expect(list).toHaveCount(0);
    await page.reload();
    await expectOnModelRow(label(blue));
    expect(writes).toBe(1);
    enabled = false;
    await page.reload();
    await expect(model).toBeVisible();
    await expect(access).toHaveCount(0);
    expect(writes).toBe(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    await info.attach('forwarded-access-program', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
