/** Own click paths through the actual screens. Recorded HTTP fixtures prove presentation, not native ACLs. */
import { expect, test, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { artifactStorageReport } from '../doubles/artifact-storage';
import { reachActivity, reachExport, reachImport, reachImportRow, reachPerson, reachService, reachSession, reachSnapshot, reachToken, reachUpload } from '../doubles/admin-reach';
import type { KeptRecord } from '../../src/generated/api-types';
import { RUNS_ON } from '../doubles/fixtures';
import { asAdmin } from './admin-mode-support';
import { DOUBLE, scenario, signIn } from './support';

async function appearance(page: Page, locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  const reply = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }), signal: AbortSignal.timeout(10_000) });
  expect(reply.ok).toBe(true); await page.reload(); await screenReady(page, locale, theme);
}
async function bounded(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  for (const panel of await page.getByRole('dialog').all()) {
    const box = await panel.boundingBox(); expect(box).not.toBeNull();
    if (box !== null) { expect(box.x).toBeGreaterThanOrEqual(0); expect(box.y).toBeGreaterThanOrEqual(0); expect(box.x + box.width).toBeLessThanOrEqual((page.viewportSize()?.width ?? 0) + 1); }
  }
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  const text = (key: string, values: Record<string, string> = {}) => caption(`@${key}`, locale).replace(/\{\{(\w+)\}\}/gu, (whole: string, name: string) => values[name] ?? whole);
  test(`admin tokens and a person’s sessions use their actual doors (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await scenario(); await signIn(page); await asAdmin(page, true);
    const token = reachToken(), service = reachToken({ info: { ...token.info, id: reachService.id, name: 'Service key' }, owner: reachService });
    const writes: string[] = []; let tokens = [token, service], sessions = [reachSession()];
    await page.route(/\/api\/v1\/admin\/(?:tokens|users\/[^/]+\/(?:tokens|sessions))(?:[/?]|$)/u, async route => {
      const path = new URL(route.request().url()).pathname;
      if (route.request().method() === 'DELETE') {
        writes.push(path); if (path.endsWith('/sessions')) sessions = []; else tokens = tokens.filter(row => !path.endsWith(`/${row.info.id}`));
        await route.fulfill({ status: 204 }); return;
      }
      await route.fulfill({ status: 200, json: { items: path.endsWith('/activity') ? [reachActivity] : path.endsWith('/sessions') ? sessions : tokens, next_cursor: null } });
    });
    await page.goto('/admin/tokens'); await appearance(page, locale, theme);
    const table = page.getByRole('table', { name: text('admin.sections.tokens.title'), exact: true });
    await expect(table.getByText('Build service', { exact: true })).toBeVisible();
    await table.getByRole('button', { name: token.info.name, exact: true }).click();
    let dialog = page.getByRole('dialog', { name: token.info.name, exact: true });
    await expect(dialog.getByText(text('operationKeys.denyAll'), { exact: true })).toBeVisible();
    await dialog.getByRole('radio', { name: text('operationKeys.activity.title'), exact: true }).click();
    await expect(dialog.getByText('DocumentGet', { exact: true })).toBeVisible(); await bounded(page);
    await page.screenshot({ path: info.outputPath('token-activity.png') });
    await dialog.getByRole('button', { name: text('common.close'), exact: true }).last().click();
    await page.goto('/admin/users'); await screenReady(page, locale, theme);
    await page.getByRole('button', { name: text('admin.users.menuOf', { name: 'Ada Lovelace' }), exact: true }).click();
    await page.getByRole('menuitem', { name: text('adminCredentials.sessions'), exact: true }).click();
    dialog = page.getByRole('dialog', { name: text('adminCredentials.sessions'), exact: true });
    await expect(dialog.getByText('Chrome (Linux)', { exact: true })).toBeVisible();
    await expect(dialog.getByText(text('security.sessions.thisOne'), { exact: true })).toBeVisible();
    await bounded(page); await page.screenshot({ path: info.outputPath('person-sessions.png') });
    await dialog.getByRole('button', { name: text('adminCredentials.endAll'), exact: true }).click();
    let confirm = page.getByRole('alertdialog');
    await expect(confirm).toContainText(text('adminCredentials.endAllBody'));
    await confirm.getByRole('button', { name: text('common.cancel'), exact: true }).click(); expect(writes).toEqual([]);
    await dialog.getByRole('button', { name: text('adminCredentials.endAll'), exact: true }).click();
    confirm = page.getByRole('alertdialog'); await confirm.getByRole('button', { name: text('adminCredentials.endAll'), exact: true }).click();
    await expect(dialog.getByText(text('adminCredentials.noSessions'), { exact: true })).toBeVisible();
    expect(writes).toEqual([`/api/v1/admin/users/${reachPerson.id}/sessions`]);
    await dialog.getByRole('button', { name: text('common.close'), exact: true }).last().click();
    await page.getByRole('button', { name: text('admin.users.menuOf', { name: 'Ada Lovelace' }), exact: true }).click();
    await page.getByRole('menuitem', { name: text('admin.sections.tokens.title'), exact: true }).click();
    dialog = page.getByRole('dialog', { name: text('admin.sections.tokens.title'), exact: true });
    const row = dialog.getByRole('row').filter({ has: page.getByRole('button', { name: token.info.name, exact: true }) });
    await row.getByRole('button', { name: text('tokens.revoke'), exact: true }).click();
    confirm = page.getByRole('alertdialog');
    await confirm.getByRole('button', { name: text('tokens.revokeConfirm', { name: token.info.name }), exact: true }).click();
    await expect(row).toHaveCount(0);
    expect(writes.at(-1)).toBe(`/api/v1/admin/users/${reachPerson.id}/tokens/${token.info.id}`);
  });

  test(`personal uploads and Storage open native artifacts (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await scenario(); await signIn(page); await asAdmin(page, true);
    let own = [reachUpload()]; const contentPaths: string[] = [];
    await page.route(/\/api\/v1\/me\/uploads(?:\?|$)/u, route => route.fulfill({ status: 200, json: { items: own, next_cursor: null } }));
    await page.route(/\/api\/v1\/uploads\/blobs\/[^/?]+(?:\?|$)/u, async route => {
      if (route.request().method() === 'DELETE') { own = []; await route.fulfill({ status: 204 }); }
      else await route.continue();
    });
    const report = artifactStorageReport();
    const upload = reachUpload({ admin_access: { place: 'system', private: true }, can_open: true, can_delete: false });
    const partial = reachUpload({ id: 'machine-part', kind: 'machine', name: 'pending.bin', state: 'partial', received: 128, content_id: null, can_open: false, can_delete: false });
    const template = report.items[0]; if (template === undefined) throw new Error('Native storage fixture needs its artifact row');
    const uploadRows: KeptRecord[] = [upload, partial].map(row => ({ ...template, id: row.id, kind: 'upload', artifact: null, upload: row, name: row.name ?? '', owner: row.uploader, bytes: row.received, accounting_key: row.id, physical_parts: [{ key: row.id, bytes: row.received }] }));
    await page.route(/\/api\/v1\/storage(?:\?|$)/u, route => route.fulfill({ status: 200, json: { ...report, items: [...report.items, ...uploadRows], totals: { items: 5, bytes: 1408, unknown: 0 } } }));
    await page.route(/\/api\/v1\/exports\/[^/?]+(?:\?|$)/u, route => route.fulfill({ status: 200, json: reachExport() }));
    await page.route(/\/api\/v1\/snapshots\/[^/?]+(?:\?|$)/u, route => route.fulfill({ status: 200, json: reachSnapshot() }));
    await page.route(/\/api\/v1\/imports\/[^/?]+(?:\/report)?(?:\?|$)/u, route => route.fulfill({ status: 200, json: new URL(route.request().url()).pathname.endsWith('/report') ? { items: [reachImportRow], next_cursor: null } : reachImport() }));
    await page.route(/\/api\/v1\/admin\/uploads\/[^/]+\/content(?:\?|$)/u, async route => {
      const url = new URL(route.request().url()); contentPaths.push(url.pathname);
      await route.fulfill(url.searchParams.get('preview') === 'true'
        ? { status: 200, json: { state: 'ready', hash: 'a'.repeat(64), size: 12, media_type: 'text/plain', is_text: true, text: 'Stored notes', image: null, width: null, height: null, duration_seconds: null, entry_count: null } }
        : { status: 200, contentType: 'text/plain', body: 'Stored notes' });
    });
    await page.goto('/storage?view=uploads'); await appearance(page, locale, theme);
    const ownTable = page.getByRole('table', { name: text('adminArtifacts.uploads'), exact: true });
    await expect(ownTable.getByText('orphan.txt', { exact: true })).toBeVisible(); await bounded(page);
    await ownTable.getByRole('button', { name: text('adminArtifacts.delete'), exact: true }).click();
    const confirm = page.getByRole('alertdialog'); await expect(confirm).toContainText(text('adminArtifacts.releaseUploadBody'));
    await confirm.getByRole('button', { name: text('adminArtifacts.delete'), exact: true }).click();
    await expect(page.getByText(text('adminArtifacts.noUploads'), { exact: true })).toBeVisible();
    await page.goto('/storage?scope=system'); await screenReady(page, locale, theme);
    const table = page.getByRole('table', { name: text('storage.records'), exact: true });
    const incomplete = table.getByRole('row').filter({ hasText: 'pending.bin' });
    await expect(incomplete.getByText(text('adminArtifacts.uploadStates.partial'), { exact: true })).toBeVisible();
    await expect(incomplete.getByRole('button', { name: text('adminArtifacts.open'), exact: true })).toHaveCount(0);
    await table.getByRole('row').filter({ hasText: 'orphan.txt' }).getByRole('button', { name: text('adminArtifacts.open'), exact: true }).click();
    let dialog = page.getByRole('dialog', { name: 'orphan.txt', exact: true });
    await expect(dialog.getByText('Stored notes', { exact: true })).toBeVisible();
    expect(contentPaths.length).toBeGreaterThan(0); expect(new Set(contentPaths)).toEqual(new Set([`/api/v1/admin/uploads/${upload.content_id}/content`]));
    await bounded(page); await page.screenshot({ path: info.outputPath('admin-upload.png') });
    await dialog.getByRole('button', { name: text('common.close'), exact: true }).last().click();
    for (const [name, kind] of [['Export package', 'export'], ['reviewed.md', 'import'], ['Workspace snapshot', 'snapshot']] as const) {
      await table.getByRole('row').filter({ hasText: name }).getByRole('button', { name: text('adminArtifacts.open'), exact: true }).click();
      dialog = page.getByRole('dialog', { name: text(`storage.kinds.${kind}`), exact: true });
      if (kind === 'export') await expect(dialog.locator('pre')).toContainText('made_by');
      else if (kind === 'import') { await expect(dialog.getByText('Reviewed page', { exact: true })).toBeVisible(); await expect(dialog.getByRole('button', { name: text('importWizard.undo.action'), exact: true })).toHaveCount(0); }
      else await expect(dialog.getByText('Ada', { exact: true }).first()).toBeVisible();
      await bounded(page); await page.screenshot({ path: info.outputPath(`${kind}.png`) });
      await dialog.getByRole('button', { name: text('common.close'), exact: true }).last().click();
    }
  });

  test(`Files discloses recorded workspace snapshots (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 900 }); await scenario(); await signIn(page); await asAdmin(page, true);
    const reads: string[] = [];
    await page.route(`**/api/v1/environments/${RUNS_ON.workspace}/snapshots*`, route => {
      reads.push(route.request().url()); return route.fulfill({ status: 200, json: { items: [reachSnapshot({ state: 'partial', missing_count: 2 })], next_cursor: null } });
    });
    await page.goto(`/resources/environments/${RUNS_ON.workspace}?tab=files`); await appearance(page, locale, theme);
    expect(reads).toEqual([]);
    await page.locator('summary').filter({ hasText: text('adminArtifacts.snapshots') }).click();
    const table = page.getByRole('table', { name: text('adminArtifacts.snapshots'), exact: true });
    await expect(table.getByText('Workspace snapshot', { exact: true })).toBeVisible();
    await expect(table.getByText(text('adminArtifacts.missingFiles', { number: '2' }), { exact: true })).toBeVisible();
    await expect(table.getByRole('button', { name: text('temporary.download'), exact: true })).toBeVisible();
    expect(reads).toHaveLength(1); await bounded(page); await page.screenshot({ path: info.outputPath('environment-snapshots.png') });
  });
}
