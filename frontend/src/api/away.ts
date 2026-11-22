/** Scoped notification status, pause, retained routes, device registrations and grouped read requests. */
import { queryOptions } from '@tanstack/react-query';

import { collectionQuery, readAllPages } from '@/api/collection';
import { suppliedRead } from '@/api/readScheduler';
import type { OutsideStatus } from '@/generated/api-types';
import { api, done, ok, repeatable } from '@/api/client';
import { INBOX_PAGE, notificationPrefsKey } from '@/api/notifications';
import type { AwayPage, GroupReadBody, PrefsChange, ReadResult, RouteChange } from '@/features/settings/personal/away/types';


export const outsideKey = ['away', 'summary'] as const;
export const outsideStatusKey = ['away', 'outside-status'] as const;
export const routesKey = ['away', 'routes'] as const;
export const devicesKey = ['away', 'devices'] as const;
export const groupedKey = ['away', 'groups'] as const;

export const outsideQuery = (nodeId?: string) => queryOptions({
  queryKey: [...outsideKey, nodeId ?? null],
  queryFn: ({ signal }) => ok(api.GET('/notifications/summary', { params: { query: nodeId === undefined ? {} : { node_id: nodeId } }, signal })),
  retry: false,
});

/** Scoped consumers need the same status the setup producer already observed, not another counter read. */
export const outsideStatusQuery = (nodeId: string) => queryOptions({
  queryKey: [...outsideStatusKey, nodeId] as const,
  ...suppliedRead([...outsideStatusKey, nodeId]),
  queryFn: async ({ signal }): Promise<OutsideStatus> => (await ok(api.GET('/notifications/summary', { params: { query: { node_id: nodeId } }, signal }))).outside,
  retry: false,
});

export const awayPrefsQuery = queryOptions({
  queryKey: notificationPrefsKey,
  queryFn: ({ signal }) => ok(api.GET('/me/notification-prefs', { signal })),
  retry: false,
});

export const routesQuery = queryOptions({ queryKey: routesKey, queryFn: ({ signal }) => readAllPages((cursor) => ok(api.GET('/me/notification-routes', { params: { query: cursor === null ? {} : { cursor } }, signal })), signal), retry: false });
export const routesPageQuery = (q: string) => collectionQuery([...routesKey, 'retained'], q, (query, signal) => ok(api.GET('/me/notification-routes', { params: { query: { ...query, retained: true } }, signal })));
export const devicesQuery = queryOptions({ queryKey: devicesKey, queryFn: ({ signal }) => readAllPages((cursor) => ok(api.GET('/me/push-subscriptions', { params: { query: cursor === null ? {} : { cursor } }, signal })), signal), retry: false });
export const devicesPageQuery = (q: string) => collectionQuery(devicesKey, q, (query, signal) => ok(api.GET('/me/push-subscriptions', { params: { query }, signal })));

/** The revision is the one the person was shown, not one fetched behind the confirmation. */
export const changePrefs = (body: PrefsChange) => ok(api.PUT('/me/notification-prefs', { body }));
export const changeRoute = (channelId: string, body: RouteChange) => ok(api.PUT('/me/notification-routes/{channel_id}', { params: { path: { channel_id: channelId } }, body }));
export const removeRetainedRoute = (routeId: string) => done(api.DELETE('/me/notification-routes/by-id/{route_id}', { params: { path: { route_id: routeId } } }));
export const removeDevice = (id: string) => done(api.DELETE('/me/push-subscriptions/{id}', { params: { path: { id } } }));
export const testDevice = (channelId: string, deviceId?: string) => ok(api.POST('/notification-channels/{id}/test', { params: { path: { id: channelId } }, body: deviceId === undefined ? {} : { device_id: deviceId } }));

export function fetchPlacePage(filter: 'all' | 'unread' | 'mentions', cursor: string | null, signal?: AbortSignal, q=''): Promise<AwayPage> {
  return ok(api.GET('/notifications', { params: { query: { group_by: 'place', limit: INBOX_PAGE, ...(q===''?{}:{q}), ...(filter === 'unread' ? { unread: true } : {}), ...(filter === 'mentions' ? { kind: 'mention,doc.mention,person.notify' } : {}), ...(cursor === null ? {} : { cursor }) } }, ...(signal === undefined ? {} : { signal }) }));
}

/** Retry reuses this exact key/body; only a new explicit group act makes a new batch. */
export function readGroup(body: GroupReadBody, key: string): Promise<ReadResult> {
  return ok(api.POST('/notifications/read', { body, ...repeatable(key) }));
}
