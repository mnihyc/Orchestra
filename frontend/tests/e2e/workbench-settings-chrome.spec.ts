/** Settings share one navigation column; native menus, view choices and shortcuts remain compact and reachable. */
import { expect, type Page, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

test.beforeEach(async () => { await scenario(); });

for (const width of [1100, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`each place keeps global navigation and one current settings list in its page at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const toggle = () => width === 400 ? page.locator('[data-container-choice="left:navigation"]') : page.getByRole('button', { name: /^(Left bar|左栏)$/, exact: true });
    for (const path of ['/org/acme/settings/general', '/org/acme.research/settings/members', `/project/${ACME.web}/settings?section=tags`, `/topic/${ACME.fixLogin}?tab=settings&section=policies`]) {
      await page.goto(path);
      const frame = page.locator('[data-settings-frame]');
      const nav = frame.locator('[data-section-nav]');
      await expect(nav).toBeVisible();
      await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
      await expect(page.locator('[data-settings-frame] [data-section-nav]:visible')).toHaveCount(1);
      if (width === 400) await toggle().press('Enter');
      const left = page.locator('[data-workbench-bar="left"]:visible');
      await expect(left.getByRole('tree')).toBeVisible();
      await expect(left.locator('[data-section-nav]')).toHaveCount(0);
      await expect(left.locator('[data-container-title] button[aria-haspopup="menu"]')).toHaveCount(1);
      if (width === 400) {
        await left.getByRole('button', { name: /^(Close|关闭)$/, exact: true }).press('Enter');
        await expect(frame).toBeVisible();
      }
      const choose = async (id: 'general' | 'policies') => {
        const section = nav.locator(`[data-section-id="${id}"]`);
        const href = await section.getAttribute('href');
        if (href === null) throw new Error('The section has no native destination');
        const destination = new URL(href, page.url()).href;
        const picker = nav.getByRole('combobox');
        if (await picker.isVisible()) {
          const name = (await section.textContent())?.trim();
          if (name === undefined || name === '') throw new Error('The section has no name');
          await picker.click();
          await page.getByRole('option', { name, exact: true }).click();
        } else { await section.focus(); await page.keyboard.press('Enter'); }
        await expect(page).toHaveURL(destination);
        await expect(nav.locator('[aria-current="page"]')).toHaveCount(1);
        await expect(nav.locator(`[data-section-id="${id}"]`)).toHaveAttribute('aria-current', 'page');
      };
      await choose('general');
      // Hiding the global tree leaves the page's same section list available.
      if (width === 1100) await toggle().click();
      await expect(page.locator('[data-workbench-bar="left"]:visible')).toHaveCount(0);
      await expect(page.locator('[data-settings-frame] [data-section-nav]:visible')).toHaveCount(1);
      await choose('policies');
      if (width === 1100) await toggle().click();
    }
    // A plain topic address returns to content and its tree, even after visiting Settings.
    await page.goto(`/topic/${ACME.fixLogin}`);
    if (width === 400) await toggle().click();
    await expect(page.locator('[data-workbench-bar="left"]:visible [role="tree"]')).toBeVisible();
    await expect(page.locator('[data-workbench-bar="left"]:visible [data-settings-sidebar]')).toHaveCount(0);
  });

  test(`native view menus, Views and searchable shortcuts at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale); await page.goto(`/thread/${ACME.mainThread}`);
    await page.waitForLoadState('networkidle');
    if (width === 400) await page.locator('[data-container-choice="left:navigation"]').click();
    const left = page.locator('[data-workbench-bar="left"]:visible');
    const menu = left.locator('[data-container-title] button[aria-haspopup="menu"]');
    await expect(menu).toHaveCount(1); await menu.click();
    await expect(page.getByRole('menuitem', { name: /^(Copy link|复制链接)$/, exact: true })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: /^(Hide|隐藏)$/, exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await left.getByRole('button', { name: /^(Add a view|添加视图)$/, exact: true }).click();
    const chooser = page.getByRole('dialog', { name: /^(Views|视图)$/, exact: true });
    await expect(chooser.getByRole('heading', { name: /^(Views|视图)$/, exact: true })).toBeVisible();
    await expect(chooser.locator('[cmdk-group-heading], [data-choice-description], [data-choice-layout]')).toHaveCount(0);
    await expect(chooser.getByRole('option', { name: /^(Resources|资源)$/, exact: true })).toHaveCount(1);
    const lists = chooser.getByRole('option', { name: /^(Resource lists|资源列表)$/, exact: true });
    await expect(lists.locator('[data-view-group]')).toBeVisible();
    const outline = chooser.locator('[data-value="commands:view.open.outline"]');
    await expect(outline.locator('[data-view-group]')).toHaveText(locale === 'en' ? 'Conversation' : '对话');
    await outline.click();
    await expect(chooser).toHaveCount(0);
    if (width === 400) await page.locator('[data-container-choice="right:conversation"]').click();
    await page.locator('body').press('?');
    const shortcuts = page.getByRole('dialog', { name: /^(Keyboard shortcuts|键盘快捷键)$/, exact: true });
    await expect(shortcuts.getByRole('searchbox')).toBeVisible();
    const box = await shortcuts.boundingBox();
    if (box === null) throw new Error('The shortcut editor has no visible box');
    // A wide editor occupies over half the window; phone sizing still follows the shared dialog token.
    expect(box.width).toBeGreaterThan(width / 2); expect(box.width).toBeLessThanOrEqual(width);
    await shortcuts.getByRole('searchbox').fill(locale === 'en' ? 'inbox' : '收件箱');
    await expect(shortcuts.getByRole('table').getByRole('row')).toHaveCount(2);
  });
}

test('the shown page offers Close and no Show, and layout toggles have short names', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await signIn(page); await page.goto(`/thread/${ACME.mainThread}`);
  const title = page.locator('[data-screen-titlebar]');
  for (const name of ['Left bar', 'Bottom panel', 'Right bar']) await expect(title.getByRole('button', { name, exact: true })).toBeVisible();
  // A thread's tab is named with the topic it is in.
  await page.getByRole('tab', { name: 'Main thread · Fix login', exact: true }).click({ button: 'right' });
  const menu = page.getByRole('menu');
  await expect(menu.getByRole('menuitem', { name: 'Show', exact: true })).toHaveCount(0);
  await expect(menu.getByRole('menuitem', { name: 'Close', exact: true })).toBeEnabled();
});
