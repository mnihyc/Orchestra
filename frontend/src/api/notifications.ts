/** API client for Inbox, preferences, channels with their texts, routes and destinations.
 *
 * What a person is told and how: the rows of their Inbox and the counters over them, what they chose
 * about being notified, the ways out of the application that can reach them (each with their own
 * switch, which is off until they turn it on), a bot's link, a browser that takes push, and a
 * webhook of their own. The Inbox itself is kept current by the person's live feed; these are the
 * requests that load it, change it and answer for it.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, done, ok, repeatable } from '@/api/client';
import type {
  MeNotificationChannelCreateBody,
  MeNotificationPrefsSetBody,
  MeNotificationRouteSetBody,
  MePushSubscriptionCreateBody,
  NotificationChannel,
  NotificationChannelUpdateBody,
  NotificationPage,
  NotificationPrefs,
  NotificationReadBody,
  NotificationReadReply,
  NotificationRoute,
  NotificationRouteLink,
  NotificationSummaryGetReply,
  NotificationTestReply,
  Notifier,
} from '@/generated/api-types';

/** How many rows one page of the Inbox holds: a screenful and a little over, so that the first look needs one request. */
export const INBOX_PAGE = 50;

/** The counters of the Inbox with the sequence they are as of. */
export function fetchSummary(signal?: AbortSignal): Promise<NotificationSummaryGetReply> {
  return ok(api.GET('/notifications/summary', signal === undefined ? {} : { signal }));
}

export interface InboxQuery {
  /** Only what is not read yet. */
  unread?: boolean;
  /** The `next_cursor` of the page before. */
  cursor?: string | null;
  limit?: number;
}

/** One page of the Inbox, newest first. */
export function fetchNotifications(query: InboxQuery = {}, signal?: AbortSignal): Promise<NotificationPage> {
  const asked: { unread?: boolean; cursor?: string; limit: number } = { limit: query.limit ?? INBOX_PAGE };
  if (query.unread === true) asked.unread = true;
  if (query.cursor !== undefined && query.cursor !== null) asked.cursor = query.cursor;
  return ok(api.GET('/notifications', { params: { query: asked }, ...(signal === undefined ? {} : { signal }) }));
}

/** Marks read the rows named, those of a place or a chat, or those written before a moment; answers with what is still unread. */
export function markRead(body: NotificationReadBody): Promise<NotificationReadReply> {
  return ok(api.POST('/notifications/read', { body }));
}

// ---------------------------------------------------------------- what a person chose

export const notificationPrefsKey = ['notification-prefs'] as const;

export const notificationPrefsQuery = queryOptions({
  queryKey: notificationPrefsKey,
  queryFn: ({ signal }): Promise<NotificationPrefs> => ok(api.GET('/me/notification-prefs', { signal })),
});

/** Changes what is given; what is left out stays. */
export function setNotificationPrefs(body: MeNotificationPrefsSetBody): Promise<NotificationPrefs> {
  return ok(api.PUT('/me/notification-prefs', { body }));
}

// ---------------------------------------------------------------- the ways out

export const notifiersKey = ['notifiers'] as const;

/** The kinds of channel this system has. */
export const notifiersQuery = queryOptions({
  queryKey: notifiersKey,
  queryFn: ({ signal }): Promise<Notifier[]> => ok(api.GET('/notifiers', { signal })),
});

export const notificationChannelsKey = ['notification-channels'] as const;

/** The channels that can reach the caller, each with their own route through it. */
export const notificationChannelsQuery = queryOptions({
  queryKey: notificationChannelsKey,
  queryFn: ({ signal }): Promise<NotificationChannel[]> => ok(api.GET('/notification-channels', { params: { query: {} }, signal })),
});

/** The caller's own switch for a channel: on or off, what it carries, how long a notification waits unread, how often a message goes. */
export function setRoute(channelId: string, body: MeNotificationRouteSetBody): Promise<NotificationRoute> {
  return ok(api.PUT('/me/notification-routes/{channel_id}', { params: { path: { channel_id: channelId } }, body }));
}

/** A link that opens a bot with a code, good for ten minutes. */
export function linkRoute(channelId: string): Promise<NotificationRouteLink> {
  return ok(api.POST('/me/notification-routes/{channel_id}/link', { params: { path: { channel_id: channelId } } }));
}

/** Switches the route off and unlinks it. */
export function deleteRoute(channelId: string): Promise<void> {
  return done(api.DELETE('/me/notification-routes/{channel_id}', { params: { path: { channel_id: channelId } } }));
}

/** One sample message through a channel, now; a mail server takes the address to send to. */
export function testChannel(channelId: string, to?: string): Promise<NotificationTestReply> {
  return ok(
    api.POST('/notification-channels/{id}/test', {
      params: { path: { id: channelId } },
      body: to === undefined ? {} : { to },
    }),
  );
}

/** A webhook of the caller's own. */
export function createOwnWebhook(body: MeNotificationChannelCreateBody, key: string): Promise<NotificationChannel> {
  return ok(api.POST('/me/notification-channels', { body, ...repeatable(key) }));
}

export function updateChannel(channelId: string, body: NotificationChannelUpdateBody): Promise<NotificationChannel> {
  return ok(api.PATCH('/notification-channels/{id}', { params: { path: { id: channelId } }, body }));
}

/** Removes a channel; every route through it ends. */
export function deleteChannel(channelId: string): Promise<void> {
  return done(api.DELETE('/notification-channels/{id}', { params: { path: { id: channelId } } }));
}

// ---------------------------------------------------------------- push in this browser

/** Tells the server that this browser takes push. */
export function addPushSubscription(body: MePushSubscriptionCreateBody): Promise<void> {
  return done(api.POST('/me/push-subscriptions', { body }));
}

export function removePushSubscription(endpoint: string): Promise<void> {
  return done(api.DELETE('/me/push-subscriptions', { body: { endpoint } }));
}
