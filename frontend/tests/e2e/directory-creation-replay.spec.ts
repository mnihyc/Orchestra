/** The real folder dialog keeps one act through a lost native reply and accepts its receipt. */
import { expect, test, type Page } from '@playwright/test';
import en from '../../src/i18n/en.json';
import zh from '../../src/i18n/zh-CN.json';
import { begin, filesPanel, MAIN, MAIN_ENVIRONMENT, openThread, showView } from './inspector-support';

async function enter(page: Page, locale: 'en' | 'zh-CN', theme: 'light' | 'dark', width: number) {
  await begin();
  await page.addInitScript(({ theme }) => localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' })), { theme });
  await page.setViewportSize({ width, height: 1000 });
  await page.emulateMedia({ colorScheme: theme });
  await openThread(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await page.goto(`/thread/${MAIN}`);
  }
  await showView(page, 'Files');
  const tree = filesPanel(page).getByTestId('file-tree');
  await expect(tree).toBeVisible();
  await tree.getByRole('treeitem', { name: /^src/ }).first().click();
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) test(`directory replay keeps its native act in ${locale}/${theme}/${width}`, async ({ page }, info) => {
  await enter(page, locale, theme, width);
  const words = (locale === 'en' ? en : zh).files;
  const attempts: { key: string | undefined; body: string | null }[] = [];
  await page.route(/\/api\/v1\/environments\/[^/]+\/files\/mkdir(?:\?.*)?$/, async (route) => {
    attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
    if (attempts.length === 2) {
      await route.fulfill({ status: 200, json: { projection: 'receipt', environment_id: MAIN_ENVIRONMENT, path: 'web/src/one-directory' } });
      return;
    }
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    if (attempts.length === 1) await route.abort('connectionfailed');
    else await route.fulfill({ response });
  });
  const tree = filesPanel(page).getByTestId('file-tree');
  const open = async () => {
    await tree.getByRole('treeitem', { name: /^src/ }).first().click({ button: 'right' });
    await page.getByRole('menuitem', { name: words.menu.newFolder, exact: true }).click();
    return page.getByRole('dialog', { name: words.name.newFolder, exact: true });
  };
  const dialog = await open();
  await dialog.getByRole('textbox', { name: words.name.name, exact: true }).fill('one-directory');
  await dialog.getByRole('button', { name: words.name.confirm.create, exact: true }).click();
  await expect(dialog.getByRole('alert')).toBeVisible();
  await expect(dialog.getByRole('textbox', { name: words.name.name, exact: true })).toHaveValue('one-directory');
  const retry = dialog.getByRole('button', { name: words.name.confirm.create, exact: true });
  await retry.focus();
  await page.keyboard.press('Enter');
  await expect(dialog).toHaveCount(0);
  await expect(tree.getByRole('treeitem', { name: /^one-directory/ })).toBeVisible();
  expect(attempts).toHaveLength(2);
  expect(attempts[0]?.key).toMatch(/^[a-f0-9-]{36}$/i);
  expect(attempts[1]).toEqual(attempts[0]);
  const sent: unknown = JSON.parse(attempts[0]?.body ?? 'null');
  expect(sent).toEqual({ path: 'web/src/one-directory' });
  const next = await open();
  await next.getByRole('textbox', { name: words.name.name, exact: true }).fill('another-directory');
  await next.getByRole('button', { name: words.name.confirm.create, exact: true }).click();
  await expect(next).toHaveCount(0);
  await expect(tree.getByRole('treeitem', { name: /^another-directory/ })).toBeVisible();
  expect(attempts).toHaveLength(3);
  expect(attempts[2]?.key).toMatch(/^[a-f0-9-]{36}$/i);
  expect(attempts[2]?.key).not.toBe(attempts[0]?.key);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await info.attach('directory-replay', { body: await page.screenshot(), contentType: 'image/png' });
});
