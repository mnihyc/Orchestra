/** The states of the Links section that are photographed for review: nothing yet, a source the reader may not read, and the tabs dialog open and locked. */
import { expect, test } from '@playwright/test';
import { PEOPLE, scenario, signIn } from './support';
import { actAs, DOCS, held, lockLinksTabs, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

async function mention(source: string, target: string, label: string): Promise<void> {
  const first = (await held(source)).blocks[0];
  if (first === undefined) throw new Error('The source has a block to write in.');
  await actAs('ada', source, [{ op: 'update', id: first.id, base_seq: first.seq_updated, content: [
    { type: 'text', text: 'Decision: see ' }, { type: 'ref', kind: 'document', id: target, label }, { type: 'text', text: ' for the dates.' },
  ] }]);
}

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`References and Related show their readable chips and locked labels at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    const first = (await held(DOCS.plan)).blocks[0];
    if (first === undefined) throw new Error('The plan has a block to write in.');
    await actAs('ada', DOCS.plan, [{ op: 'update', id: first.id, base_seq: first.seq_updated, content: [
      { type: 'ref', kind: 'document', id: DOCS.notes, label: 'Notes' },
      { type: 'text', text: ' and ' },
      { type: 'ref', kind: 'document', id: DOCS.private, label: 'Budget talks' },
    ] }]);
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera);
    await openDocument(page, DOCS.plan);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    await section.getByRole('tab', { name: /^(References|Cites)/ }).click();
    const references = section.getByRole('list', { name: 'References' });
    await expect(references.getByRole('link', { name: /Notes from the canary/ })).toBeVisible();
    await expect(references.getByText('Budget talks', { exact: true })).toBeVisible();
    await expect(references).not.toContainText('Only for me');
    await page.screenshot({ path: info.outputPath(`links-references-data-${width}-${theme}.png`), animations: 'disabled' });
    await section.getByRole('tab', { name: 'Related', exact: true }).click();
    const related = section.getByRole('list', { name: 'Related' });
    await expect(related.getByRole('link', { name: /Notes from the canary/ })).toBeVisible();
    await expect(related).not.toContainText('Only for me');
    await page.screenshot({ path: info.outputPath(`links-related-data-${width}-${theme}.png`), animations: 'disabled' });
  });

  test(`Links says so when there is nothing yet, at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await openDocument(page, DOCS.notes);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    await expect(section.getByText('Nothing refers to this document yet')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`links-empty-referenced-${width}-${theme}.png`), animations: 'disabled' });
    await section.getByRole('tab', { name: /^(References|Cites)$/ }).click();
    await expect(section.getByText('This document refers to nothing yet')).toBeVisible();
    await section.getByRole('tab', { name: 'Graph', exact: true }).click();
    await expect(section.getByText('Nothing is linked yet')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`links-empty-graph-${width}-${theme}.png`), animations: 'disabled' });
  });

  test(`A source the reader may not read leaves no trace in Links, at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await mention(DOCS.notes, DOCS.plan, 'Plan');
    await mention(DOCS.private, DOCS.plan, 'Plan');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page, PEOPLE.vera);
    await openDocument(page, DOCS.plan);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    const list = section.getByRole('list', { name: 'Referenced by' });
    await expect(list.getByRole('link', { name: /Notes from the canary/ })).toBeVisible();
    await expect(list.getByRole('listitem')).toHaveCount(1);
    await expect(section.getByRole('tab', { name: /^(Referenced by|Cited by)/ })).toContainText('1');
    await expect(section).not.toContainText('Only for me');
    await page.screenshot({ path: info.outputPath(`links-hidden-source-${width}-${theme}.png`), animations: 'disabled' });
  });

  test(`The Links tabs dialog, open and locked, at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await openDocument(page, DOCS.plan);
    for (const locked of [false, true]) {
      if (locked) await lockLinksTabs(DOCS.plan);
      await page.reload();
      await expect(page.locator('[data-main-area]')).toBeVisible();
      await page.locator('[data-main-area]').getByRole('button', { name: 'Actions for Rollout plan', exact: true }).click();
      await page.getByRole('menuitem', { name: 'Links tabs…' }).click();
      const dialog = page.getByRole('dialog', { name: 'Links tabs' });
      await expect(dialog).toBeVisible();
      if (locked) await expect(dialog.getByRole('checkbox', { name: 'Graph' })).toBeDisabled();
      else await expect(dialog.getByRole('checkbox', { name: 'Graph' })).toBeEnabled();
      if (locked) await expect(dialog).toContainText('A lock above sets these tabs');
      await page.screenshot({ path: info.outputPath(`links-dialog-${locked ? 'locked' : 'open'}-${width}-${theme}.png`), animations: 'disabled' });
      await page.keyboard.press('Escape');
    }
  });
}
