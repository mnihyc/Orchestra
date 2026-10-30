/** A person's page and calendar, always projected through the current reader's own access. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { MeProfileUpdateBody, ProfileActivity, ProfilePinInput, UserProfile, UserSharePage } from '@/generated/api-types';

export const profileKey = (username: string) => ['profile', username] as const;
export const profileActivityKey = (username: string) => ['profile-activity', username] as const;

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
