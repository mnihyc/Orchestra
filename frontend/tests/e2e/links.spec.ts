/** A built document's Links section: what refers to it with the line around each mention, what it refers to, the graph and the tabs a document shows. */
import { expect, test } from '@playwright/test';
import { scenario, signIn } from './support';
import { actAs, DOCS, held, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

/** Notes mention the plan in a line of words, so that the plan has a source to list. */
async function noteMentionsPlan(): Promise<void> {
  const notes = await held(DOCS.notes);
  const first = notes.blocks[0];
  if (first === undefined) throw new Error('The notes have a block to write in.');
  await actAs('ada', DOCS.notes, [{ op: 'update', id: first.id, base_seq: first.seq_updated, content: [
    { type: 'text', text: 'Decision: see ' }, { type: 'ref', kind: 'document', id: DOCS.plan, label: 'Plan' }, { type: 'text', text: ' for the dates.' },
  ] }]);
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`Links lists a mention, its line and the graph at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await noteMentionsPlan();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await openDocument(page, DOCS.plan);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    const tabs = section.getByRole('tablist', { name: 'Links' });
    await expect(tabs).toBeVisible();
    // The tab bar never wraps: every tab sits on one line at either width.
    const boxes = await tabs.getByRole('tab').evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().top));
    expect(new Set(boxes.map((top) => Math.round(top))).size).toBe(1);
    const list = section.getByRole('list', { name: 'Referenced by' });
    await expect(list.getByRole('link', { name: /Notes/ })).toBeVisible();
    await expect(section.getByText('1 mention')).toBeVisible();
    await section.getByRole('button', { name: /^Show mentions in / }).click();
    const mention = section.getByRole('button', { name: /^Open .* at this mention$/ });
    await expect(mention).toContainText('Decision: see');
    await expect(mention.locator('strong')).toHaveText('Rollout plan');
    await page.screenshot({ path: info.outputPath(`links-referenced-${width}-${theme}.png`), animations: 'disabled' });
    await section.getByRole('tab', { name: /^(References|Cites)$/ }).click();
    await expect(section.getByRole('list', { name: 'References' }).or(section.getByText('This document refers to nothing yet'))).toBeVisible();
    await section.getByRole('tab', { name: 'Graph', exact: true }).click();
    await expect(section.locator('[data-graph-canvas]')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`links-graph-${width}-${theme}.png`), animations: 'disabled' });
    await section.getByRole('button', { name: 'Open graph' }).click();
    await expect(page.locator('[data-graph-page]')).toBeVisible();
    expect(new URL(page.url()).searchParams.get('graph')).toBe('1');
    await page.getByRole('group', { name: 'Graph options' }).getByRole('radio', { name: '2' }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('depth')).toBe('2');
    await page.screenshot({ path: info.outputPath(`links-graph-page-${width}-${theme}.png`), animations: 'disabled' });
  });
}

test('A document that switches a tab off no longer shows it, to anyone', async ({ page }) => {
  await scenario();
  await signIn(page);
  await openDocument(page, DOCS.plan);
  await page.locator('[data-main-area]').getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Links tabs…' }).click();
  const dialog = page.getByRole('dialog', { name: 'Links tabs' });
  await dialog.getByRole('checkbox', { name: 'Graph' }).click();
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await showWorkbenchView(page, 'Links');
  const section = page.locator('[data-section-view="links"]');
  await expect(section.getByRole('tablist', { name: 'Links' })).toBeVisible();
  await expect(section.getByRole('tab', { name: 'Graph', exact: true })).toHaveCount(0);
});
