/** Current notification contracts shared by delivery, device and grouped Inbox controls. */
import type { NotificationPrefs, NotificationRoute } from '@/generated/api-types';
export type {
  Notification as AwayNotification, NotificationPage as AwayPage, NotificationGroup,
  NotificationSummaryGetReply as AwaySummary, OutsideStatus, NotificationPrefs as AwayPrefs,
  MeNotificationPrefsSetBody as PrefsChange, NotificationRoute as AwayRoute,
  NotificationChannel as AwayChannel, MeNotificationRouteSetBody as RouteChange, PushDevice,
  NotificationReadBody as GroupReadBody, NotificationReadReply as ReadResult,
} from '@/generated/api-types';

/** Only a supplied outcome can be presented as one; absence on an older cached row stays unknown. */
export function suppliedRoute(route: NotificationRoute): NotificationRoute | null {
  return 'health' in route && 'waiting_count' in route ? route : null;
}
/** A pause is changed only from a supplied preference revision. */
export function suppliedPrefs(prefs: NotificationPrefs): NotificationPrefs | null {
  return 'revision' in prefs && typeof prefs.revision === 'number' && 'pause_until' in prefs ? prefs : null;
}
