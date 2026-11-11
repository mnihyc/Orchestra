/** A person's page and calendar, always projected through the current reader's own access. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { Member, MemberPage, MeProfileUpdateBody, ProfileActivity, ProfilePinInput, UserProfile, UserRef, UserSharePage } from '@/generated/api-types';
import { PAGE_SIZE } from '@/api/nodes';

export const profileKey = (username: string) => ['profile', username] as const;
export const profileActivityKey = (username: string) => ['profile-activity', username] as const;

/** A visible membership's role comes from the directory, never from the profile's org name. */
export async function readProfileMembership(user: Pick<UserRef, 'id' | 'username'>, domain: string, signal: AbortSignal): Promise<Member | null> {
  if (user.username.trim() === '') return null;
  let cursor: string | null = null;
  const seen = new Set<string>();
  do {
    const page: MemberPage = await ok(api.GET('/orgs/{id}/members', {
      params: { path: { id: domain }, query: { q: user.username, direct: true, limit: PAGE_SIZE, ...(cursor === null ? {} : { cursor }) } }, signal,
    }));
    const member = page.items.find((row) => row.user.id === user.id && row.domain.id === domain);
    if (member !== undefined) return member;
    cursor = page.next_cursor;
    if (cursor !== null && seen.has(cursor)) return null;
    if (cursor !== null) seen.add(cursor);
  } while (cursor !== null);
  return null;
}

export const profileMembershipQuery = (user: Pick<UserRef, 'id' | 'username'>, domain: string) => queryOptions({
  queryKey: ['profile-membership', user.id, user.username, domain] as const,
  retry: false,
  queryFn: ({ signal }) => readProfileMembership(user, domain, signal),
});

/** The card shows a short page; more is requested only when the reader asks. */
const CARD_SHARES_PAGE = 8;
export const userSharesQuery = (id: string) => infiniteQueryOptions({
  queryKey: ['user-shares', id] as const,
  initialPageParam: undefined as string | undefined,
  queryFn: ({ signal, pageParam }): Promise<UserSharePage> => ok(api.GET('/users/{id}/shares', {
    params: { path: { id }, query: { limit: CARD_SHARES_PAGE, ...(pageParam === undefined ? {} : { cursor: pageParam }) } }, signal,
  })),
  getNextPageParam: (page) => page.next_cursor ?? undefined,
});

export const profileQuery = (username: string) => queryOptions({
  queryKey: profileKey(username),
  queryFn: ({ signal }): Promise<UserProfile> => ok(api.GET('/profiles/{username}', { params: { path: { username } }, signal })),
  retry: false,
});

export const profileActivityQuery = (username: string) => queryOptions({
  queryKey: profileActivityKey(username),
  queryFn: ({ signal }): Promise<ProfileActivity> => ok(api.GET('/profiles/{username}/activity', { params: { path: { username } }, signal })),
  retry: false,
});

export function updateProfile(body: MeProfileUpdateBody): Promise<UserProfile> {
  return ok(api.PATCH('/me/profile', { body }));
}

export function replaceProfilePins(pins: ProfilePinInput[]): Promise<UserProfile> {
  return ok(api.PUT('/me/profile/pins', { body: { pins } }));
}
