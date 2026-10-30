/** A channel manager sees the supplied route count, without a list of people or addresses. */
import { useTranslation } from 'react-i18next';

import type { NotificationChannel } from '@/generated/api-types';

export function enabledRouteCount(channel: NotificationChannel): number | null {
  if (!channel.can_edit || !('enabled_route_count' in channel)) return null;
  return typeof channel.enabled_route_count === 'number' ? channel.enabled_route_count : null;
}

export function EnabledRoutes({ channel }: { channel: NotificationChannel }) {
  const { t } = useTranslation();
  const count = enabledRouteCount(channel);
  return count === null ? null : <p className="text-sm text-text-muted">{t('away.manager.count', { count })}</p>;
}
