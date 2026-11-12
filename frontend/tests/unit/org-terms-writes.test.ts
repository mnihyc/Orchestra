/** The existing accept operation and native OIDC entrance carry the exact reviewed hash only when joining. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import type { HttpTransport } from '../doubles/http-wire';
import { acceptInvitation } from '@/api/users';
import { identityStart } from '@/features/auth/IdentityProviders';
const calls = vi.hoisted(() => ({ post: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (original) => ({ ...await original<typeof Client>(), api: { POST: calls.post } }));
beforeEach(() => { vi.clearAllMocks(); calls.post.mockResolvedValue({ data: { org: null, node: null, chat: null }, response: new Response(null, { status: 200 }) }); });
it('adds the hash to the existing invitation accept body', async () => {
  const hash = 'a'.repeat(64); await acceptInvitation('invitation', hash);
  expect(calls.post).toHaveBeenCalledWith('/invitations/accept', { body: { token: 'invitation', accepted_terms_hash: hash } });
});
it('leaves a no-terms accept body unchanged', async () => {
  await acceptInvitation('invitation');
  expect(calls.post).toHaveBeenCalledWith('/invitations/accept', { body: { token: 'invitation' } });
});
it('carries the confirmed hash on OIDC join and leaves ordinary sign-in independent', () => {
  const hash = 'b'.repeat(64);
  const join = new URL(identityStart('company', '/', 'invite', true, hash), 'https://orch.invalid');
  expect(join.searchParams.get('accepted_terms_hash')).toBe(hash);
  expect(join.searchParams.get('accept_invitation')).toBe('true');
  expect(join.searchParams.get('invitation')).toBe('invite');
  const signIn = new URL(identityStart('company', '/', null, false, hash), 'https://orch.invalid');
  expect(signIn.searchParams.has('accepted_terms_hash')).toBe(false);
});
