/** Routes not implemented by the HTTP double reuse the typed data of their existing browser proofs. */
import type { Page } from '@playwright/test';
import type { AdminMessage } from '../src/generated/api-types.ts';
import { me, userRef } from '../tests/doubles/fixtures.ts';
import { ADMIN_MESSAGE_ID } from './layout-routes.ts';
import { readStorageSearch, storageRequest } from '../src/features/storage/search.ts';
import { storageReport } from '../tests/doubles/storage-report.ts';
import { expect } from '@playwright/test';
import type { LayoutRoute, SCREEN_PRESENTATIONS } from './layout-routes.ts';
import { documentTreeFixture } from '../tests/e2e/document-tree-support.ts';
import { replySent, signIn } from '../tests/e2e/support.ts';
import { asAdmin } from '../tests/e2e/admin-mode-support.ts';
import { caption } from './capture/open.ts';
import { showWorkbenchView } from '../tests/e2e/workbench-support.ts';

/** Persist the requested appearance before the measured document has any client cache. */
export async function signInScreenRoute(page: Page, mode: (typeof SCREEN_PRESENTATIONS)[number]): Promise<void> {
  await page.setViewportSize({ width: mode.width, height: 900 });
  await page.emulateMedia({ colorScheme: mode.theme, reducedMotion: 'reduce' });
  const login = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
  await signIn(page); const auth: unknown = await login;
  if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The screen fixture needs its fake sign-in result.');
  await asAdmin(page, true);
  expect((await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme: mode.theme, locale: mode.locale } })).ok()).toBe(true);
}

/** Install state before navigation; browser reads still pass through the ordinary fake API. */
export async function prepareLayoutRoute(page: Page, row: LayoutRoute): Promise<void> {
  if (row.pattern === '/messages/$id') {
    // Reuse the received-message contract exercised by the administrator-message browser proofs.
    const message: AdminMessage = { id: ADMIN_MESSAGE_ID, org: null, title: 'A message for everyone',
      body: '## What changed\n\nRead this **important** change.', sender: userRef(me(1, 'ada', 'Ada Lovelace')),
      created_at: '2026-11-08T12:00:00Z', recipient_count: 4, delivery_job_id: null };
    await page.route(`**/api/v1/messages/${ADMIN_MESSAGE_ID}`, (route) => route.fulfill({ json: message }));
    return;
  }
  if (row.fixture === 'document-tree') { await documentTreeFixture(page); return; }
  if (row.fixture === undefined) return;
  const response = await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/screen-gate`, { data: { name: row.fixture } });
  expect(response.ok(), `Install ${row.fixture}`).toBe(true);
}

/** Assert the requested state exists, so an empty/loading/error screen cannot satisfy the gate. */
export async function openLayoutRoute(page: Page, row: LayoutRoute): Promise<void> {
  const locale = await page.locator('html').getAttribute('lang') === 'zh-CN' ? 'zh-CN' : 'en';
  const words = (key: string) => caption(`@${key}`, locale);
  if (row.fixture === 'thread-options-allowed' || row.fixture === 'thread-options-fixed') {
    await showWorkbenchView(page, 'Parameters');
    const parameters = page.getByTestId('parameters-panel');
    await expect(parameters.getByRole('combobox', { name: words('webSearch.title'), exact: true })).toHaveValue(words('choiceCatalog.webSearch.live.name'));
    if (row.fixture === 'thread-options-fixed') {
      await expect(parameters.getByRole('textbox', { name: 'Verbosity', exact: true })).toBeDisabled();
      await expect(parameters).toContainText(words('providerOptions.fixedByProvider'));
    } else await expect(parameters.getByRole('combobox', { name: 'Verbosity', exact: true })).toHaveValue('normal');
  } else if (row.fixture === 'provider-search-models' || row.fixture === 'provider-search-editor') {
    await expect(page.getByRole('row').filter({ has: page.getByText('Screen model', { exact: true }) })).toBeVisible();
    if (row.fixture === 'provider-search-editor') {
      await page.getByRole('button', { name: words('providers.models.menuOf').replace('{{name}}', 'Screen model'), exact: true }).click();
      await page.getByRole('menuitem', { name: words('models.list.edit'), exact: true }).click();
      const editor = page.getByRole('dialog', { name: words('models.editor.title'), exact: true });
      await expect(editor.getByRole('group', { name: words('webSearch.title'), exact: true }).getByRole('switch', { name: words('webSearch.modeSwitch').replace('{{mode}}', words('choiceCatalog.webSearch.live.name')), exact: true })).toBeChecked();
      const raw = editor.locator('[data-option-schema-editor] details').filter({ hasText: words('providerFields.raw') });
      if (await raw.getAttribute('open') === null) await raw.locator('summary').click();
      await expect(editor.getByRole('textbox', { name: words('providerFields.raw'), exact: true })).toHaveValue(/"native"/u);
      await expect(editor.getByRole('textbox', { name: `${words('webSearch.price')} ${words('ui.field.optional')}`, exact: true })).toHaveValue('0.01');
    }
  } else if (row.fixture === 'agent-share-ladders' || row.fixture === 'agent-access-ladders') {
    await page.locator('[data-thread-header] [data-thread-more]').click();
    await page.getByRole('menuitem', { name: words('thread.header.shareThread'), exact: true }).click();
    const access = page.getByRole('dialog').filter({ has: page.locator('[data-tutorial-target="permission-page"]') });
    await access.getByRole('tab', { name: words('access.tabs.agents'), exact: true }).click();
    const recorded = access.getByRole('list', { name: words('access.agents.shared'), exact: true });
    await expect(recorded.getByRole('combobox', { name: words('access.agents.area.timeline'), exact: true })).toContainText(words('choiceCatalog.agentStep.manage.name'));
    if (row.fixture === 'agent-access-ladders') {
      await recorded.getByText(words('access.advanced.title'), { exact: true }).click();
      await expect(recorded.getByRole('checkbox', { name: words('access.agents.fork'), exact: true })).toBeChecked();
      await expect(recorded.getByRole('checkbox', { name: words('access.agents.archive'), exact: true })).toBeChecked();
    } else {
      await access.getByRole('button', { name: words('access.agents.share'), exact: true }).click();
      const form = page.getByRole('dialog').filter({ has: page.getByRole('button', { name: words('access.agents.dialog.byArea'), exact: true }) });
      await form.getByRole('button', { name: words('access.agents.dialog.byArea'), exact: true }).click();
      for (const area of ['timeline', 'chat', 'documents']) {
        await form.getByRole('combobox', { name: words(`access.agents.area.${area}`), exact: true }).click();
        await page.getByRole('option', { name: words('choiceCatalog.agentStep.manage.name'), exact: true }).click();
      }
      await form.getByText(words('access.advanced.title'), { exact: true }).click();
      await expect(form.getByRole('checkbox', { name: words('access.agents.fork'), exact: true })).toBeChecked();
      await expect(form.getByRole('checkbox', { name: words('access.agents.archive'), exact: true })).toBeChecked();
    }
  } else if (row.fixture?.startsWith('gateways-')) {
    const catalogue = page.getByRole('region', { name: words('regionalGateways.title'), exact: true });
    await expect(catalogue).toContainText('Europe gateway');
    await expect(catalogue).toContainText('Asia tools');
    if (row.fixture === 'gateways-edit') {
      await catalogue.getByRole('button', { name: words('regionalGateways.edit'), exact: true }).click();
      await expect(page.getByRole('dialog', { name: words('regionalGateways.editTitle'), exact: true })).toBeVisible();
    }
  } else if (row.fixture?.startsWith('gateway-')) {
    const card = page.getByRole('region', { name: words('regionalGateways.connector'), exact: true });
    await expect(card).toBeVisible();
    if (row.fixture === 'gateway-automatic') await expect(card.getByRole('list', { name: words('regionalGateways.measurements') })).toContainText('Europe gateway');
    if (row.fixture === 'gateway-offline') await expect(card).toContainText(words('regionalGateways.offline'));
    if (row.fixture === 'gateway-unreported') await expect(card).toContainText(words('regionalGateways.notReported'));
  } else if (row.fixture === 'tunnel-attached') {
    await expect(page.locator('[data-screen="thread"]').getByLabel(words('regionalGateways.toolConnection'), { exact: true })).toContainText('Asia tools');
  } else if (row.fixture === 'tunnel-unavailable') {
    await expect(page.getByRole('list', { name: words('regionalGateways.attempted') })).toContainText('Europe gateway');
  }
  if (row.fixture?.startsWith('background-')) {
    const header = page.locator('[data-thread-header]'), notes = page.locator('[data-thread-notes]');
    await expect(header).toBeVisible();
    if (row.fixture === 'background-finished') await expect(page.locator('[data-background-wake="admitted"]')).toContainText('Build web');
    else if (row.fixture === 'background-held') await expect(page.locator('[data-background-wake="held"]')).toContainText('Build web');
    else {
      await expect(notes.locator('[data-background-task]')).toHaveCount(1);
      await expect(header.locator('[data-fold="agents"]')).toHaveCount(1);
      await expect(notes.locator('[data-background-wake-notice]')).toBeVisible();
      await expect(notes.locator('[data-background-wake-notice]')).toContainText(words(row.fixture === 'background-off' ? 'backgroundWake.nextInput' : row.fixture === 'background-lost' ? 'backgroundWake.lost' : 'backgroundWake.willWake'));
      if (row.fixture === 'background-lost') await expect(notes.locator('[data-background-task]')).toHaveAttribute('data-background-state', 'lost');
    }
  }
}

/** Healthy read-only report pages; writes and other paths keep the ordinary HTTP double. */
export async function personalReportFixtures(page: Page): Promise<void> {
  await page.route(/\/api\/v1\/storage(?:\?|$)/u, async (read) => {
    if (read.request().method() !== 'GET') { await read.fallback(); return; }
    const query = storageRequest(readStorageSearch(Object.fromEntries(new URL(read.request().url()).searchParams)));
    await read.fulfill({ status: 200, json: storageReport(query) });
  });
  await page.route(/\/api\/v1\/me\/imports(?:\?|$)/u, async (read) => {
    if (read.request().method() !== 'GET') { await read.fallback(); return; }
    await read.fulfill({ status: 200, json: { items: [], next_cursor: null } });
  });
}

export async function resourceFixtures(page: Page): Promise<void> {
  await personalReportFixtures(page);
  // HTTP, aggregate and live reads share the native records and current-reader checks.
  const native = await page.request.post('http://127.0.0.1:' + (process.env.ORCH_DOUBLE_PORT ?? '41871') + '/__double/request-feeds/fixtures');
  if (!native.ok()) throw new Error('The native resource fixtures were not installed.');
}
