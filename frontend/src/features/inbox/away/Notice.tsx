/** A route's historical Inbox notice names only supplied safe outcome facts and its existing repair card. */
import { useTranslation } from 'react-i18next';

import type { Notification } from '@/generated/api-types';
import { RelativeTime } from '@/ui/RelativeTime';

export function noticeWords(row: Notification): { way: string | null; reason: string | null; count: number | null; accepted: string | null; devices: string[] } {
  const data = typeof row.data === 'object' && row.data !== null ? row.data as Record<string, unknown> : {};
  return { way: typeof data.way === 'string' ? data.way : null, reason: typeof data.reason === 'string' ? data.reason : null, count: typeof data.waiting_count === 'number' ? data.waiting_count : null, accepted: typeof data.last_ok_at === 'string' ? data.last_ok_at : null, devices: Array.isArray(data.affected_devices) ? data.affected_devices.filter((device): device is string => typeof device === 'string') : [] };
}

export function RouteNotice({ row }: { row: Notification }) {
  const { t } = useTranslation();
  const words = noticeWords(row);
  return <span className="flex flex-col gap-1 text-sm text-text-muted">{words.reason !== null && <span>{words.reason}</span>}{words.devices.length > 0 && <span>{t('away.outcome.partial', { devices: words.devices.join(t('startFlow.separator')) })}</span>}<span>{t('away.outcome.lastAccepted')} {words.accepted === null ? t('away.outcome.never') : <RelativeTime at={words.accepted} />}</span>{words.count !== null && <span>{t('away.outcome.waiting', { count: words.count })}</span>}<span className="font-medium text-accent-text">{t('away.outcome.open')}</span></span>;
}
