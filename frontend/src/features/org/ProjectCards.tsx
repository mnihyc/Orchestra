/** Project and department summaries as compact named rows.
 *
 * What an org's home shows of the things in it, one row each: a project with how many topics are open
 * in it and how many folders it holds, what of its work needs a look (in its folders too) and when
 * something last happened, and a department with its projects and active people, including inherited roles.
 * The whole name wraps as a native link. Descriptions and tags open on demand beside the place menu;
 * a department's routing address is used in the link, never displayed as its description.
 */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import { PlaceDetails } from '@/features/navigation/PlaceDetails';
import { type PlaceStats, PlaceLink, placeOfNode } from '@/features/navigation/places';
import type { Org, Project } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { NodeIcon } from '@/ui/NodeIcon';
import { RelativeTime } from '@/ui/RelativeTime';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { StateChip } from '@/ui/StateChip';
import { ContainedThreadStateIcon } from '@/ui/ThreadState';
import { Tooltip } from '@/ui/Tooltip';

/** What needs a look in a place, as the chips a card, a row and a board's card share: nothing is drawn where nothing needs it. */
export function AttentionChips({ stats, className }: { stats: Pick<PlaceStats, 'running' | 'waiting' | 'approvals' | 'blocked'>; className?: string }) {
  const { t } = useTranslation();
  const approvals = Math.max(stats.approvals, stats.waiting);
  if (stats.running + approvals + stats.blocked === 0) return null;
  return (
    <div className={cx('flex flex-wrap items-center gap-2', className)}>
      {stats.blocked > 0 && <StateChip state="blocked" label={t('places.attention.blocked', { count: stats.blocked })} />}
      {approvals > 0 && <StateChip state="waiting_approval" label={t('places.attention.approvals', { count: approvals })} />}
      {stats.running > 0 && <StateChip state="running" label={t('places.attention.running', { count: stats.running })} />}
    </div>
  );
}

const CARD =
  'group relative flex min-w-0 items-start gap-3 px-3 py-2 transition-colors hover:bg-bg-subtle';

/** A full wrapping title; the row's details and menu remain separate keyboard targets. */
const CARD_LINK = 'focus-inset block font-medium leading-5 wrap-anywhere text-text hover:underline';

function Mark({ children }: { children: React.ReactNode }) {
  return <span className="flex h-(--control-sm) w-5 shrink-0 items-center justify-center text-text-muted">{children}</span>;
}

export function ProjectCard({ project, stats }: { project: Project; stats: PlaceStats }) {
  const { t } = useTranslation();
  return (
    <article className={CARD}>
        <Mark>
          <NodeIcon kind="project" color={project.color} own={project.custom_color} />
        </Mark>
        <div className="flex min-w-0 flex-1 flex-col gap-1 pt-1">
          <h3 className="min-w-0 text-base">
            <PlaceLink place={project} className={CARD_LINK}>
              {project.name}
            </PlaceLink>
          </h3>
      <AttentionChips stats={stats} />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
        <span>{t('places.cards.openTopics', { count: stats.openTopics })}</span>
        {stats.folders > 0 && <span>{t('places.cards.folders', { count: stats.folders })}</span>}
        {stats.lastActivity !== null && (
          <RelativeTime at={stats.lastActivity} />
        )}
      </div>
      </div>
      <div className="flex shrink-0 items-start gap-1"><ContainedThreadStateIcon id={project.id} /><AsAdminMark of={adminAccessOf(project)?.place} responsive /><PlaceDetails node={project} row /><NodeMenuButton place={placeOfNode(project)} node={project} /></div>
    </article>
  );
}

export function DepartmentCard({ org, stats }: { org: Org; stats: PlaceStats }) {
  const { t } = useTranslation();
  return (
    <article className={CARD}>
        <Mark>
          <NodeIcon kind="org" color={org.color} own={org.custom_color} />
        </Mark>
        <div className="flex min-w-0 flex-1 flex-col gap-1 pt-1">
          <h3 className="min-w-0 text-base">
            <Link to="/org/$slug" params={{ slug: org.address }} className={CARD_LINK}>
              {org.name}
            </Link>
          </h3>
      <AttentionChips stats={stats} />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-text-muted">
        <span>{t('places.cards.projects', { count: org.project_count })}</span>
        {org.people_count !== undefined && org.people_count !== null && <Tooltip content={t('places.cards.peopleIncludingInherited')}>
          <span tabIndex={0}>{t('places.cards.peopleWorking', { count: org.people_count })}</span>
        </Tooltip>}
      </div>
      </div>
      <div className="flex shrink-0 items-start gap-1"><ContainedThreadStateIcon id={org.id} /><AsAdminMark of={adminAccessOf(org)?.place} responsive /><PlaceDetails node={org} row /><NodeMenuButton place={placeOfNode(org)} node={org} /></div>
    </article>
  );
}
