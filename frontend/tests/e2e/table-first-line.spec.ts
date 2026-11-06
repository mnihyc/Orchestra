/** Shared cell layout preserves mixed inline data, small wrapped text and outside control semantics. */
import { expect, test } from '@playwright/test';
import { inspectLayout, type LayoutInput } from '../../e2e/layout-rules-dom';
import { LAYOUT_INPUT } from '../../e2e/layout-rules-helpers';
import type { TableHarness } from '../component/table-first-line-types';
import { scenario, signIn } from './support';

test('shared table first lines stay aligned across wrapped cell shapes', { tag: '@development' }, async ({ page }, info) => {
  await scenario(); await signIn(page);
  const failures: unknown[] = [], receipts: unknown[] = [];
  for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['mixed', 'small', 'resource'] as const) {
    await page.setViewportSize({ width, height: 900 });
    await page.evaluate(async ({ kind, locale }) => {
      const module = '/tests/component/table-first-line-harness.tsx';
      const harness = await import(/* @vite-ignore */ module) as TableHarness;
      await harness.mountTableCase(kind, locale, 'light');
    }, { kind, locale });
    const example = page.locator('[data-table-example]'), table = example.getByRole('table');
    await expect(table).toBeVisible(); await page.mouse.move(0, 0);
    const result = await page.evaluate(inspectLayout, { ...LAYOUT_INPUT, mode: 'icons' } satisfies LayoutInput);
    const geometry = await table.locator('tbody tr').evaluate((row) => {
      const box = (element: Element) => { const value = element.getBoundingClientRect(); return { y: value.y, height: value.height, centre: value.y + value.height / 2 }; };
      const line = (element: Element) => {
        const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT); let node: Node | null;
        while ((node = walker.nextNode()) !== null) {
          if (node.textContent?.trim() === '' || node.parentElement?.closest('[hidden],[aria-hidden="true"],.sr-only') !== null) continue;
          const range = document.createRange(); range.selectNodeContents(node); const value = Array.from(range.getClientRects()).find((one) => one.width > 0 && one.height > 0);
          if (value !== undefined) return { y: value.y, height: value.height, centre: value.y + value.height / 2 };
        }
        return null;
      };
      const name = row.querySelector('[data-table-column="name"]');
      if (name === null) throw new Error('The identity cell is required');
      const strong = name.querySelector('strong'), before = strong?.previousSibling;
      let mixedGap: number | null = null;
      if (strong !== null && before?.nodeType === Node.TEXT_NODE && before.textContent !== null) {
        const range = document.createRange(); range.setStart(before, 0); range.setEnd(before, before.textContent.trimEnd().length);
        const text = range.getBoundingClientRect(), next = strong.getBoundingClientRect();
        if (Math.abs(text.y - next.y) < 1) mixedGap = next.x - text.right;
      }
      const secondary = row.querySelector('[data-secondary] p');
      let secondarySpacing: { lineHeight: number; distances: number[] } | null = null;
      if (secondary !== null) { const range = document.createRange(); range.selectNodeContents(secondary); const tops = [...new Set(Array.from(range.getClientRects()).map((rect) => rect.y))].sort((one, two) => one - two); secondarySpacing = { lineHeight: Number.parseFloat(getComputedStyle(secondary).lineHeight), distances: tops.slice(1).map((top, index) => top - tops[index]!) }; }
      return { name: line(name), text: name.textContent, mixedGap, secondarySpacing, controls: Array.from(row.querySelectorAll('[data-table-column="select"] button,[data-table-column="primary"] button,[data-table-column="acts"] button')).map(box) };
    });
    const outside = await example.evaluate((root) => {
      const first = (element: Element) => { const name = element.querySelector('[data-choice-name]'); if (name === null) throw new Error('A choice label is required'); const range = document.createRange(); range.selectNodeContents(name); const line = range.getClientRects()[0]; if (line === undefined) throw new Error('A visible label line is required'); return line.y + line.height / 2; };
      const choices = ['form', 'compact'].map((kind) => { const owner = root.querySelector(`[data-outside-${kind}]`), control = owner?.querySelector('[role="checkbox"]'); if (owner === null || control === null || control === undefined) throw new Error('The outside choice is required'); const rect = control.getBoundingClientRect(); return { kind, delta: Math.abs(rect.y + rect.height / 2 - first(owner)) }; });
      const chips = ['[data-outside-chip] .resource-chip', '[data-outside-status] [data-status-chip]'].map((selector) => {
        const chip = root.querySelector(selector); if (chip === null) throw new Error('The outside compact reference is required');
        const style = getComputedStyle(chip), number = (value: string) => Number.parseFloat(value) || 0;
        const natural = Math.max(number(style.minHeight), number(style.lineHeight) + number(style.paddingTop) + number(style.paddingBottom) + number(style.borderTopWidth) + number(style.borderBottomWidth));
        return { selector, height: chip.getBoundingClientRect().height, natural };
      });
      return { choices, chips };
    });
    const title = `${width}/${locale}/${kind}`;
    const iconFaults = result.faults.filter((row) => row.rule === 'icon-centre');
    const deltas = geometry.controls.map((box) => geometry.name === null ? Number.POSITIVE_INFINITY : Math.abs(box.centre - geometry.name.centre));
    if (iconFaults.length > 0 || geometry.secondarySpacing === null || geometry.secondarySpacing.distances.some((distance) => Math.abs(distance - geometry.secondarySpacing!.lineHeight) >= 1) || deltas.some((value) => value >= 1) || outside.choices.some((value) => value.delta >= 1) || outside.chips.some((value) => Math.abs(value.height - value.natural) >= 1)) failures.push({ title, iconFaults, deltas, outside });
    if (kind === 'mixed') {
      expect(geometry.text).toContain(locale === 'en' ? 'Complete mixed inline name with every final word and natural wrapping' : '完整的 多行 名称包括末尾的文字和不同格式');
      if (geometry.mixedGap !== null) expect(geometry.mixedGap).toBeGreaterThan(0);
    }
    const checkbox = table.getByRole('checkbox').last(); await checkbox.check(); await expect(example.getByLabel('selection result')).toHaveText('selected');
    receipts.push({ title, geometry, outside, iconFaults });
  }
  await info.attach('table-first-line-receipts', { body: JSON.stringify(receipts, null, 2), contentType: 'application/json' });
  expect(failures).toEqual([]);
});
