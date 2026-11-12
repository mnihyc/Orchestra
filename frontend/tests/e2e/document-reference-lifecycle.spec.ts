/** Click paths for the live editor's reference context, in both languages, widths and themes. */
import { expect, type Page, test } from '@playwright/test';
import { PEOPLE, scenario, signIn } from './support';
import { DOCS, held, waitUntilSaved } from './documents-support';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  const body = locale === 'en' ? 'Document text' : '文档正文';
  test(`a committed editor inserts its actual reference and retains it after reopening in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    await page.goto(`/doc/${DOCS.plan}`);
    const editor = page.getByRole('textbox', { name: body });
    await expect(editor).toBeVisible();
    await editor.getByText('Ship the redirect fix', { exact: false }).first().click();
    await page.keyboard.press('End'); await page.keyboard.type(' [[Notes');
    const menu = page.getByRole('listbox');
    await menu.getByRole('option', { name: /Notes from the canary/ }).click();
    await expect(editor).toContainText('Notes from the canary'); await waitUntilSaved(page);
    await expect.poll(async () => (await held(DOCS.plan)).blocks.some((block) => block.content.some((run) => run.type === 'ref'))).toBe(true);
    await page.reload();
    await expect(page.getByRole('textbox', { name: body })).toContainText('Notes from the canary');
  });

  test(`a reader cannot open writing suggestions or change native blocks in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera); await language(page, locale);
    await page.goto(`/doc/${DOCS.plan}`);
    const editor = page.getByRole('textbox', { name: body });
    await expect(editor).toHaveAttribute('contenteditable', 'false');
    const before = (await held(DOCS.plan)).blocks;
    await editor.click(); await page.keyboard.type(' [[Notes');
    await page.waitForLoadState('networkidle');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    expect((await held(DOCS.plan)).blocks).toEqual(before);
  });
}
