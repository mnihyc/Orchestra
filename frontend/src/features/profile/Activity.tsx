/** Fifty-three weeks of visible work, with every count also available as a table. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { profileActivityQuery } from '@/api/profile';
import { PlaceLink } from '@/features/navigation/places';
import type { ProfileActivity } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';

/** UTC calendar arithmetic uses whole days, independently of the browser's daylight-saving changes. */
const DAY_MS = 24 * 60 * 60 * 1000;
/** Five ranks use explicit borders and accessible counts as well as theme colours. */
const RANK_CLASSES = ['bg-bg-muted border-border', 'bg-accent-subtle border-accent-text/25', 'bg-accent-subtle border-accent-text/60', 'bg-accent-solid border-accent-solid', 'bg-accent-solid border-text'] as const;

export function calendarDays(activity: ProfileActivity): { day: string; turns: number }[] {
  const counts = new Map(activity.days.map((day) => [day.day, day.turns]));
  const from = Date.parse(`${activity.from}T00:00:00Z`);
  const to = Date.parse(`${activity.to}T00:00:00Z`);
  if (!Number.isFinite(from) || !Number.isFinite(to) || to < from) return [];
  return Array.from({ length: Math.floor((to - from) / DAY_MS) + 1 }, (_, index) => {
    const day = new Date(from + index * DAY_MS).toISOString().slice(0, 10);
    return { day, turns: counts.get(day) ?? 0 };
  });
}

export function ActivityCalendar({ activity }: { activity: ProfileActivity }) {
  const { t } = useTranslation();
  const [table, setTable] = useState(false);
  const days = calendarDays(activity);
  // Relative ranks adapt to this person's own visible calendar; no hidden work shapes the colours.
  const maximum = Math.max(1, ...days.map((day) => day.turns));
  return <div className="flex min-w-0 flex-col gap-3">
    <div className="flex flex-wrap items-center justify-between gap-2"><p className="font-medium">{t('profile.page.turns', { count: activity.total })}</p><Button size="sm" onClick={() => setTable(!table)} aria-expanded={table}>{t(table ? 'profile.page.hideTable' : 'profile.page.showTable')}</Button></div>
    <div data-scroll-allow="diagram" className="overflow-x-auto pb-2" role="group" aria-label={t('profile.page.work')}>
      <div className="grid w-max grid-flow-col grid-rows-7 gap-1">
        {days.map(({ day, turns }) => {
          const rank = turns === 0 ? 0 : Math.min(4, Math.ceil(turns / maximum * 4));
          const label = t('profile.page.dayCount', { day, count: turns });
          return <Tooltip key={day} content={label}><span tabIndex={0} role="img" aria-label={label} className={`inline-block size-3 rounded-xs border ${RANK_CLASSES[rank]}`} /></Tooltip>;
        })}
      </div>
    </div>
    {table && <ScrollRegion kind="table" label={t('profile.page.work')} className="rounded-md border border-border"><table className="w-full text-left text-sm"><caption className="sr-only">{t('profile.page.work')}</caption><thead><tr><th scope="col" className="p-2">{t('profile.page.day')}</th><th scope="col" className="p-2">{t('profile.page.turnsLabel')}</th></tr></thead><tbody>{days.map(({ day, turns }) => <tr key={day} className="border-t border-border"><th scope="row" className="p-2 font-normal"><time dateTime={day}>{day}</time></th><td className="p-2 tabular-nums">{turns}</td></tr>)}</tbody></table></ScrollRegion>}
    {activity.recent.length > 0 && <div><h3 className="mb-2 text-sm font-medium text-text-muted">{t('profile.page.recent')}</h3><ul className="flex flex-col gap-2">{activity.recent.map(({ node, last_at }) => <li key={node.id} className="flex flex-wrap items-center justify-between gap-2 text-sm"><PlaceLink place={node} className="text-accent-text hover:underline">{node.name}</PlaceLink><RelativeTime at={last_at} /></li>)}</ul></div>}
  </div>;
}

export function Activity({ username }: { username: string }) {
  const { t } = useTranslation();
  const query = useQuery(profileActivityQuery(username));
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} compact />;
  return <section aria-label={t('profile.page.work')} className="flex min-w-0 flex-col gap-2"><h2 className="text-lg font-semibold">{t('profile.page.work')}</h2><p className="text-sm text-text-muted">{t('profile.page.visibleWork')}</p><ActivityCalendar activity={query.data} /></section>;
}
