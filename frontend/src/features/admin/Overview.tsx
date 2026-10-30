/** Health, heartbeats, queues, versions, charts of the stored metrics, and maintenance banner.
 *
 * How the system stands: its version and build, each of its parts with the last time it said it was
 * alive, how much waits in its queues, how many machines are connected and how many threads are
 * running, and under them the charts of what the server stored about itself over a day, a week, a
 * month, a year or since it was installed. The state is asked again while the screen is in view, as
 * often as the parts report; a part that has not reported lately is shown as failing, with what it
 * says, so that a person looking at this knows before the people who use the system do.
 */
import { useQuery } from '@tanstack/react-query';
import { CircleAlert, CircleCheck } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';

import { systemQuery } from '@/api/admin';
import { BannerEditor } from '@/features/admin/BannerEditor';
import { OverviewCharts } from '@/features/admin/Charts';
import { Installation } from '@/features/admin/Installation';
import { SPANS } from '@/features/admin/search';
import type { SectionProps } from '@/features/admin/sections';
import type { SystemComponent, SystemInfo } from '@/generated/api-types';
import { Figure, FIGURE_GRID } from '@/features/admin/Figure';
import { boxClasses, Section } from '@/shell/Page';
import { cx } from '@/ui/cx';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

// A workbench pane may be much narrower than the window. Reflow the same cards within its
// own container; the loading view uses the identical column rules.
const PART_GRID = 'grid gap-3 @sm:grid-cols-2 @3xl:grid-cols-4';

/** One part of the system: its name, whether it answers, what it says when it does not, and when it last said it was alive. */
function PartCard({ part }: { part: SystemComponent }) {
  const { t } = useTranslation();
  const Icon = part.ok ? CircleCheck : CircleAlert;
  const hasDetail = part.detail !== null && part.detail !== '';
  return (
    <li className={cx(boxClasses, 'flex min-w-0 flex-col gap-1 p-3', !part.ok && 'border-danger bg-danger-subtle')} data-part={part.name} data-ok={part.ok}>
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <Icon aria-hidden className={part.ok ? 'text-success' : 'text-danger'} />
        <h3 className="text-base font-medium text-text">{t(`admin.overview.parts.${part.name}`)}</h3>
        <span className="ml-auto text-sm whitespace-nowrap text-text">{part.ok ? t('admin.overview.well') : t('admin.overview.failing')}</span>
      </div>
      {hasDetail && <p className="text-sm wrap-anywhere text-text-muted">{part.detail}</p>}
      {part.seen_at !== null ? (
        <p className="text-sm text-text-muted">
          {t('admin.overview.lastHeartbeat')} <RelativeTime at={part.seen_at} />
        </p>
      ) : (
        // What the part says of itself stands where there is something said; the words of this screen where there is not.
        !part.ok && !hasDetail && <p className="text-sm text-text-muted">{t('admin.overview.noHeartbeat')}</p>
      )}
    </li>
  );
}

function State({ system }: { system: SystemInfo }) {
  const { t } = useTranslation();
  return (
    <>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
        <span>{t('admin.overview.version', { version: system.version })}</span>
        <span aria-hidden>·</span>
        <span>{t('admin.overview.build', { build: system.build })}</span>
        <span aria-hidden>·</span>
        <span>
          {t('admin.overview.started')} <RelativeTime at={system.started_at} />
        </span>
      </p>

      <Section id="admin-parts" title={t('admin.overview.partsTitle')} lead={t('admin.overview.partsLead')}>
        <ul className={PART_GRID}>
          {system.components.map((part) => (
            <PartCard key={part.name} part={part} />
          ))}
        </ul>
      </Section>

      <Section id="admin-now" title={t('admin.overview.nowTitle')}>
        <ul className={FIGURE_GRID}>
          <Figure label={t('admin.overview.machines')} value={system.machines_online} />
          <Figure label={t('admin.overview.threads')} value={system.threads_running} />
          <Figure label={t('admin.overview.events')} value={system.queues.outbox_pending} />
          <Figure label={t('admin.overview.commands')} value={system.queues.commands_open} />
          <Figure label={t('admin.overview.jobs')} value={system.queues.jobs_running} />
        </ul>
      </Section>
    </>
  );
}

function Loading() {
  return (
    <SkeletonGroup className="flex flex-col gap-6">
      <Skeleton className="h-4 w-1/3" />
      <div className={PART_GRID}>
        {[0, 1, 2, 3].map((at) => (
          <Skeleton key={at} className="h-16 w-full" />
        ))}
      </div>
      <div className={FIGURE_GRID}>
        {[0, 1, 2, 3, 4].map((at) => (
          <Skeleton key={at} className="h-16 w-full" />
        ))}
      </div>
    </SkeletonGroup>
  );
}

export function Overview({ search, onSearch }: SectionProps) {
  const { t } = useTranslation();
  const system = useQuery(systemQuery);
  const span = search.span ?? 'day';
  return (
    <div className="@container flex min-w-0 flex-col gap-6">
      {system.isPending ? (
        <Loading />
      ) : system.isError ? (
        <ErrorState error={system.error} retrying={system.isRefetching} onRetry={() => void system.refetch()} />
      ) : (
        <State system={system.data} />
      )}

      <Installation />

      <Section
        id="admin-over-time"
        title={t('admin.overview.overTime')}
        lead={t('admin.overview.overTimeLead')}
        action={
          <SegmentedControl
            label={t('admin.overview.span')}
            value={span}
            onValueChange={(next) => onSearch({ span: next === 'day' ? undefined : next })}
            segments={SPANS.map((one) => ({ value: one, ...choiceOption(t, `period.${one}`) }))}
          />
        }
      >
        <OverviewCharts span={span} />
      </Section>

      <Section id="admin-banner" title={t('admin.overview.banner.title')} lead={t('admin.overview.banner.lead')}>
        <BannerEditor />
      </Section>
    </div>
  );
}
