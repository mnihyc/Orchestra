/** A confirmed security act retakes the current account without reusing a fresh or pending old profile. */
import { afterEach, expect, it, vi } from 'vitest';
import { meQuery } from '@/api/auth';
import { queryClient } from '@/api/query';
import { disableAuthenticator, disableEmailCode, enableEmailCode, unlinkIdentity } from '@/api/users';
import type { Me } from '@/generated/api-types';
import { me } from '../doubles/fixtures';
import type * as ClientModule from '@/api/client';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string) => Promise<unknown>>(), post: vi.fn<(path: string, options: unknown) => Promise<unknown>>(), remove: vi.fn<(path: string, options: unknown) => Promise<unknown>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get, POST: calls.post, DELETE: calls.remove } };
});
const account = me(1, 'ada', 'Ada Lovelace');
function answer(data: unknown) { return { data, response: new Response(JSON.stringify(data), { status: 200, headers: { 'Content-Type': 'application/json' } }) }; }
afterEach(() => { queryClient.clear(); calls.get.mockReset(); calls.post.mockReset(); calls.remove.mockReset(); });

const changes: { name: string; run: () => Promise<Me>; before: Partial<Me>; after: Partial<Me> }[] = [
  { name: 'disable the authenticator', run: () => disableAuthenticator('fixture password', '135790'), before: { totp_enabled: true }, after: { totp_enabled: false } },
  { name: 'enable mailed codes', run: () => enableEmailCode('fixture code'), before: { email_code_second_step: false }, after: { email_code_second_step: true } },
  { name: 'disable mailed codes', run: () => disableEmailCode('fixture password', 'fixture code'), before: { email_code_second_step: true }, after: { email_code_second_step: false } },
  { name: 'remove a linked identity', run: () => unlinkIdentity('example'), before: {}, after: { identities: [] } },
];
for (const change of changes) it(`retakes the account once after a successful act to ${change.name}`, async () => {
  queryClient.setQueryDefaults(meQuery.queryKey, { staleTime: Infinity });
  queryClient.setQueryData(meQuery.queryKey, { ...account, ...change.before });
  calls.post.mockResolvedValue({ response: new Response(null, { status: 204 }) });
  calls.remove.mockResolvedValue({ response: new Response(null, { status: 204 }) });
  const current = { ...account, ...change.after };
  calls.get.mockResolvedValue(answer(current));
  expect(await change.run()).toEqual(current);
  expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/me']);
  expect(queryClient.getQueryData(meQuery.queryKey)).toEqual(current);
});

it('retires an older profile read before publishing the account after the security act', async () => {
  let release!: (value: unknown) => void;
  const held = new Promise<unknown>((resolve) => { release = resolve; });
  calls.get.mockReturnValueOnce(held).mockResolvedValue(answer({ ...account, totp_enabled: false }));
  calls.post.mockResolvedValue({ response: new Response(null, { status: 204 }) });
  const earlier = queryClient.query({ ...meQuery, staleTime: 0 }).catch(() => undefined);
  expect((await disableAuthenticator('fixture password', '135790')).totp_enabled).toBe(false);
  release(answer({ ...account, totp_enabled: true }));
  await earlier;
  expect(queryClient.getQueryData(meQuery.queryKey)?.totp_enabled).toBe(false);
  expect(calls.get).toHaveBeenCalledTimes(2);
});
