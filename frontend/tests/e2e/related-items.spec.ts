/** A built document inspector loads one related list, keeps it by revision and opens only the requested reference card; Related is a tab of the Links section. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';
import { DOCS, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`Related is bounded and lazy at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    let lists = 0, cards = 0;
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (request.method() !== 'GET') return;
      if (url.pathname === `/api/v1/documents/${DOCS.plan}/related`) lists += 1;
      if (url.pathname === '/api/v1/references/preview' && url.searchParams.get('id') === DOCS.notes) cards += 1;
    });
    await openDocument(page, DOCS.plan);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    await section.getByRole('tab', { name: 'Related', exact: true }).click();
    const list = section.getByRole('list', { name: 'Related' });
    const chip = list.getByRole('link', { name: 'Notes from the canary', exact: true });
    await expect(chip).toBeVisible();
    await expect.poll(() => lists).toBe(1);
    expect(cards).toBe(0);
    const bounds = await list.boundingBox();
    if (bounds === null) throw new Error('The opened related list must have visible bounds.');
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
    const bar = page.getByRole('region', { name: 'Right bar', exact: true });
    await bar.getByRole('button', { name: width === 400 ? 'Close' : 'Hide the right bar', exact: true }).click();
    await page.mouse.move(0, 0);
    await expect(chip).not.toBeVisible();
    await showWorkbenchView(page, 'Links');
    await section.getByRole('tab', { name: 'Related', exact: true }).click();
    await page.mouse.move(0, 0);
    await expect(chip).toBeVisible();
    expect(lists).toBe(1);
    await page.screenshot({ path: info.outputPath(`related-list-${width}-${theme}.png`), animations: 'disabled' });
    await chip.focus();
    await page.keyboard.press('Enter');
    const card = page.locator('[data-reference-card]');
    await expect(card).toContainText('Notes from the canary');
    await expect(card.getByRole('button', { name: 'Open', exact: true })).toBeVisible();
    await expect.poll(() => cards).toBe(1);
    expect(new URL(page.url()).pathname).toBe(`/doc/${DOCS.plan}`);
    await page.screenshot({ path: info.outputPath(`related-${width}-${theme}.png`), animations: 'disabled' });
    await card.getByRole('button', { name: 'Dora Developer', exact: true }).focus();
    const person = page.locator('[data-inspection-card][aria-label="Dora Developer"]');
    await expect(person).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(person).toHaveCount(0);
    await expect(card).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(card).toHaveCount(0);
  });
}
