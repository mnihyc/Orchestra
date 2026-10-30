// @vitest-environment node
/** Relevance is address state, and a name result keeps its own route rather than pretending to be a message. */
import { expect, it } from 'vitest';
import { searchKey } from '@/api/search';
import { authorOfHit, atOfHit, crumbsOf, destinationOfHit, keyOfHit } from '@/features/search/hits';
import { targetOfHit } from '@/features/search/ResultView';
import { filtersOf, mergeParams, readParams, restoredView, savedView, tabOf, withKind, withTab } from '@/features/search/params';
import { rememberedFilterNames } from '@/features/search/filterNames';
import type { SearchHit } from '@/generated/api-types';

it('keeps relevance across reloads and a delayed query write, with an independent cached result', () => {
  const current = readParams({ q: 'release', sort: 'relevance' });
  expect(current).toEqual({ q: 'release', sort: 'relevance' });
  expect(filtersOf(current).sort).toBe('relevance');
  expect(searchKey(filtersOf(current))).not.toEqual(searchKey({ q: 'release' }));
  expect(mergeParams({ ...current }, { q: 'release' }, { q: 'release policy' })).toEqual({ q: 'release policy', sort: 'relevance' });
  expect(readParams({ q: 'release', sort: 'unknown' })).toEqual({ q: 'release' });
  const document = '00000000-0000-4000-8000-000000000001';
  expect(mergeParams({q: 'release', document_id: document, regex: 'true', sort: 'newest'}, {q: 'release'}, {q: 'release policy'}))
    .toEqual({q: 'release policy', document_id: document, regex: 'true', sort: 'newest'});
});

it('keeps advanced selections in links and on this device without saving typed words', () => {
  const owner = '00000000-0000-4000-8000-000000000001';
  const editor = '00000000-0000-4000-8000-000000000002';
  const current = readParams({ q: 'release', created_by: owner, edited_by: editor, owner, date_field: 'edited',
    from: '2026-10-01', title_only: 'true', shared_with_me: true, file_type: 'image/*' });
  expect(filtersOf(current)).toMatchObject({ q: 'release', created_by: owner, edited_by: editor, owner,
    date_field: 'edited', from: '2026-10-01', title_only: true, shared_with_me: true, file_type: 'image/*' });
  expect(savedView(current)).not.toContain('release');
  expect(restoredView({ q: 'another' }, savedView(current))).toEqual({ ...current, q: 'another' });
  expect(restoredView({ q: 'another', owner: editor }, savedView(current))).toBeUndefined();
  expect(mergeParams({ ...current }, { q: 'release' }, { q: 'canary' })).toEqual({ ...current, q: 'canary' });
  expect(searchKey(filtersOf(current))).not.toEqual(searchKey({ q: 'release' }));
  expect(readParams({ owner: 'not-an-id', date_field: 'activity', file_type: 'image/**', title_only: 'no' })).toEqual({});
});

it('keeps the selected result category in the address and asks for its matching kinds', () => {
  const old = readParams({ q: 'release', kinds: 'document' });
  expect(tabOf(old)).toBe('documents');
  expect(filtersOf(withTab(old, 'all')).kinds).toBeUndefined();
  expect(filtersOf(withTab(old, 'chats')).kinds).toEqual(['chat_message']);
  expect(filtersOf(withTab(old, 'documents')).kinds).toEqual(['document']);
  expect(filtersOf(withTab(old, 'conversations')).kinds).toContain('user_message');
  expect(filtersOf(withTab(old, 'files')).with_file).toBe(true);
  const none = readParams({ q: 'release', tab: 'conversations', kinds: 'chat_message,document' });
  expect(filtersOf(none).kinds).toEqual([]);
  expect(searchKey(filtersOf(none))).not.toEqual(searchKey({ q: 'release' }));
  expect(filtersOf(withTab(withTab(none, 'files'), 'conversations')).kinds).toEqual([]);
  expect(filtersOf(readParams({ q: 'release', kinds: '' })).kinds).toEqual([]);
  expect(withKind({ kinds: '' }, 'user_message', true)).toBe('user_message');
  expect(readParams({ tab: 'unknown' })).toEqual({});
  expect(restoredView({ q: 'other' }, savedView(withTab(old, 'files')))?.tab).toBe('files');
});

it('keeps an explicit empty selection when the last legacy kind is turned off', () => {
  const old = readParams({ q: 'release', kinds: 'user_message' });
  const kinds = withKind(old, 'user_message', false);
  expect(kinds).toBe('');
  const none = readParams({ ...old, tab: tabOf(old), kinds });
  expect(filtersOf(none).kinds).toEqual([]);
  expect(filtersOf(withTab(withTab(none, 'files'), 'conversations')).kinds).toEqual([]);
});

it('bounds remembered labels while a hundred thousand choices pass through the filters', () => {
  const params = {scope:'scope', document_id:'document', chat_id:'chat', owner:'owner', created_by:'creator', edited_by:'editor', author:'author'};
  let names = Object.fromEntries(Object.values(params).map((id) => [id, id]));
  let largest = 0;
  for (let i = 0; i < 100_000; i += 1) {
    names = rememberedFilterNames(names, params, {id:`choice-${i}`, name:`Person ${i}`});
    largest = Math.max(largest, Object.keys(names).length);
  }
  expect(largest).toBe(8);
  expect(names).toEqual({...Object.fromEntries(Object.values(params).map((id) => [id, id])), 'choice-99999':'Person 99999'});
});

it('opens a named place at its served address, names no author and offers no false conversation beside it', () => {
  const node = { id: 'topic', kind: 'topic', name: 'Release policy', redacted: false } as const;
  const hit: SearchHit = { kind: 'name', node, path: [{ id: 'org', kind: 'org', name: 'Work', redacted: false }, node],
    href: '/topic/topic', at: '2026-10-15T10:00:00Z', snippet: 'Release policy', matches: [[0, 7]] };
  expect(destinationOfHit(hit, () => 'a-different-org')).toEqual({ to: '/topic/topic' });
  expect(crumbsOf(hit, { chat: 'Chat', private: () => 'Private' })).toEqual(['Work', 'Release policy']);
  expect(keyOfHit(hit)).toBe('name:topic');
  expect(authorOfHit(hit)).toBeNull();
  expect(atOfHit(hit)).toBe(hit.at);
  expect(targetOfHit(hit)).toBeNull();
});
