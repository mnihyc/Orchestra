/** Resource identities and their native actions fit the available table box, including a newly created unit. */
import { createInstance } from 'i18next';
import { expect, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { detailMount, detailUnitProvider } from '../component/resource-detail-fixtures';
import { inventoryRow } from '../doubles/everything-row';
import { scenario, signIn } from './support';

const longMountName = 'shared-notes-for-team-research';
const mount = { ...detailMount, name: 'shared-notes', description: 'World 2: shared notes for all environments and all the researchers on this team', locations: [], locations_hidden: 0 };
const longMount = { ...mount, id: 'long-mount', name: longMountName };
const provider = { ...detailUnitProvider, name: 'A model connection with a long descriptive name on a newly created unit' };

async function expectTableFits(page: Page, label: string) {
  const table = page.getByRole('table', { name: label, exact: true }); await expect(table).toBeVisible();
  await page.evaluate(async () => { await document.fonts.ready; });
  const bounds = await table.evaluate((element) => {
    const region = element.closest<HTMLElement>('[data-scroll-allow="table"]'); if (region === null) throw new Error('The table needs its native named region');
    const box = region.getBoundingClientRect();
    const scale = region.offsetWidth === 0 ? 1 : box.width / region.offsetWidth;
    const left = box.left + region.clientLeft * scale, room = region.clientWidth * scale;
    return { width: element.getBoundingClientRect().width, room, left, right: left + room };
  });
  expect(bounds.width).toBeLessThanOrEqual(bounds.room + 1);
  for (const button of await table.getByRole('button').all()) {
    const box = await button.boundingBox(); if (box === null) throw new Error('A row action must remain visible');
    expect(box.x).toBeGreaterThanOrEqual(bounds.left - 1); expect(box.x + box.width).toBeLessThanOrEqual(bounds.right + 1);
  }
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  return table;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`mount and provider names keep their actions inside the table (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await signIn(page);
    const words = createInstance(); await words.init({ lng: locale, fallbackLng: false, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
    const t = words.getFixedT(locale);
    await page.goto('/settings/interface'); await page.getByRole('radio', { name: theme === 'dark' ? 'Dark' : 'Light', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    if (locale === 'zh-CN') {
      await page.getByRole('combobox', { name: 'Language', exact: true }).click(); await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    await page.route('**/api/v1/everything?*', async (route) => {
      const params = new URL(route.request().url()).searchParams;
      if (params.get('resource_details') !== 'true') { await route.continue(); return; }
      const kinds = params.get('kinds');
      const items = kinds === 'mount' ? [mount, longMount].map((value) => inventoryRow({ id: value.id, kind: 'mount', name: value.name, owner: value.owner, my_access: 'manage', resource: { kind: 'mount', value } }))
        : kinds === 'provider' ? [inventoryRow({ id: provider.id, kind: 'provider', name: provider.name, owner: provider.owner, my_access: 'manage', resource: { kind: 'provider', value: provider } })] : null;
      if (items === null) { await route.continue(); return; }
      const response = await route.fetch(); await route.fulfill({ response, json: { items, next_cursor: null } });
    });
    await page.goto('/resources/mounts'); const mounts = await expectTableFits(page, t('screens.mounts.title'));
    const shortName = mounts.getByRole('link', { name: mount.name, exact: true });
    expect(await shortName.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    const fullName = mounts.getByRole('link', { name: longMount.name, exact: true }); await fullName.focus();
    await expect(page.getByRole('tooltip')).toContainText(longMount.name);
    const edit = mounts.getByRole('button', { name: t('resources.act.of', { act: t('resources.act.editButton'), name: mount.name }) });
    await expect(edit).toBeInViewport(); await info.attach(`mounts-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await page.goto('/resources/providers'); const providers = await expectTableFits(page, t('providers.list.label'));
    await expect(providers).not.toContainText('9fca895b060d');
    const open = providers.getByRole('button', { name: t('resources.act.of', { act: t('resources.act.open'), name: provider.name }) });
    const menu = providers.getByRole('button', { name: t('resources.act.menuOf', { name: provider.name }) });
    await expect(open).toBeInViewport(); await expect(menu).toBeInViewport();
    const name = providers.getByRole('link', { name: provider.name, exact: true }); await name.focus(); await expect(page.getByRole('tooltip')).toContainText(provider.name);
    await menu.click(); await expect(page.getByRole('menuitem', { name: t('resources.act.edit'), exact: true })).toBeVisible(); await page.keyboard.press('Escape');
    await info.attach(`providers-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    if (width === 1440) {
      await page.evaluate(() => { document.documentElement.style.zoom = '2'; }); await expectTableFits(page, t('providers.list.label'));
      await expect(open).toBeInViewport(); await expect(menu).toBeInViewport();
    }
  });
}
