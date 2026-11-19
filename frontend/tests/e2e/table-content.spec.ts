/** Actual table geometry on the three screens that exposed squeezed facts, in both languages and themes. */
import { expect, type Locator, test } from '@playwright/test';

import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support.ts';

/** Plain facts must not wrap or be clipped, and must not receive the identity column's spare room. */
async function contentColumns(table: Locator, ids: readonly string[]): Promise<void> {
  const measured = await table.evaluate((element, columns) => {
    if (!(element instanceof HTMLTableElement)) throw new Error('The collection lost its native table');
    const headers = Array.from(element.tHead?.rows[0]?.cells ?? []);
    return headers.filter((header) => columns.includes(header.dataset.tableColumn ?? '')).map((header) => {
      const cells = [header, ...Array.from(element.tBodies).flatMap((body) => Array.from(body.rows).flatMap((row) => {
        const cell = row.cells[header.cellIndex];
        return cell === undefined ? [] : [cell];
      }))];
      const facts = cells.map((cell) => {
        const style = getComputedStyle(cell);
        const walker = document.createTreeWalker(cell, NodeFilter.SHOW_TEXT);
        const rects: DOMRect[] = [];
        for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
          if ((node.textContent ?? '').trim() === '') continue;
          const range = document.createRange();
          range.selectNodeContents(node);
          rects.push(...Array.from(range.getClientRects()).filter((rect) => rect.width > 0 && rect.height > 0));
        }
        const lines = new Set(rects.map((rect) => Math.round(rect.top)));
        const box = cell.getBoundingClientRect();
        const left = Number.parseFloat(style.paddingLeft);
        const right = Number.parseFloat(style.paddingRight);
        const inkLeft = Math.min(...rects.map((rect) => rect.left));
        const inkRight = Math.max(...rects.map((rect) => rect.right));
        const clipped = Array.from(cell.querySelectorAll<HTMLElement>('*')).some((child) => {
          const overflow = getComputedStyle(child).overflowX;
          return (overflow === 'hidden' || overflow === 'clip') && child.scrollWidth > child.clientWidth + 1;
        });
        return { oneLine: lines.size <= 1, clipped,
          fits: rects.length === 0 || inkLeft >= box.left + left - 1 && inkRight <= box.right - right + 1,
          needed: rects.length === 0 ? left + right : inkRight - inkLeft + left + right };
      });
      return { id: header.dataset.tableColumn, width: header.getBoundingClientRect().width,
        needed: Math.max(...facts.map((fact) => fact.needed)), facts };
    });
  }, [...ids]);
  for (const column of measured) {
    for (const fact of column.facts) {
      expect(fact.oneLine, `${column.id ?? ''}: a complete fact stays on one line`).toBe(true);
      expect(fact.clipped, `${column.id ?? ''}: no consumer cap cuts it short`).toBe(false);
      expect(fact.fits, `${column.id ?? ''}: all glyphs fit the column`).toBe(true);
    }
    // Two pixels cover collapsed-border and fractional font rounding, not extra allocated space.
    expect(column.width, `${column.id ?? ''}: surplus belongs to the name`).toBeLessThanOrEqual(column.needed + 2);
  }
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`tables size their facts before growing names (${locale}, ${theme})`, async ({ page }, info) => {
    await scenario();
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const words = locale === 'en' ? english : chinese;
    const scenes = [
      { key: 'agents', path: '/resources/agents', label: words.agents.list.label, facts: [{ id: 'pin', from: 640 }, { id: 'usable', from: 640 }] },
      { key: 'templates', path: '/resources/templates', label: words.templates.list.label, facts: [{ id: 'source', from: 800 }, { id: 'units', from: 640 }] },
      { key: 'roles', path: '/org/acme/settings/roles', label: words.access.roles.table.replace('{{name}}', 'Acme'), facts: [{ id: 'gave', from: 640 }, { id: 'until', from: 480 }] },
    ];
    for (const scene of scenes) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(scene.path);
      const table = page.getByRole('table', { name: scene.label, exact: true });
      await expect(table).toBeVisible();
      await expect(table.locator('tbody tr').first()).toBeVisible();
      for (const width of [1440, 1100, 400]) {
        await page.setViewportSize({ width, height: 900 });
        await expect(table.locator('thead [data-table-grow="true"]')).toHaveCount(1);
        const region = table.locator('..');
        const room = await region.evaluate((element) => element.clientWidth);
        // Wait for the same documented responsive thresholds used by these columns before measuring ink.
        for (const fact of scene.facts) await expect(table.locator(`thead [data-table-column="${fact.id}"]`)).toHaveCount(room >= fact.from ? 1 : 0);
        await contentColumns(table, scene.facts.map((fact) => fact.id));
        await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await region.focus();
        await expect(region).toBeFocused();
        if (width === 1440 && scene.key === 'templates') {
          const source = table.locator('td[data-table-column="source"] code').filter({ hasText: 'gcr.io/distroless/static-debian12:nonroot' });
          await expect(source).toBeVisible();
          await expect(source).toHaveText('gcr.io/distroless/static-debian12:nonroot');
          expect(await source.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
        }
        await info.attach(`${scene.key}-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
      }
    }
  });
}
