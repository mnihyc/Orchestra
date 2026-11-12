/** Addresses, file names and native request builders retain the explicit whole-system scope. */
import { expect, it } from 'vitest';
import { figuresParams, offersThreads, parseScope, patchSearch, readSearch, recordsParams, sameScope, scopeText, tablesFor } from '@/features/usage/search';
import { usageFileName } from '@/features/usage/saveFile';
import { capsQuery, eventsQuery, usageQuery } from '@/api/usage';
const scope = { kind: 'system' } as const, span = { from: '2026-10-01', to: '2026-10-07', days: 7, range: 'custom' as const };
it('round-trips System independently from Me and rejects scoped lookalikes', () => {
  expect(parseScope('system')).toEqual(scope); expect(scopeText(scope)).toBe('system'); expect(sameScope(scope, { kind: 'me' })).toBe(false);
  expect(parseScope('system:someone')).toEqual({ kind: 'me' }); expect(parseScope('systems')).toEqual({ kind: 'me' });
});
it('keeps System through URL normalization and subsequent view/filter changes', () => {
  const search = readSearch({ scope: 'system', view: 'requests', range: '7d' }); expect(search.scope).toBe('system');
  expect(patchSearch(search, { view: 'performance', show: 'speed' }).scope).toBe('system'); expect(readSearch({ ...search }).scope).toBe('system');
});
it('uses the native system aggregate and record scope, retaining filters', () => {
  const search = { scope: 'system', org: 'org-id', view: 'requests' as const };
  expect(figuresParams(scope, search, span, ['org', 'person'], 10)).toMatchObject({ scope: 'system', filters: { org_id: 'org-id' }, breakdowns: ['org', 'person'] });
  expect(recordsParams(scope, search, span)).toMatchObject({ scope: 'system', filters: { org_id: 'org-id' }, requestsOnly: true, includeRunning: true });
});
it('offers system-level breakdowns and identifies exported files honestly', () => {
  expect(tablesFor(scope, null)).toEqual(expect.arrayContaining(['org', 'person', 'provider', 'topic'])); expect(offersThreads(scope, 'reports')).toBe(true);
  expect(usageFileName(scope, span, 'records')).toBe('orch-usage-system-2026-10-01-to-2026-10-07-records.csv');
});
it('declares current admin reach on every system data cache supplier', () => {
  const params = { scope: 'system', from: span.from, to: span.to, filters: {}, breakdowns: [] };
  for (const query of [usageQuery(params), eventsQuery(params), capsQuery({ kind: 'system' })]) expect(query.meta).toMatchObject({ requiresAdminMode: true, requiresSystemAdmin: true });
  expect(usageQuery({ ...params, scope: 'me' }).meta).toBeUndefined();
});
