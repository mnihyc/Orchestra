/** API client for Meta, registration, signing in by each proof, the second step and recovery.
 *
 * The requests a person makes before they are signed in, and the two that concern themself once they
 * are. An answer that signs somebody in is taken as the session of this page here, so that no screen
 * can forget to.
 */
import { QueryObserver, queryOptions } from '@tanstack/react-query';

import { api, client, done, ok } from '@/api/client';
import { queryClient } from '@/api/query';
import type {
  AuthEmailCodeSendBody,
  AuthLoginBody,
  AuthNameAvailabilityGetReply,
  AuthRecoveryConfirmBody,
  AuthRegisterBody,
  AuthResult,
  InvitationAcceptReply,
  Me,
  MeAuthorityReply,
  MeUpdateBody,
  Meta,
  SetupCreateBody,
} from '@/generated/api-types';

/**
 * How long what the system says of itself counts as fresh. It changes when an administrator changes
 * it, which is seldom, and a page that is open for a day should still learn of a new banner.
 */
const META_FRESH_FOR_MS = 5 * 60_000;

/** The fewest characters a password has. The server holds the rule; this is so the form can say it first. */
export const PASSWORD_MIN_CHARS = 10;

/** What a username looks like: 3 to 32 of lower-case letters, digits, hyphen and underscore, beginning with a letter or a digit. */
export const USERNAME_PATTERN = /^[a-z0-9][a-z0-9_-]{2,31}$/;

export const metaKey = ['meta'] as const;
export const meKey = ['me'] as const;

/** What the system says of itself to anybody: its name, who may register, the ways of signing in. */
export const metaQuery = queryOptions({
  queryKey: metaKey,
  queryFn: async ({ signal }): Promise<Meta> => {
    const reader = client.session.getState().readerVersion;
    const meta = await ok(api.GET('/meta', { signal }));
    // All other Meta facts are public; this permission belongs to the caller.
    return client.session.getState().readerVersion === reader ? meta : { ...meta, may_create_org: false };
  },
  staleTime: META_FRESH_FOR_MS,
});

/** A newly signed-in caller needs a fresh permission fact after any previous reader's flight. */
export async function refreshMetaForSignIn(): Promise<void> {
  const reader = client.session.getState();
  const current = () => {
    const next = client.session.getState();
    return next.status === 'signed_in' && next.userId === reader.userId && next.readerVersion === reader.readerVersion;
  };
  if (!current()) return;
  const held = new QueryObserver(queryClient, { ...metaQuery, enabled: false, staleTime: Infinity });
  const release = held.subscribe(() => undefined);
  const hidePreviousPermission = () => {
    if (!current()) return;
    const query = queryClient.getQueryCache().find<Meta>({ queryKey: metaKey, exact: true });
    const meta = query?.state.data;
    // Keep native error/fetch state. A canceled cached refetch can revert an older caller's flag.
    query?.setState({ ...(meta === undefined ? {} : { data: { ...meta, may_create_org: false } }), isInvalidated: true });
  };
  hidePreviousPermission();
  try {
    const previous = queryClient.getQueryCache().find({ queryKey: metaKey, exact: true });
    // Invalidating before this settles would merely join the anonymous/previous caller's request.
    if (previous !== undefined && previous.state.fetchStatus !== 'idle') await previous.promise;
    if (!current()) return;
    // LiveQueryClient refuses broad invalidation. Join any current flight, with no cancel/refetch pair.
    const refreshed = queryClient.invalidateQueries({ queryKey: metaKey, exact: true }, { cancelRefetch: false });
    const next = queryClient.getQueryCache().find({ queryKey: metaKey, exact: true });
    // The raw flight rejects explicit cancellation even when a cached refetch returns reverted data.
    await Promise.all([refreshed, next?.state.fetchStatus === 'idle' ? undefined : next?.promise]);
  } catch {
    // Its owner still receives the error. Revoke a reverted permission; never retry cancellation.
    hidePreviousPermission();
  } finally { release(); }
}

/** The person signed in. Every sign-in and every renewal writes it, so it is seldom asked for. */
export const meQuery = queryOptions({
  queryKey: meKey,
  queryFn: ({ signal }): Promise<Me> => ok(api.GET('/me', { signal })),
  staleTime: META_FRESH_FOR_MS,
});

/** The same native session projections, including caller-specific Meta, in one read. */
export function fetchShellAuthority(signal?: AbortSignal): Promise<MeAuthorityReply> {
  return ok(api.GET('/me/authority', { signal }));
}

/** Checks the public username/root-org namespace only. The server still decides at registration. */
export const nameAvailabilityQuery = (name: string) => queryOptions({
  queryKey: ['name-availability',name] as const,
  queryFn: ({signal}): Promise<AuthNameAvailabilityGetReply> => ok(api.GET('/auth/name-availability',{params:{query:{name}},signal})),
  retry: false,
});

function adopt(result: AuthResult): AuthResult {
  client.adopt(result);
  return result;
}

/** Makes the first administrator. From then on the system needs no setup. */
export async function setUp(body: SetupCreateBody): Promise<AuthResult> {
  const result = adopt(await ok(api.POST('/setup', { body })));
  queryClient.setQueryData(metaKey, (meta: Meta | undefined) => meta && { ...meta, needs_setup: false });
  return result;
}

/** Signs in with one proof, or with two for somebody who has a second step. */
export async function signIn(body: AuthLoginBody): Promise<AuthResult> {
  return adopt(await ok(api.POST('/auth/login', { body })));
}

/**
 * Asks for a code by mail: with the name alone as a way in, with the password as a second step. The
 * answer is the same whatever the name, so nothing is learnt from it.
 */
export function askEmailCode(body: AuthEmailCodeSendBody): Promise<void> {
  return done(api.POST('/auth/email-code', { body }));
}

/** How a registration ended: signed in, or waiting for the address to be confirmed. */
export type Registered = { signedIn: true; result: AuthResult; accepted: InvitationAcceptReply | null } | { signedIn: false };

/** Registers a person. Where addresses are confirmed first, nobody is signed in yet. */
export async function register(body: AuthRegisterBody): Promise<Registered> {
  const reply = await ok(api.POST('/auth/register', { body }));
  if ('access_token' in reply) return { signedIn: true, result: adopt(reply), accepted: reply.accepted_invitation ?? null };
  return { signedIn: false };
}

/** Asks for a link to set a new password. The answer is the same whatever the address. */
export function askRecovery(email: string): Promise<void> {
  return done(api.POST('/auth/recovery', { body: { email } }));
}

/** Checks a reset link without consuming it; discard its cached result when the card closes. */
export const recoveryQuery = (token: string) => queryOptions({
  queryKey: ['recovery-token', token],
  queryFn: async ({ signal }): Promise<boolean> => {
    await done(api.GET('/auth/recovery/{token}', { params: { path: { token } }, signal }));
    return true;
  },
  retry: false,
  staleTime: 0,
  gcTime: 0,
});

/** Sets a new password with the token of a link. Every session of that person ends. */
export function confirmRecovery(body: AuthRecoveryConfirmBody): Promise<void> {
  return done(api.POST('/auth/recovery/confirm', { body }));
}

/**
 * Confirms an address with the token of a link. Where the link also signs its person in, the browser
 * holds a session afterwards and the page takes it up; the answer says whether it did.
 */
export async function confirmEmail(token: string): Promise<boolean> {
  await done(api.POST('/auth/verify-email', { body: { token } }));
  if (client.session.getState().status === 'signed_in') return true;
  // Whether a session came with the answer is not said in it; asking for a token finds out.
  return client.renew().catch(() => false);
}

/** Asks for the confirmation link to be mailed again. The answer is the same whatever the name. */
export function resendConfirmation(login: string): Promise<void> {
  return done(api.POST('/auth/verify-email/resend', { body: { login } }));
}

/** Ends this browser's session. */
export function signOut(): Promise<void> {
  return client.signOut();
}

/** Changes what the person signed in set for themself. */
export async function updateMe(body: MeUpdateBody): Promise<Me> {
  const reader = client.session.getState();
  const me = await ok(api.PATCH('/me', { body }));
  const current = client.session.getState();
  if (current.userId === reader.userId && current.readerVersion === reader.readerVersion) queryClient.setQueryData(meKey, me);
  return me;
}
