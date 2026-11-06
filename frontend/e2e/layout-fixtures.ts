/** Routes not implemented by the HTTP double reuse the typed data of their existing browser proofs. */
import type { Page } from '@playwright/test';
import { readStorageSearch, storageRequest } from '../src/features/storage/search.ts';
import { storageReport } from '../tests/doubles/storage-report.ts';

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
