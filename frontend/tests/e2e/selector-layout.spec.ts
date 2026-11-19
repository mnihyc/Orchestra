/** Actual selector geometry and computed colours on the screen that exposed the long All description. */
import { expect, test } from '@playwright/test';
import { contrast } from '../accessibility/contrast.ts';
import { inspectLayout } from '../../e2e/layout-rules-dom.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) test(`selector geometry, selected menu checks and readable palette rows at both widths/themes in ${locale}`, async ({ page }) => {
  const catalogue = locale === 'en' ? english : chinese;
  await scenario();
  await signIn(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await page.goto('/resources/environments');
  for (const theme of ['light', 'dark'] as const) {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.getByRole('button', { name: catalogue.shell.account.label, exact: true }).click();
    await page.getByRole('menuitemradio', { name: catalogue.choiceCatalog.theme[theme].name, exact: true }).click();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      const registered = catalogue.choiceCatalog.environmentFilter.machine.name;
      const group = page.getByRole('radiogroup').filter({ has: page.getByRole('radio', { name: registered, exact: true }) });
      const all = group.getByRole('radio', { name: catalogue.choiceCatalog.environmentFilter.all.name, exact: true });
      await all.click();
      await expect(all).toBeChecked();
      await expect(all).not.toHaveAttribute('aria-describedby');
      await expect(group.locator('[data-choice-descriptions], [data-choice-description], [data-choice-help]')).toHaveCount(0);
      await all.focus();
      await page.keyboard.press('ArrowRight');
      await expect(group.getByRole('radio', { name: registered, exact: true })).toBeChecked();
      await expect(page).toHaveURL(/kind=machine/);
      await page.keyboard.press('ArrowLeft');
      await expect(all).toBeChecked();
      const sizes = await group.getByRole('radio').evaluateAll((options) => options.map((option) => {
        const label = document.getElementById(option.getAttribute('aria-labelledby') ?? '');
        if (label === null) throw new Error('An option lost its short name');
        const paint = document.createElement('canvas').getContext('2d');
        if (paint === null) throw new Error('The browser cannot measure label width');
        paint.font = getComputedStyle(label).font;
        const style = getComputedStyle(option);
        return {
          name: label.textContent,
          width: option.getBoundingClientRect().width,
          expected: paint.measureText(label.textContent).width + Number.parseFloat(style.paddingLeft) + Number.parseFloat(style.paddingRight) + Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.borderRightWidth),
        };
      }));
      for (const size of sizes) {
        // Two pixels cover fractional font/box rounding, not room for any explanatory sentence.
        expect(size.width, size.name).toBeLessThanOrEqual(size.expected + 2);
        expect(size.width, size.name).toBeGreaterThanOrEqual(size.expected - 2);
      }
      await expect.poll(async () => {
        const frame = await group.evaluate((element) => {
          const style = getComputedStyle(element);
          return { border: style.borderTopColor, width: Number.parseFloat(style.borderTopWidth), fill: style.backgroundColor };
        });
        const colours = await group.getByRole('radio').evaluateAll((options) => options.map((option) => {
          const style = getComputedStyle(option);
          const background = getComputedStyle(option.closest('[role="group"], [role="radiogroup"]') ?? option.parentElement ?? option).backgroundColor;
          return { border: style.borderTopColor, width: Number.parseFloat(style.borderTopWidth), background, fill: style.backgroundColor, ink: style.color, selected: option.getAttribute('aria-checked') === 'true' };
        }));
        // The group owns the one boundary; an unselected segment deliberately has no filled box.
        return frame.width >= 1 && contrast(frame.border, frame.fill) >= 3
          && colours.every((colour) => !colour.selected || contrast(colour.fill, colour.background) >= 3 && contrast(colour.ink, colour.fill) >= 4.5);
      }).toBe(true);
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
      await page.getByRole('button', { name: catalogue.shell.account.label, exact: true }).click();
      const chosen = page.getByRole('menuitemradio', { name: catalogue.choiceCatalog.theme[theme].name, exact: true });
      await page.getByRole('menuitemradio', { name: catalogue.choiceCatalog.theme[theme === 'light' ? 'dark' : 'light'].name, exact: true }).focus();
      await expect(chosen).toBeChecked();
      await expect(chosen.locator('svg.lucide-check')).toHaveCount(1);
      await expect.poll(() => chosen.evaluate((element) => getComputedStyle(element).backgroundColor)).toBe('rgba(0, 0, 0, 0)');
      const menuAudit = await page.evaluate(inspectLayout, { duplicateAllowList: [], rawValues: [] });
      expect(menuAudit.faults.filter((fault) => fault.rule === 'choice-text-contrast' || fault.rule === 'choice-word-width')).toEqual([]);
      await page.keyboard.press('Escape');
      await page.keyboard.press('Control+k');
      const palette = page.getByRole('dialog').filter({ has: page.locator('[cmdk-root]') });
      await expect(palette).toBeVisible();
      const input = palette.getByRole('combobox');
      await input.fill('Acme');
      await expect(palette.locator('[role="option"][aria-selected="true"]')).toHaveCount(1);
      const paletteAudit = await page.evaluate(inspectLayout, { duplicateAllowList: [], rawValues: [] });
      expect(paletteAudit.faults.filter((fault) => fault.rule === 'choice-text-contrast' || fault.rule === 'choice-word-width')).toEqual([]);
      await page.keyboard.press('Escape');
    }
  }
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) test(`creation audience has no caption column and one complete question in ${locale}/${theme}`, async ({ page }) => {
  const catalogue = locale === 'en' ? english : chinese;
  await scenario(); await signIn(page);
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await page.goto('/resources/environments');
  await page.getByRole('button', { name: catalogue.shell.account.label, exact: true }).click();
  await page.getByRole('menuitemradio', { name: catalogue.choiceCatalog.theme[theme].name, exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  await page.getByRole('button', { name: catalogue.resourceCreation.environments, exact: true }).click();
  const dialog = page.getByRole('dialog', { name: catalogue.environments.make.title });
  const group = dialog.getByRole('radiogroup', { name: catalogue.resources.access.label });
  const own = group.getByRole('radio', { name: catalogue.choiceCatalog.resourceAccess.only_me.name, exact: true });
  await expect(own).toBeChecked();
  await expect(own).toHaveAttribute('data-state', 'checked');
  const questionName = catalogue.help.hint.label.replace('{{term}}', catalogue.resources.access.label);
  for (const width of [1440, 400]) {
    await page.setViewportSize({ width, height: 900 });
    await expect(dialog.locator('[data-choice-selected-description], [data-choice-descriptions], [data-choice-description]')).toHaveCount(0);
    const sizes = await group.evaluate((element) => {
      const field = element.closest('[data-choice-selector]');
      if (field === null) throw new Error('The shared audience selector lost its frame');
      return { group: element.getBoundingClientRect().height, field: field.getBoundingClientRect().height };
    });
    // The selector wrapper has one row, with one pixel for fractional layout rounding, no six-line caption.
    expect(sizes.field).toBeLessThanOrEqual(sizes.group + 1);
    await expect(dialog.getByRole('button', { name: questionName, exact: true })).toHaveCount(1);
    await dialog.getByRole('button', { name: questionName, exact: true }).click();
    const help = page.getByRole('dialog', { name: catalogue.resources.access.label, exact: true });
    await expect(help.getByRole('term')).toHaveCount(2);
    await expect(help).toContainText(catalogue.resources.access.gives.only_me.replaceAll('{{home}}', catalogue.environments.make.yourPlace));
    await expect(help).toContainText(catalogue.resources.access.gives.defaults.replaceAll('{{home}}', catalogue.environments.make.yourPlace));
    await page.keyboard.press('Escape');
    await expect(own).toBeChecked();
    await own.hover();
    await expect(page.getByRole('tooltip')).toContainText(catalogue.resources.access.gives.only_me.replaceAll('{{home}}', catalogue.environments.make.yourPlace));
    await page.mouse.move(0, 0);
    await expect(page.getByRole('tooltip')).toHaveCount(0);
    await expect(dialog).toBeInViewport();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
});
