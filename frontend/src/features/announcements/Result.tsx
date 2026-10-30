/** Recorded delivery counts and per-topic outcomes.
 */
import { useTranslation } from 'react-i18next';
import type { AnnouncementDelivered, PlaceAnnouncementCreateReply } from '@/generated/api-types';

export function DeliveryCounts({ counts }: { counts: AnnouncementDelivered }) {
  const { t } = useTranslation();
  return <span>{t('placeAnnouncements.counts', counts)}</span>;
}

export function AnnouncementResult({ reply }: { reply: PlaceAnnouncementCreateReply }) {
  const { t } = useTranslation();
  const counts = reply.announcements.reduce<AnnouncementDelivered>((sum, row) => ({ interrupted: sum.interrupted + row.delivered.interrupted,
    steered: sum.steered + row.delivered.steered, queued: sum.queued + row.delivered.queued, deferred: sum.deferred + row.delivered.deferred }),
  { interrupted: 0, steered: 0, queued: 0, deferred: 0 });
  return <div role="status" className="flex flex-col gap-2"><p>{t('placeAnnouncements.sent', { count: reply.announcements.length })}</p><DeliveryCounts counts={counts} /><p>{t('placeAnnouncements.skipped', { count: reply.topics_skipped })}</p></div>;
}
