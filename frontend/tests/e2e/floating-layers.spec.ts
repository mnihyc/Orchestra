/** Every shared floating family fits the real viewport at all corners, with long contents still reachable. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { resourceFixtures } from '../../e2e/layout-fixtures';
import { asAdmin } from './admin-mode-support';
import { scenario, signIn } from './support.ts';
import { FLOATING_KINDS, type FloatingKind } from './floating-cases';

const kinds = FLOATING_KINDS;
const surface = '.floating-layer:visible,[data-radix-popper-content-wrapper] > :visible,[data-overflow-tooltip]:visible';

for (const width of [400, 1440]) for (const theme of ['light', 'dark']) for (const locale of ['en', 'zh-CN']) {
  test(`floating layers fit and scroll (${width}/${theme}/${locale})`, { tag: '@development' }, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: theme as 'light' | 'dark' });
    await signIn(page);
    await page.evaluate(async ({ theme, locale }) => {
      const module = '/tests/component/floating-harness.tsx';
      const { setFloatingAppearance } = await import(/* @vite-ignore */ module) as { setFloatingAppearance: (theme: string, locale: string) => Promise<void> };
      await setFloatingAppearance(theme, locale);
    }, { theme, locale });
    const measurements: unknown[] = [];
    for (const kind of kinds) for (const [left, top] of [[8, 8], [width - 158, 8], [8, 850], [width - 158, 850]]) {
      await page.evaluate(async ({ kind, left, top, text }) => {
        const module = '/tests/component/floating-harness.tsx';
        const { mountFloating } = await import(/* @vite-ignore */ module) as { mountFloating: (kind: FloatingKind, left: number, top: number, text: string) => Promise<void> };
        await mountFloating(kind, left!, top!, text);
      }, { kind, left, top, text: locale === 'en' ? 'A complete long name with all its final words' : '完整的名称，包括末尾的文字' });
      const location = `${kind} ${left}/${top}`;
      const example = page.locator('[data-floating-example]');
      if (kind === 'overflow') { await expect(example.locator('span')).toHaveAttribute('tabindex', '0'); await example.locator('span').hover(); }
      else if (kind === 'tooltip') await example.getByRole('button').hover();
      else if (kind === 'context') await example.getByRole('button').click({ button: 'right' });
      else if (kind === 'date') await example.getByRole('button').click();
      else if (kind === 'select' || kind.includes('combobox')) await example.getByRole('combobox').click({ position: { x: 10, y: 10 } });
      else if (!['suggestions', 'reader', 'fixed', 'overflow'].includes(kind)) await example.getByRole('button').first().click();
      if (kind === 'submenu') {
        await page.getByRole('menuitem', { name: 'Nested layer' }).hover();
        await expect(page.getByRole('menu')).toHaveCount(2);
      }
      if (['popover', 'help', 'detail', 'person', 'agent', 'reference', 'date', 'avatars', 'holder'].includes(kind)) await expect(page.getByRole('dialog'), location).toBeVisible();
      await expect(page.locator(surface).last(), location).toBeVisible();
      await page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
      const layers = await page.locator(surface).evaluateAll((layers) => layers.map((layer) => {
        const element = layer as HTMLElement, box = element.getBoundingClientRect();
        const scroll = [element, ...Array.from(element.querySelectorAll<HTMLElement>('*'))].find((child) => ['auto', 'scroll'].includes(getComputedStyle(child).overflowY) && child.scrollHeight > child.clientHeight + 1 && child.clientHeight > 0);
        let scrolls = false;
        if (scroll !== undefined) { const before = scroll.scrollTop; scroll.scrollTop = 0; scroll.scrollTop = scroll.clientHeight; scrolls = scroll.scrollTop > 0; scroll.scrollTop = before; }
        return { role: element.getAttribute('role'), left: box.left, top: box.top, right: box.right, bottom: box.bottom, scrolls };
      }));
      for (const layer of layers) {
        expect(layer.left, location).toBeGreaterThanOrEqual(-1); expect(layer.top, location).toBeGreaterThanOrEqual(-1);
        expect(layer.right, location).toBeLessThanOrEqual(width + 1); expect(layer.bottom, location).toBeLessThanOrEqual(901);
      }
      if (kind === 'overflow' || kind === 'tooltip') {
        const anchor = await (kind === 'overflow' ? example.locator('span') : example.getByRole('button')).boundingBox();
        if (anchor === null) throw new Error('The tooltip trigger is missing');
        for (const layer of layers) expect(layer.bottom <= anchor.y + 1 || layer.top >= anchor.y + anchor.height - 1, 'a long tooltip stays clear of its trigger').toBe(true);
      }
      if (!['date', 'reader', 'submenu'].includes(kind)) expect(layers.some((layer) => layer.scrolls), `${kind} long contents scroll`).toBe(true);
      if (kind === 'submenu') expect(layers.some((layer) => layer.role === 'menu' && layer.scrolls), 'long submenu scrolls').toBe(true);
      measurements.push({ kind, left, top, layers });
      await page.keyboard.press('Escape'); await page.mouse.move(0, 0);
    }
    await info.attach('floating-measurements', { body: JSON.stringify(measurements, null, 2), contentType: 'application/json' });
    await info.attach('floating-screen', { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });

  test(`only the top transient layer closes on Escape (${width}/${theme}/${locale})`, { tag: '@development' }, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: theme as 'light' | 'dark' });
    await signIn(page);
    const text = locale === 'en' ? 'A complete value inside the layered inspection' : '检查卡片中完整显示的名称和末尾文字';
    await page.evaluate(async ({ theme, locale, text }) => {
      const module = '/tests/component/floating-harness.tsx';
      const { setFloatingAppearance, mountLayeredOverflow } = await import(/* @vite-ignore */ module) as {
        setFloatingAppearance: (theme: string, locale: string) => Promise<void>;
        mountLayeredOverflow: (text: string) => Promise<void>;
      };
      await setFloatingAppearance(theme, locale);
      await mountLayeredOverflow(text);
    }, { theme, locale, text });
    const trigger = page.getByRole('button', { name: text, exact: true });
    const tooltip = page.locator('[data-overflow-tooltip]');
    const inspection = page.locator('[data-inspection-card]:visible');
    const parent = page.getByRole('dialog', { name: 'Layer parent', exact: true });
    await trigger.focus();
    await expect(tooltip).toHaveText(text);
    await expect(inspection).toHaveCount(1);
    await page.keyboard.press('Escape');
    await expect(tooltip).toHaveCount(0);
    await expect(inspection).toHaveCount(1);
    await expect(parent).toBeVisible();
    await expect(trigger).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(inspection).toHaveCount(0);
    await expect(parent).toBeVisible();
    await expect(trigger).toBeFocused();
    await page.keyboard.press('Escape');
    await expect(parent).toBeHidden();
  });
}

type InspectionWindow = Window & { __inspectionFrames: { active: boolean; frames: { left: number; top: number; right: number; bottom: number }[] } };

for (const locale of ['en', 'zh-CN'] as const) {
  test(`a host inspection stays inside the viewport while its native sharing read resolves (${locale})`, async ({ page }, info) => {
    const words = locale === 'en' ? en : zh;
    await scenario();
    await resourceFixtures(page);
    await page.setViewportSize({ width: 1440, height: 900 });
    await signIn(page);
    await asAdmin(page, true);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    let release: () => void = () => undefined;
    const held = new Promise<void>((done) => { release = done; });
    await page.route(/\/api\/v1\/hosts\/[^/]+\/permissions$/, async (route) => {
      await held;
      await route.fulfill({ status: 503, json: { type: 'about:blank', status: 503, title: 'Temporarily unavailable' } });
    });
    await page.goto('/admin/runtimes');
    await page.getByRole('combobox', { name: words.admin.runtimes.host, exact: true }).click();
    await page.getByRole('option', { name: /build-box/ }).hover();
    const card = page.locator('[data-inspection-card][aria-label="build-box"]');
    await expect(card.getByText(words.linkCards.sharingChecking, { exact: true })).toBeVisible();
    await card.evaluate((element) => {
      const observed = window as unknown as InspectionWindow;
      observed.__inspectionFrames = { active: true, frames: [] };
      const sample = () => {
        if (!observed.__inspectionFrames.active) return;
        const box = element.getBoundingClientRect();
        observed.__inspectionFrames.frames.push({ left: box.left, top: box.top, right: box.right, bottom: box.bottom });
        requestAnimationFrame(sample);
      };
      requestAnimationFrame(sample);
    });
    release();
    await expect(card.getByText(words.linkCards.sharingUnavailable, { exact: true })).toBeVisible();
    const frames = await page.evaluate(async () => {
      await new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
      const observation = (window as unknown as InspectionWindow).__inspectionFrames;
      observation.active = false;
      return observation.frames;
    });
    expect(frames.length).toBeGreaterThan(1);
    for (const box of frames) {
      expect(box.left).toBeGreaterThanOrEqual(0); expect(box.top).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(1440); expect(box.bottom).toBeLessThanOrEqual(900);
    }
    await info.attach('native-sharing-resize-bounds', { body: JSON.stringify(frames), contentType: 'application/json' });
  });
}
