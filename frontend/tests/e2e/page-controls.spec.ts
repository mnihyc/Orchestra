/** Shared page controls fit desktop and phone screens in both languages/themes, with real keyboard and geometry checks. */
import { expect, type APIRequestContext, type Page, test } from '@playwright/test';
import { contrast } from '../accessibility/contrast';
import { ACME, bodySent, PEOPLE, scenario, signIn } from './support';
import { detailMcp, detailMount } from '../component/resource-detail-fixtures';
import { inventoryRow } from '../doubles/everything-row';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
/** A CSS pixel of drift is visible at the shared ordinary control height. */
const MAX_DRIFT = 1;

async function preference(request: APIRequestContext, locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.ada.login, password: PEOPLE.ada.password } });
  expect(login.ok()).toBe(true);
  const { access_token: token } = await login.json() as { access_token: string };
  const saved = await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale, theme } });
  expect(saved.ok()).toBe(true);
}

async function fits(page: Page) {
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
}

async function capture(page: Page, name: string) {
  const path = test.info().outputPath(`${name.replaceAll(/[^\w.-]/g, '-')}.png`);
  await page.screenshot({ path, fullPage: true, animations: 'disabled' });
  await test.info().attach(name, { path, contentType: 'image/png' });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`page controls and settings sections fit in ${locale}/${theme}`, async ({ page, request }) => {
    // The full kind/scope matrix includes keyboard workflows and screenshots in one signed-in scenario.
    test.slow();
    await scenario(); await signIn(page); await preference(request, locale, theme);
    const words = locale === 'en' ? english : chinese;
    // This geometry case requires a row in every kind. The default double has
    // no MCP/mount inventory; supply the same typed rows used by detail proofs.
    await page.route('**/api/v1/everything?*', async (route) => {
      const query = new URL(route.request().url()).searchParams;
      const kind = query.get('kinds');
      if (query.get('resource_details') !== 'true' || (kind !== 'mcp_server' && kind !== 'mount')) { await route.continue(); return; }
      const row = kind === 'mcp_server'
        ? inventoryRow({ id: detailMcp.id, kind: 'mcp_server', name: detailMcp.name, owner: detailMcp.owner, my_access: 'manage', resource: { kind: 'mcp_server', value: detailMcp } })
        : inventoryRow({ id: detailMount.id, kind: 'mount', name: detailMount.name, owner: detailMount.owner, my_access: 'manage', resource: { kind: 'mount', value: detailMount } });
      const response = await route.fetch(); expect(response.ok()).toBe(true);
      await route.fulfill({ response, json: { items: [row], next_cursor: null } });
    });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await page.goto('/resources/machines');
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      const bar = page.locator('[data-filter-bar]');
      await expect(bar).toHaveCount(1); await expect(bar.getByRole('searchbox')).toBeVisible();
      await expect(bar.getByRole('switch')).toHaveCount(0); await expect(bar.getByRole('combobox')).toHaveCount(0);
      await expect(bar.getByRole('button')).toHaveCount(2);
      await expect(bar.getByRole('button', { name: words.resources.controls.filter, exact: true })).toBeVisible();
      await expect(bar.getByRole('button', { name: words.resources.controls.sort, exact: true })).toBeVisible();
      const action = page.locator('[data-resource-kind="machines"]').getByRole('button'); await expect(action).toHaveCount(1); await expect(action).toBeVisible();
      await expect(bar.locator('[data-filter-action]')).toHaveCount(0);
      const controls = await bar.locator('input:not([type="checkbox"]), button:not([role]), button[role="combobox"], [data-choice-selector] [role="radiogroup"]').evaluateAll((elements) => elements
        .filter((element) => element.getClientRects().length > 0).map((element) => ({ name: element.getAttribute('aria-label') ?? element.textContent, height: element.getBoundingClientRect().height })));
      for (const control of controls) expect(Math.abs(control.height - 28), control.name).toBeLessThan(MAX_DRIFT);
      const centres = await action.evaluate((element) => {
        const icon = element.querySelector('svg'); const walk = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
        let words = walk.nextNode(); while (words !== null && (words.textContent ?? '').trim() === '') words = walk.nextNode();
        if (icon === null || words === null) throw new Error('The primary action lost its icon or words');
        const range = document.createRange(); range.selectNodeContents(words); const line = range.getClientRects()[0];
        if (line === undefined) throw new Error('The action has no first text line');
        const bounds = icon.getBoundingClientRect(); return Math.abs(bounds.top + bounds.height / 2 - line.top - line.height / 2);
      });
      expect(centres).toBeLessThan(MAX_DRIFT); await fits(page);
      await capture(page, `machines-${width}-${locale}-${theme}`);

      for (const kind of ['providers', 'agents', 'machines', 'environments', 'templates', 'hosts', 'mcp', 'mounts']) {
        await page.goto(`/resources/${kind}`);
        const list = page.getByRole('table').first(); await expect(list).toBeVisible();
        await expect(list.locator('thead [data-table-column="select"]')).toBeVisible();
        const first = list.locator('tbody tr').first();
        await expect(first.locator('[data-table-column="name"]')).toBeVisible();
        // Before hover, the row's primary action must be visible even on a touch-width screen.
        await expect(first.locator('[data-table-column="primary"]').getByRole('button')).toBeVisible();
        await expect(page.locator(`[data-resource-kind="${kind}"]`).getByRole('button').first()).toBeVisible();
        const aligned = await page.locator('[data-filter-bar]').getByRole('searchbox').evaluate((input) => {
          const top = input.getBoundingClientRect().top;
          const buttons = input.closest('[data-filter-bar]')?.querySelectorAll('button');
          return buttons === undefined ? false : Array.from(buttons).every((button) => Math.abs(button.getBoundingClientRect().top - top) < 1);
        });
        expect(aligned, `${kind}: search, Filter and Sort stay on one row`).toBe(true);
        const filter = page.locator('[data-filter-bar]').getByRole('button', { name: words.resources.controls.filter, exact: true });
        const sort = page.locator('[data-filter-bar]').getByRole('button', { name: words.resources.controls.sort, exact: true });
        const filterPanel = page.locator('[data-resource-filter-panel]'), sortPanel = page.locator('[data-resource-sort-panel]');
        await filter.press('Enter');
        await expect(filterPanel.getByRole('switch')).toHaveCount(1);
        await expect(filterPanel.getByRole('switch')).toBeFocused();
        const tracks = await page.locator('[data-resource-filter-panel] [data-choice-selector] [role="radiogroup"]').evaluateAll((elements) => elements.map((element) => element.getBoundingClientRect().height));
        for (const height of tracks) expect(Math.abs(height - 28)).toBeLessThan(MAX_DRIFT);
        await page.keyboard.press('Escape');
        // Escape finishes by returning focus; tab to the next trigger after that keyboard handoff.
        await expect(filterPanel).toHaveCount(0); await expect(filter).toBeFocused();
        await page.keyboard.press('Tab'); await expect(sort).toBeFocused();
        await page.keyboard.press('Enter');
        await expect(sortPanel.getByRole('combobox')).toHaveCount(2);
        await page.keyboard.press('Escape');
        await expect(sortPanel).toHaveCount(0); await expect(sort).toBeFocused();
        await fits(page);
        await capture(page, `list-${kind}-${width}-${locale}-${theme}`);
      }

      await page.goto('/everything');
      await page.getByRole('button', { name: words.everything.filters, exact: true }).click();
      const date = page.locator('input[data-date-field="date"]').first(); await expect(date).toBeVisible(); await expect(date).toHaveAttribute('type', 'text');
      const picker = page.getByRole('button', { name: /^(Choose date|选择日期)/ }).first();
      await picker.click(); await expect(page.getByRole('grid')).toBeVisible();
      await page.keyboard.press('ArrowRight');
      await expect(page.getByRole('grid').locator('button[tabindex="0"]')).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(date).toHaveValue(/^\d{4}-\d{2}-\d{2}$/); await expect(page.getByRole('grid')).toHaveCount(0);
      await picker.click(); await page.getByRole('button', { name: locale === 'en' ? 'Clear date' : '清除日期', exact: true }).click();
      await expect(date).toHaveValue(''); await fits(page);
      await capture(page, `everything-${width}-${locale}-${theme}`);

      for (const path of ['/settings/profile', '/org/acme/settings/general', '/org/acme.research/settings/general', `/project/${ACME.web}/settings`, `/topic/${ACME.fixLogin}?tab=settings`, '/admin/overview']) {
        if (path.startsWith('/admin')) {
          const admin = page.locator('[data-as-admin]');
          if (await admin.getAttribute('aria-checked') !== 'true') await admin.click();
          await expect(admin).toHaveAttribute('aria-checked', 'true');
        }
        await page.goto(path);
        const frame = page.locator('[data-settings-frame]:visible');
        const sections = frame.locator('[data-section-nav]'); await expect(sections).toBeVisible();
        const compact = await frame.evaluate((element) => element.getBoundingClientRect().width < 600);
        await expect(sections.locator('[aria-current="page"]')).toHaveCount(1);
        await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
        const place = new Map([
          ['/org/acme/settings/general', 'Acme'], ['/org/acme.research/settings/general', 'Research'],
          [`/project/${ACME.web}/settings`, 'Web'], [`/topic/${ACME.fixLogin}?tab=settings`, 'Fix login'],
        ]).get(path);
        if (place !== undefined) {
          const ownTab = page.getByRole('tab', { name: `${words.settings.org.sectionNames.general} · ${place}`, exact: true });
          await expect(ownTab).toHaveAttribute('aria-selected', 'true');
          await expect(ownTab).toBeInViewport();
        }
        if (compact) {
          const select = sections.getByRole('combobox'); await expect(select).toBeVisible();
          await expect(sections.getByRole('link')).toHaveCount(0);
          const target = await sections.locator('a[data-section-id]').last().evaluate((link) => ({ name: link.textContent.trim(), href: (link as HTMLAnchorElement).href }));
          if (target.name === '') throw new Error('The actual section link lost its name');
          const before = page.url();
          await select.focus(); await expect(page.getByRole('listbox')).toBeVisible();
          if (path.startsWith('/admin/')) await expect(page.getByRole('option')).toHaveCount(21);
          await select.fill(target.name);
          await expect(page.getByRole('option', { name: target.name, exact: true })).toBeVisible();
          await expect(page.getByRole('option')).toHaveCount(1); expect(page.url()).toBe(before);
          await select.press('Enter'); await expect(page).toHaveURL(target.href);
          await expect(page.getByRole('listbox')).toHaveCount(0);
          await expect(sections.locator('[aria-current="page"]')).toHaveCount(1);
          await expect(sections.getByRole('combobox')).toHaveValue(target.name);
          await expect(page.getByRole('heading', { level: 1 })).toHaveCount(1);
        } else {
          await expect(sections.getByRole('combobox')).not.toBeVisible();
          const links = sections.getByRole('link');
          for (const link of await links.all()) await expect(link).toBeVisible();
          const colours = await sections.locator('[aria-current="page"]').evaluate((element) => {
            const style = getComputedStyle(element); return { ink: style.color, fill: style.backgroundColor };
          });
          expect(contrast(colours.ink, colours.fill)).toBeGreaterThanOrEqual(4.5);
          const selected = sections.locator('[aria-current="page"]'); await selected.focus(); await expect(selected).toBeFocused();
          if (path.startsWith('/settings/') || path.startsWith('/admin/')) {
            // Settings sections stay inside this page; changing page type
            // cannot substitute them for the persistent global Navigation.
            await expect(page.locator('[data-settings-sidebar]')).toHaveCount(0);
            await expect(frame.locator('[data-section-nav]')).toHaveCount(1);
            await expect(page.locator('[data-workbench-bar="left"]').getByRole('navigation', { name: words.views.navigation.title, exact: true })).toBeVisible();
          }
        }
        if (path === '/settings/profile' && width === 1440) {
          const name = page.locator('input[autocomplete="name"]');
          const original = await name.inputValue();
          const accepted = page.waitForResponse(async (response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me') && ((await bodySent(response.request())) as { display_name?: string }).display_name === 'Release reviewer');
          await name.fill('Release reviewer');
          expect((await accepted).ok()).toBe(true);
          const undo = page.getByRole('button', { name: /^(Undo the change to|撤销对)/ }); await expect(undo).toBeVisible();
          const undone = page.waitForResponse(async (response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname.endsWith('/me') && ((await bodySent(response.request())) as { display_name?: string }).display_name === original);
          await undo.click(); expect((await undone).ok()).toBe(true); await expect(name).toHaveValue(original);
        }
        await fits(page);
        await capture(page, `sections-${path.replaceAll('/', '-')}-${width}-${locale}-${theme}`);
      }
    }
    for (const width of [599, 600]) {
      await page.setViewportSize({ width, height: 900 }); await page.goto('/org/acme/settings/general');
      const frame = page.locator('[data-settings-frame]:visible');
      const sections = frame.locator('[data-section-nav]');
      const compact = await frame.evaluate((element) => element.getBoundingClientRect().width < 600);
      if (compact) await expect(sections.getByRole('combobox')).toBeVisible();
      else { await expect(sections.getByRole('combobox')).not.toBeVisible(); await expect(sections.getByRole('link').first()).toBeVisible(); }
      await fits(page);
    }
  });
}
