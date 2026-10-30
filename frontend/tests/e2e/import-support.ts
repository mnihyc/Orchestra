/** A browser fixture for the import UI; actual document creation still goes through the test world's producer. */
import type { Page } from '@playwright/test';
import type { Import, ImportPlan, ImportReportRow, ImportRequest, Job } from '../../src/generated/api-types.ts';

const IMPORT = '00000000-0000-4000-802a-000000000101';
const JOB = '00000000-0000-4000-802a-000000000102';

/** Supplies the new plan and job responses while delegating document creation to the existing fake API. */
export async function installImportFixture(page: Page): Promise<void> {
  let request: ImportRequest | null = null;
  let made: { id: string; title: string } | null = null;
  const node = { id: '', kind: 'topic' as const, name: 'Fix login', redacted: false };
  const counts = { places: 0, folders: 0, documents: 1, chats: 0, threads: 0, files: 0, messages: 0, turns: 0, comments: 0, versions: 0 };
  const planned = (): ImportPlan => ({
    node, source: { mode: 'files', entries: 1, bytes: 64, package: null }, counts,
    actions: { create: 1, keep_both: 0, replace: 0, skip: 0 }, conflicts: 0, bytes: 64, limits: { max_bytes: 2_147_483_648, max_items: 10_000 },
    authors: 'labels', access: { carried: false, allowed: false, restored: false }, people: [], losses: [], blockers: [],
    objects: { items: [{ source_id: '00000000-0000-4000-802a-000000000103', kind: 'document', name: 'Imported plan', parent: null, path: [], bytes: 64, action: 'create', allowed: ['create', 'skip'], existing: null, blocked: null, creator: null, created_at: null }], next_cursor: null },
  });
  const job = (): Job => ({ id: JOB, kind: 'import', state: 'succeeded', result: { import_id: IMPORT }, node_id: node.id,
    batch_id: null, created_by: null, environment_id: null, provider_id: null, error: null, log: null,
    created_at: '2026-10-28T12:00:00Z', started_at: '2026-10-28T12:00:01Z', finished_at: '2026-10-28T12:00:02Z',
    progress: { phase: 'report', percent: 100, bytes_done: 64, bytes_total: 64, files_done: 1, files_total: 1, eta_seconds: 0, rate_bps: null, message: null, phases: [] } });
  const record = (): Import => {
    if (request === null) throw new Error('The import was not started');
    return { id: IMPORT, node, request, state: 'done', job_id: JOB, undo_job_id: null, files: request.source.files.map((file) => file.name), package: null,
      counts, actions: { create: 1, keep_both: 0, replace: 0, skip: 0 }, losses: [], failure: null,
      created_at: '2026-10-28T12:00:00Z', finished_at: '2026-10-28T12:00:02Z', undone_at: null };
  };
  await page.route('**/api/v1/nodes/*/imports/preflight', async (route) => {
    const match = /\/nodes\/([^/]+)\/imports\/preflight$/.exec(new URL(route.request().url()).pathname);
    node.id = match?.[1] ?? '';
    await route.fulfill({ status: 200, json: planned() });
  });
  await page.route('**/api/v1/nodes/*/imports', async (route) => {
    request = route.request().postDataJSON() as ImportRequest;
    const legacy = new URL(route.request().url());
    legacy.pathname = legacy.pathname.replace(/\/imports$/, '/documents/import');
    const headers = await route.request().allHeaders();
    const response = await route.fetch({ url: legacy.toString(), method: 'POST', headers: { 'content-type': 'application/json', ...(headers.authorization === undefined ? {} : { authorization: headers.authorization }) },
      postData: JSON.stringify({ client_id: request.client_id, files: request.source.files.map((file) => ({ ...file, format: 'markdown' })) }) });
    if (!response.ok()) { await route.fulfill({ response }); return; }
    const result = await response.json() as { entries: { document: { id: string; title: string } | null }[] };
    made = result.entries[0]?.document ?? null;
    if (made === null) throw new Error('The document producer did not create a document');
    await route.fulfill({ status: 202, json: job() });
  });
  await page.route('**/api/v1/me/imports*', async (route) => {
    const lines = request === null ? [] : [record()].map(({ id, node, state, job_id, undo_job_id, files, actions, created_at, finished_at, undone_at }) => ({
      id, node, state, job_id, undo_job_id, first_file: files[0] ?? null, file_count: files.length, actions, created_at, finished_at, undone_at }));
    await route.fulfill({ status: 200, json: { items: lines, next_cursor: null } });
  });
  await page.route(`**/api/v1/imports/${IMPORT}**`, async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/report')) {
      if (made === null) throw new Error('The report has no document');
      const row: ImportReportRow = { position: 0, source_id: '00000000-0000-4000-802a-000000000103', kind: 'document', name: made.title, path: [], action: 'create', outcome: 'created', made: { id: made.id, kind: 'document', node: null }, message: null, warnings: [], author: null };
      await route.fulfill({ status: 200, json: { items: [row], next_cursor: null } });
    } else await route.fulfill({ status: 200, json: record() });
  });
}
