/** The root choice is address state, separate from a narrower place and device defaults. */
import { expect, it } from 'vitest';
import { filtersOf, inChat, inOrg, mergeParams, readParams, restoredView, savedView } from '@/features/search/params';
import { groupHitIds, hitRoot } from '@/features/search/orgHits';
import type { NodeRef, SearchHit } from '@/generated/api-types';

const org = '00000000-0000-4000-8000-000000000001';
const personal = '00000000-0000-4000-8000-000000000002';
const topic = '00000000-0000-4000-8000-000000000003';
const root = (id: string, name: string): NodeRef => ({ id, kind: 'org', name, redacted: false });
it('round-trips one root and the deliberate all-org choice, refusing arbitrary identifiers', () => {
  expect(readParams(Object.fromEntries(new URLSearchParams(`q=canary&org=${org}`)))).toEqual({ q: 'canary', org });
  expect(readParams({ q: 'canary', org: 'all' })).toEqual({ q: 'canary', org: 'all' });
  expect(readParams({ org: 'everything' })).toEqual({});
  expect(filtersOf({ q: 'canary', org }).scope).toBe(org);
  expect(filtersOf({ q: 'canary', org: 'all' }).scope).toBeUndefined();
  expect(filtersOf({ q: 'canary', org, scope: topic }).scope).toBe(topic);
  expect(filtersOf({ q: 'canary', org, document_id: 'doc' })).toMatchObject({ scope: org, document_id: 'doc' });
  expect(inChat({ q: 'canary', org }, 'chat', topic, personal)).toMatchObject({ org: personal, scope: topic, chat_id: 'chat' });
});
it('changes roots without retaining an unrelated place and preserves a newer org during delayed typing', () => {
  expect(inOrg({ q: 'canary', org, scope: topic, chat_id: topic, document_id: topic, sort: 'relevance' }, 'all'))
    .toEqual({ q: 'canary', org: 'all', sort: 'relevance' });
  expect(mergeParams({ q: 'canary', org: 'all' }, { q: 'canary', org }, { q: 'canary song', org }))
    .toEqual({ q: 'canary song', org: 'all' });
});
it('does not remember All my orgs as the next default, while explicit links win over saved filters', () => {
  const kept = savedView({ org: 'all', q: 'private words', sort: 'relevance', with_file: 'true' });
  expect(JSON.parse(kept)).toEqual({ sort: 'relevance', with_file: 'true' });
  expect(restoredView({ q: 'new' }, JSON.stringify({ org: 'all', sort: 'relevance' }))).toEqual({ q: 'new', sort: 'relevance' });
  expect(restoredView({ q: 'new', org }, kept)).toBeUndefined();
});
it('groups only multiple answering roots and keeps native order within each, including unnamed roots', () => {
  const a = root(org, 'Work'), b = root(personal, 'Personal');
  const roots = new Map([['a', a], ['b', b], ['a2', a], ['unknown', null]]);
  expect(groupHitIds(['a', 'a2'], roots)).toEqual([{ root: null, ids: ['a', 'a2'] }]);
  expect(groupHitIds(['a', 'b', 'a2', 'unknown'], roots)).toEqual([
    { root: a, ids: ['a', 'a2'] }, { root: b, ids: ['b'] }, { root: null, ids: ['unknown'] },
  ]);
});
it('never guesses a root from a project name or leaks a redacted path through the org list', () => {
  const hit: SearchHit = { kind: 'name', node: { id: topic, kind: 'topic', name: 'Canary', redacted: false }, path: [], href: `/topic/${topic}`, at: '2026-01-01T00:00:00Z', snippet: 'Canary', matches: [] };
  expect(hitRoot(hit, [])).toBeNull();
  const hidden = { ...root(org, 'Hidden'), name: null, redacted: true };
  expect(hitRoot({ ...hit, path: [hidden] }, [])).toEqual(hidden);
  expect(hitRoot(hit, [], new Map([[topic, { org_id: org, path: [hidden] }]]))).toEqual(hidden);
});
