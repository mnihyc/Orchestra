/** Production edit/review UI with exact native-shaped responses; native receipt semantics have separate DB proofs. */
import { expect, test } from '@playwright/test';
import type { DocumentDiscussion } from '../../src/generated/api-types';
import { scenario, signIn } from './support';
import { DOCS, openDocument } from './documents-support';
import { showWorkbenchView } from './workbench-support';

for (const theme of ['light', 'dark'] as const) test(`a stale comment draft reviews its captured revision before a separate Send in ${theme}`, async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await page.emulateMedia({ colorScheme: theme });
  await scenario(); await signIn(page); await openDocument(page, DOCS.plan); await showWorkbenchView(page, 'Comments');
  const panel = page.getByRole('region', { name: 'Comments on this document' });
  await panel.getByRole('button', { name: 'Start a discussion', exact: true }).click();
  await panel.getByRole('textbox', { name: 'New discussion' }).fill('Original comment');
  const created = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith(`/documents/${DOCS.plan}/discussions`));
  await panel.getByRole('button', { name: 'Send', exact: true }).click();
  const discussion = await (await created).json() as DocumentDiscussion;
  const original = discussion.comments[0]; if (original === undefined) throw new Error('The UI-created discussion must contain its actual comment');
  await expect(panel.getByRole('region', { name: 'New discussion', exact: true })).toHaveCount(0);
  const row = page.locator(`[data-comment="${original.id}"]`);
  const current = { ...original, content: [{ type: 'text', text: 'Newer server comment' }], seq_updated: original.seq_updated + 1, edited_at: '2026-10-17T12:00:00Z' };
  const seen: { clientId: string; base: number; content: unknown[] }[] = [];
  await page.route(`**/api/v1/documents/${DOCS.plan}/comments/${original.id}`, async (route) => {
    if (route.request().method() !== 'PATCH') { await route.continue(); return; }
    const raw: unknown = route.request().postDataJSON();
    if (raw === null || typeof raw !== 'object' || !('client_id' in raw) || typeof raw.client_id !== 'string'
      || !('expected_seq' in raw) || typeof raw.expected_seq !== 'number' || !('content' in raw) || !Array.isArray(raw.content)) throw new Error('The real edit must send its paired request identity and authored revision');
    const content: unknown[] = raw.content; seen.push({ clientId: raw.client_id, base: raw.expected_seq, content });
    if (seen.length === 1) await route.fulfill({ status: 409, json: { type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', status: 409,
      title: 'Changed', detail: 'The comment changed after this draft was begun.', request_id: 'browser-comment-review', current_comment: current } });
    else await route.fulfill({ json: { ...current, content: [{ type: 'text', text: 'My revised comment' }], seq_updated: current.seq_updated + 1 } });
  });
  await row.getByRole('button', { name: 'Edit', exact: true }).click();
  const field = row.getByRole('textbox', { name: 'Edit', exact: true }); await field.fill('My revised comment');
  await row.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(row.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
  await row.getByRole('button', { name: 'Review edit', exact: true }).click();
  const review = page.getByRole('dialog', { name: 'Review edit', exact: true });
  await expect(review.getByText('Newer server comment', { exact: true })).toBeVisible();
  await expect(review.getByText('My revised comment', { exact: true })).toBeVisible();
  expect(seen).toHaveLength(1); expect(seen[0]?.base).toBe(original.seq_updated);
  await page.screenshot({ path: info.outputPath(`comment-edit-review-${theme}.png`), animations: 'disabled' });
  await review.getByRole('button', { name: 'Keep draft on this version', exact: true }).click();
  expect(seen).toHaveLength(1); await expect(field).toHaveValue('My revised comment');
  await row.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(field).toHaveCount(0); await expect(row).toContainText('My revised comment');
  expect(seen).toHaveLength(2); expect(seen[1]?.base).toBe(current.seq_updated);
  expect(seen[1]?.clientId).not.toBe(seen[0]?.clientId); expect(seen[1]?.content).toEqual([{ type: 'text', text: 'My revised comment' }]);
});
