/** The inspector keeps a crowded graph readable and opens the full graph without dropping its neighbours. */
import { expect, test } from '@playwright/test';

import type { GraphNode, LinkGraph } from '../../src/generated/api-types';
import { scenario, signIn } from './support';
import { DOCS, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

const names = ['Dependency plan', 'Release checklist', 'Service handbook', 'Testing notes'];
const center: GraphNode = { id: DOCS.plan, name: 'Rollout plan', kind: 'document', context: [], current: true, distance: 0 };
const neighbours: GraphNode[] = Array.from({ length: 30 }, (_, at) => ({
  ...center, id: `019a0000-0000-7000-8000-${(at + 100).toString().padStart(12, '0')}`,
  name: names[at] ?? `Working notes ${at.toString().padStart(2, '0')}`, distance: 1, current: false,
}));
const graph: LinkGraph = { center: DOCS.plan, nodes: [center, ...neighbours], edges: neighbours.map((node) => ({ from: node.id, to: DOCS.plan, kind: 'reference' })), truncated: false, meaning: 'off' };

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`A graph with thirty neighbours stays readable at ${width}px in ${theme}`, async ({ page }, info) => {
    await scenario();
    await page.route(`**/documents/${DOCS.plan}/graph?*`, (route) => route.fulfill({ json: graph }));
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await openDocument(page, DOCS.plan);
    await showWorkbenchView(page, 'Links');
    const section = page.locator('[data-section-view="links"]');
    await section.getByRole('tab', { name: 'Graph', exact: true }).click();
    await expect(section.locator('[data-graph-canvas]')).toBeVisible();
    await expect(section.locator('[data-graph-nodes] [role="option"]')).toHaveCount(5);
    await expect(section.getByRole('status')).toHaveText('4 nearest links');
    await expect(section.getByRole('button', { name: 'Open graph' })).toBeVisible();
    await page.screenshot({ path: info.outputPath(`links-thirty-preview-${width}-${theme}.png`), animations: 'disabled' });
    await section.getByRole('button', { name: 'Open graph' }).click();
    const full = page.locator('[data-graph-page]');
    await expect(full).toBeVisible();
    await expect(full.locator('[data-graph-nodes] [role="option"]')).toHaveCount(31);
    await expect(full.getByRole('listbox', { name: 'Nodes' })).toContainText('Working notes 29');
    await page.screenshot({ path: info.outputPath(`links-thirty-full-${width}-${theme}.png`), animations: 'disabled' });
  });
}
