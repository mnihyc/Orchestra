/** Routes not implemented by the HTTP double reuse the typed data of their existing browser proofs. */
import type { Page } from '@playwright/test';
import { readStorageSearch, storageRequest } from '../src/features/storage/search.ts';
import { storageReport } from '../tests/doubles/storage-report.ts';
import type { Skill, SkillVersion, SkillVersionInfo, SkillVersionInfoPage } from '../src/generated/api-types.ts';
import { ACME, RUNS_ON } from '../tests/doubles/fixtures.ts';
import { ADA } from '../tests/doubles/thread-scenes.ts';
import { SKILL_ID } from './layout-routes.ts';

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
  // HTTP and live reads use the same native MCP/mount records and current-reader checks.
  const native = await page.request.post('http://127.0.0.1:' + (process.env.ORCH_DOUBLE_PORT ?? '41871') + '/__double/request-feeds/fixtures');
  if (!native.ok()) throw new Error('The native resource fixtures were not installed.');
  const home = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };
  const origin = { kind: 'git' as const, repository: 'https://git.example.test/acme/skills.git', reference: 'main',
    commit: '0123456789abcdef0123456789abcdef01234567', folder: 'pdf-tools', host_id: RUNS_ON.buildHost };
  const latest: SkillVersionInfo = { id: '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a63', number: 2, name: 'pdf-tools', description: 'Reads and fills PDF forms.',
    size: 2048, file_count: 2, archive: { blob: 'ab'.repeat(32), size: 1024 }, origin, created_by: ADA, created_at: '2026-10-14T08:00:00Z' };
  const skill: Skill = { id: SKILL_ID, name: 'pdf-tools', description: 'Reads and fills PDF forms.', org_id: ACME.org, owner: ADA, home,
    address: 'acme.pdf-tools', enabled: true, latest, versions: 2, origin, draft: null, revision: 1,
    access: { admin_only: [], participating: true, permissions: ['skill.view', 'skill.use', 'skill.manage'], administers: false, through: [{ kind: 'owner' }] } };
  const files: SkillVersion = { version: latest, files: [
    { path: 'SKILL.md', size: 1536, sha256: 'cd'.repeat(32), executable: false },
    { path: 'scripts/fill.py', size: 512, sha256: 'ef'.repeat(32), executable: true }] };
  const versions: SkillVersionInfoPage = { items: [latest], next_cursor: null };
  await page.route('**/api/v1/skills**', async (route) => {
    if (route.request().method() !== 'GET') throw new Error('A layout walk must not change skills.');
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith('/file')) {
      await route.fulfill({ contentType: 'text/plain; charset=utf-8', body: '---\nname: pdf-tools\ndescription: Reads and fills PDF forms.\n---\n\n# PDF tools\n' });
      return;
    }
    await route.fulfill({ json: path.endsWith('/skills') ? [{ ...skill, content_hidden: false }] : path.endsWith('/versions') ? versions
      : path.includes('/versions/') ? files : skill });
  });
}
