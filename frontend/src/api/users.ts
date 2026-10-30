/** API client for Users and invitations in both forms.
 *
 * What an invitation shows to whoever holds its link, accepting one, and for those who manage an
 * org's members: making one in either form, the invitations an org has, and taking one back. And
 * what a person does to their own sign-in: the password, the second step by an authenticator app and
 * by a mailed code, the sessions that are open, linked identities, and API tokens.
 */
import { queryOptions } from '@tanstack/react-query';

import { meQuery } from '@/api/auth';
import { api, done, ok, repeatable } from '@/api/client';
import { queryClient } from '@/api/query';
import type {
  ApiTokenCreateBody,
  TokenCreated,
  ApiTokenInfo,
  Invitation,
  InvitationAcceptReply,
  InvitationCreateBody,
  InvitationCreateReply,
  InvitationPreview,
  Me,
  MeTotpSetupReply,
  MeTotpEnableReply,
  RecoveryCodeBatch,
  RecoveryCodeStatus,
  SensitiveProof,
  SessionInfo,
  UserRef,
} from '@/generated/api-types';

export const invitationKey = (token: string) => ['invitation', token] as const;

/** What an invitation is for, who sent it and until when it is good. Anybody who holds the link may ask. */
export const invitationQuery = (token: string) =>
  queryOptions({
    queryKey: invitationKey(token),
    queryFn: ({ signal }): Promise<InvitationPreview> =>
      ok(api.GET('/invitations/lookup/{token}', { params: { path: { token } }, signal })),
    // An invitation that is no longer good stays so: asking again changes nothing.
    retry: false,
  });

/** Takes what an invitation carries, for the person signed in. */
export function acceptInvitation(token: string): Promise<InvitationAcceptReply> {
  return ok(api.POST('/invitations/accept', { body: { token } }));
}

export const invitationsKey = (orgId: string) => ['invitations', orgId] as const;

/** The invitations into an org, newest first. */
export const invitationsQuery = (orgId: string) =>
  queryOptions({
    queryKey: invitationsKey(orgId),
    queryFn: ({ signal }): Promise<Invitation[]> =>
      ok(api.GET('/invitations', { params: { query: { org_id: orgId } }, signal })),
  });

/**
 * Makes an invitation. With an address it is a personal link, for that address alone; without one,
 * a shared link. The answer holds the link, which is shown once.
 */
export function createInvitation(body: InvitationCreateBody, key: string): Promise<InvitationCreateReply> {
  return ok(api.POST('/invitations', { body, ...repeatable(key) }));
}

/** Takes an invitation back: its link stops being good. */
export function revokeInvitation(id: string): Promise<void> {
  return done(api.DELETE('/invitations/{id}', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- a person's own sign-in

/** A bodyless security act needs the server's current account facts, even when the old profile is fresh. */
async function refreshMe(): Promise<Me> {
  await queryClient.cancelQueries({ queryKey: meQuery.queryKey, exact: true });
  return queryClient.query({ ...meQuery, staleTime: 0 });
}

/** Changes the password; every other session of the person ends with it. */
export function changePassword(currentPassword: string, newPassword: string): Promise<void> {
  return done(api.POST('/me/password', { body: { current_password: currentPassword, new_password: newPassword } }));
}

/**
 * What setting a second step up is proved with: the password, or for a person who has none the
 * one-use proof their identity provider gave for it.
 */
export type SetupProof = { password: string } | { identity_proof: string };

/** Begins an authenticator app: the secret to scan or type. Asked again before it is switched on, it gives the same secret. */
export function setUpAuthenticator(proof: SetupProof): Promise<MeTotpSetupReply> {
  return ok(api.POST('/me/totp/setup', { body: proof }));
}

/**
 * Gives this browser's second step, which the orgs that require one wait for: a code of the
 * authenticator, or one that was mailed. The person is read again, so every screen agrees.
 */
export function verifySecondStep(code: { totp: string } | { email_code: string } | { recovery_code: string }): Promise<Me> {
  return ok(api.POST('/me/second-step/verify', { body: code }));
}

/** Switches the authenticator app on with a code it shows. */
export function enableAuthenticator(code: string): Promise<MeTotpEnableReply> {
  return ok(api.POST('/me/totp/enable', { body: { code } }));
}

/** Loaded only when recovery management is opened; it never reads plaintext codes. */
export const recoveryCodesQuery = queryOptions({
  queryKey: ['recovery-codes'],
  queryFn: ({ signal }): Promise<RecoveryCodeStatus> => ok(api.GET('/me/recovery-codes', { signal })),
  staleTime: 0,
});

/** Replaces every older saved code; the caller shows this batch once, outside persistent caches. */
export function regenerateRecoveryCodes(proof: SensitiveProof): Promise<RecoveryCodeBatch> {
  return ok(api.POST('/me/recovery-codes', { body: { proof } }));
}

/** Switches the authenticator app off; the password and a code of the app are asked. */
export async function disableAuthenticator(password: string, code: string): Promise<Me> {
  await done(api.POST('/me/totp/disable', { body: { password, code } }));
  return refreshMe();
}

/**
 * Mails a code to the confirmed address: to switch it on (with the password, or the identity
 * provider's proof for a person without one) or off; with nothing, for this browser to give a
 * mailed code that is the person's second step already.
 */
export function sendEmailCode(proof: SetupProof | Record<string, never>): Promise<void> {
  return done(api.POST('/me/email-code/send', { body: proof }));
}

/** Switches the mailed code on as a second step, with the code that was mailed. */
export async function enableEmailCode(code: string): Promise<Me> {
  await done(api.POST('/me/email-code/enable', { body: { code } }));
  return refreshMe();
}

/** Switches the mailed code off; the password is asked, and a code that was mailed or one of the authenticator app. */
export async function disableEmailCode(password: string, code: string): Promise<Me> {
  await done(api.POST('/me/email-code/disable', { body: { password, code } }));
  return refreshMe();
}

/** Starts linking a sign-in identity: the address of its provider, which the page goes to. */
export async function linkIdentity(key: string): Promise<string> {
  const reply = await ok(api.POST('/me/identities/{key}/link', { params: { path: { key } } }));
  return reply.redirect_url;
}

/** Takes a linked identity away; refused when it would leave no way to sign in. */
export async function unlinkIdentity(key: string): Promise<Me> {
  await done(api.DELETE('/me/identities/{key}', { params: { path: { key } } }));
  return refreshMe();
}

export const sessionsKey = ['sessions'] as const;

/** The sessions open for the person, this one marked. */
export const sessionsQuery = queryOptions({
  queryKey: sessionsKey,
  queryFn: ({ signal }): Promise<SessionInfo[]> => ok(api.GET('/me/sessions', { signal })),
});

/** Ends one of the person's sessions; its browser is signed out at its next request. */
export function revokeSession(id: string): Promise<void> {
  return done(api.DELETE('/me/sessions/{id}', { params: { path: { id } } }));
}

/** Signs out every other browser; this session and personal API tokens remain usable. */
export function revokeOtherSessions(): Promise<SessionInfo[]> {
  return ok(api.DELETE('/me/sessions'));
}

export const tokensKey = ['tokens'] as const;

/** The API tokens the person has made, with what each may do. Their secrets were shown once and are not kept. */
export const tokensQuery = queryOptions({
  queryKey: tokensKey,
  queryFn: ({ signal }): Promise<ApiTokenInfo[]> => ok(api.GET('/me/tokens', { signal })),
});

/** Makes an API token. The answer holds its secret, which is never given again. */
export function createToken(body: ApiTokenCreateBody, key: string): Promise<TokenCreated> {
  return ok(api.POST('/me/tokens', { body, ...repeatable(key) }));
}

/** Ends a token for good. */
export function revokeToken(id: string): Promise<void> {
  return done(api.DELETE('/me/tokens/{id}', { params: { path: { id } } }));
}

// ---------------------------------------------------------------- finding people

/** The people whose names begin with, or hold, what was typed; among an org's members when `orgId` is given. */
export function searchUsers(q: string, orgId?: string, signal?: AbortSignal): Promise<UserRef[]> {
  return ok(
    api.GET('/users/search', {
      params: { query: { q, ...(orgId === undefined ? {} : { org_id: orgId }) } },
      ...(signal ? { signal } : {}),
    }),
  );
}
