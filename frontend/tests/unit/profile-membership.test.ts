/** A directory search must match the person and domain, including after the first page. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import type { HttpTransport } from '../doubles/http-wire';
import { readProfileMembership } from '@/api/profile';
import type { Member, MemberPage } from '@/generated/api-types';
import { ADA, BEA, TOPIC } from './general-access-fixtures';

const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: calls.get } };
});
const row: Member = { terms_hash: null, terms_accepted_at: null, user: ADA, domain: { ...TOPIC, kind: 'org' }, role: 'viewer', is_owner: false,
  address: '@ada', also: 0, suspended: false, since: null, last_seen_at: null, second_step: null, second_step_required: null };
const reply = (page: MemberPage) => ({ data: page, response: new Response(null, { status: 200 }) });
beforeEach(() => vi.clearAllMocks());

it('does not substitute another person or another domain and follows the supplied cursor', async () => {
  calls.get.mockResolvedValueOnce(reply({ items: [{ ...row, user: BEA }, { ...row, domain: { ...row.domain, id: 'other-org' } }], next_cursor: 'next' }))
    .mockResolvedValueOnce(reply({ items: [row], next_cursor: null }));
  const signal = new AbortController().signal;
  expect(await readProfileMembership(ADA, row.domain.id, signal)).toEqual(row);
  expect(calls.get.mock.calls[0]?.[1]).toMatchObject({ params: { path: { id: row.domain.id }, query: { q: ADA.username, direct: true } }, signal });
  expect(calls.get.mock.calls[1]?.[1]).toMatchObject({ params: { query: { cursor: 'next' } }, signal });
});

it('has no fabricated role when the search is complete', async () => {
  calls.get.mockResolvedValue(reply({ items: [], next_cursor: null }));
  expect(await readProfileMembership(ADA, row.domain.id, new AbortController().signal)).toBeNull();
  expect(calls.get).toHaveBeenCalledOnce();
});

it('stops a repeated cursor rather than starting an unbounded request loop', async () => {
  calls.get.mockResolvedValue(reply({ items: [], next_cursor: 'same' }));
  expect(await readProfileMembership(ADA, row.domain.id, new AbortController().signal)).toBeNull();
  expect(calls.get).toHaveBeenCalledTimes(2);
});

it('never turns a missing username into an unfiltered directory scan', async () => {
  expect(await readProfileMembership({ id: ADA.id, username: '' }, row.domain.id, new AbortController().signal)).toBeNull();
  expect(calls.get).not.toHaveBeenCalled();
});
