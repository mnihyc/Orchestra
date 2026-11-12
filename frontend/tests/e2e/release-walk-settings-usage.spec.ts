/** Pane width and real section addresses remain coherent through actual browsing and device folds. */
import { expect, test } from '@playwright/test';
import { language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Usage → all six ranges in one selector → 90 days → resize the main pane to 700 → wide segmented row → Custom; unknown org section → Defaults → fold a provider home → reload; project/topic Defaults links and unavailable Members sections.
  // 用量 → 单个控件内的六种范围 → 90 天 → 主区改为 700 像素 → 宽区单行分段控件 → 自定义；未知组织分区 → 默认值 → 折叠提供方归属 → 刷新；项目与主题的默认值地址，以及不可用的成员部分。
  test(`usage and settings keep their visible state and address ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const t = await language(page, request, locale, theme);
    await page.goto('/usage?range=30d'); const period = page.locator('[data-usage-period]');
    const narrow = period.getByRole('combobox', { name: t('usage.period.label'), exact: true });
    await expect(period).toBeVisible();
    if (await narrow.isVisible()) {
      await narrow.click(); await expect(page.getByRole('option')).toHaveCount(6);
      await page.getByRole('option', { name: t('usagePerf.periodNames.90d'), exact: true }).click();
    } else await period.getByRole('radio', { name: t('usagePerf.periodNames.90d'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('range')).toBe('90d');
    await info.attach('usage-responsive-period', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    if (width === 1440) {
      const main = await page.locator('[data-main-area]').boundingBox(); if (main === null) throw new Error('The usage main pane is laid out');
      const windowWidth = Math.round(width + 700 - main.width); await page.setViewportSize({ width: windowWidth, height: 900 });
      await expect(narrow).toBeVisible(); await expect(narrow).toContainText(t('usagePerf.periodNames.90d'));
      await info.attach('usage-700-main', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
      await page.setViewportSize({ width: windowWidth + 500, height: 900 });
      const group = period.getByRole('radiogroup', { name: t('usage.period.label'), exact: true }); await expect(group).toBeVisible();
      const tops = await group.getByRole('radio').evaluateAll((items) => items.map((item) => item.getBoundingClientRect().top));
      expect(tops).toHaveLength(6); expect(Math.max(...tops) - Math.min(...tops)).toBeLessThan(1);
      await expect(group.getByRole('radio', { name: t('usagePerf.periodNames.90d'), exact: true })).toBeChecked();
      await page.setViewportSize({ width, height: 900 });
    }
    await period.getByRole('button', { name: t('usage.period.custom'), exact: true }).click();
    const custom = page.getByRole('dialog', { name: t('usage.period.customLabel'), exact: true });
    await expect(custom.getByLabel(t('usage.period.from'), { exact: true })).toBeVisible(); await expect(custom.getByLabel(t('usage.period.to'), { exact: true })).toBeVisible();
    await custom.getByRole('button', { name: t('common.cancel'), exact: true }).click();
    await page.goto('/org/acme/settings/agents');
    await expect(page.getByRole('heading', { name: t('settings.sectionUnavailable'), exact: true })).toBeVisible();
    await expect(page.getByText(t('settings.chooseSection'), { exact: true })).toBeVisible();
    await expect(page.getByRole('textbox', { name: t('settings.general.name'), exact: true })).toHaveCount(0);
    const nav = page.locator('[data-settings-frame] [data-section-nav]');
    await expect(nav.locator('[aria-current="page"]')).toHaveCount(0);
    await info.attach('unknown-settings-section', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    const sections = nav.getByRole('combobox');
    if (await sections.isVisible()) { await sections.click(); await page.getByRole('option', { name: t('day100.names.defaults'), exact: true }).click(); }
    else await nav.getByRole('link', { name: t('day100.names.defaults'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).pathname).toBe('/org/acme/settings/defaults');
    await expect(page.getByRole('searchbox', { name: t('settings.editor.search'), exact: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.has('_list')).toBe(false); expect(new URL(page.url()).searchParams.has('list')).toBe(false);
    await info.attach('defaults-section-address', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    const key = 'agent.default_provider_id'; await page.goto(`/org/acme/settings/defaults?setting=${key}`);
    const field = page.locator(`[data-setting="${key}"]`).getByRole('combobox');
    await field.click();
    const collapse = page.getByRole('button', { name: t('resourceGroups.collapse', { name: 'Acme' }), exact: true });
    await collapse.click(); await expect(page.getByRole('button', { name: t('resourceGroups.expand', { name: 'Acme' }), exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    expect(new URL(page.url()).searchParams.has('list')).toBe(false); expect(new URL(page.url()).searchParams.has('_list')).toBe(false);
    await page.reload(); await field.click();
    await expect(page.getByRole('button', { name: t('resourceGroups.expand', { name: 'Acme' }), exact: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('setting')).toBe(key); expect(new URL(page.url()).searchParams.has('list')).toBe(false);
    await page.keyboard.press('Escape');
    for (const path of [`/project/${ACME.web}/settings?section=defaults&setting=${key}`, `/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=${key}`]) {
      await page.goto(path); await expect(page.locator(`[data-setting="${key}"]`)).toBeVisible();
      expect(new URL(page.url()).searchParams.get('section')).toBe('defaults');
    }
    for (const path of [`/project/${ACME.web}/settings?section=members`, `/topic/${ACME.fixLogin}?tab=settings&section=members`]) {
      await page.goto(path);
      await expect(page.getByRole('heading', { name: t('settings.sectionUnavailable'), exact: true })).toBeVisible();
      await expect(page.locator('[data-settings-frame] [aria-current="page"]')).toHaveCount(0);
      await expect(page.getByRole('textbox', { name: t('settings.general.name'), exact: true })).toHaveCount(0);
    }
    await info.attach('unavailable-other-scope-section', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
  });
}
