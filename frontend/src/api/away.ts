/** Scoped notification status, pause, retained routes, device registrations and grouped read requests. */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import { INBOX_PAGE, notificationPrefsKey } from '@/api/notifications';
import type { AwayPage, GroupReadBody, PrefsChange, ReadResult, RouteChange } from '@/features/settings/personal/away/types';


export const outsideKey = ['away', 'summary'] as const;
export const routesKey = ['away', 'routes'] as const;
export const devicesKey = ['away', 'devices'] as const;
export const groupedKey = ['away', 'groups'] as const;

export const outsideQuery = (nodeId?: string) => queryOptions({
  queryKey: [...outsideKey, nodeId ?? null],
  queryFn: ({ signal }) => ok(api.GET('/notifications/summary', { params: { query: nodeId === undefined ? {} : { node_id: nodeId } }, signal })),
  retry: false,
});

export const awayPrefsQuery = queryOptions({
  queryKey: notificationPrefsKey,
  queryFn: ({ signal }) => ok(api.GET('/me/notification-prefs', { signal })),
  retry: false,
});

export const routesQuery = queryOptions({ queryKey: routesKey, queryFn: ({ signal }) => ok(api.GET('/me/notification-routes', { signal })), retry: false });
export const devicesQuery = queryOptions({ queryKey: devicesKey, queryFn: ({ signal }) => ok(api.GET('/me/push-subscriptions', { signal })), retry: false });

/** The revision is the one the person was shown, not one fetched behind the confirmation. */
export const changePrefs = (body: PrefsChange) => ok(api.PUT('/me/notification-prefs', { body }));
export const changeRoute = (channelId: string, body: RouteChange) => ok(api.PUT('/me/notification-routes/{channel_id}', { params: { path: { channel_id: channelId } }, body }));
export const removeRetainedRoute = (routeId: string) => done(api.DELETE('/me/notification-routes/by-id/{route_id}', { params: { path: { route_id: routeId } } }));
export const removeDevice = (id: string) => done(api.DELETE('/me/push-subscriptions/{id}', { params: { path: { id } } }));
export const testDevice = (channelId: string, deviceId?: string) => ok(api.POST('/notification-channels/{id}/test', { params: { path: { id: channelId } }, body: deviceId === undefined ? {} : { device_id: deviceId } }));

export function fetchPlacePage(filter: 'all' | 'unread' | 'mentions', cursor: string | null, signal?: AbortSignal): Promise<AwayPage> {
  return ok(api.GET('/notifications', { params: { query: { group_by: 'place', limit: INBOX_PAGE, ...(filter === 'unread' ? { unread: true } : {}), ...(filter === 'mentions' ? { kind: 'mention,doc.mention,person.notify' } : {}), ...(cursor === null ? {} : { cursor }) } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Retry reuses this exact key/body; only a new explicit group act makes a new batch. */
export function readGroup(body: GroupReadBody, key: string): Promise<ReadResult> {
  return ok(api.POST('/notifications/read', { body, ...repeatable(key) }));
}
