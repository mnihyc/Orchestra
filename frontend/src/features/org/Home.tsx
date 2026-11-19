/** One home for an org at every level: the orgs inside it, its projects, running work, attention, documents and the usage board; for a person's own org, what it is and the ways to begin.
 *
 * One screen serves every org, a root org and a department alike, at the address of its short name or
 * of its path. Under the name and what can be done with the org it has three sections: the overview,
 * with this person's waiting and recent threads first, a shared filter immediately before the orgs
 * and projects inside it as compact rows, then guidance and context; its members, with invitations;
 * and Activity. Description and tags open on demand from the header.
 *
 * A person's own org begins empty and says what it is. What the reader may not do stays on the
 * screen, switched off, saying what it takes and whom to ask.
 */
import { useContentFocused } from '@/features/workbench/instance';
import { OutsideOpenings } from '@/features/ports/Outside';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { BriefcasePlus, FolderOpen, LayoutGrid } from 'lucide-react';
import { Activity, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { childrenQuery, orgBySlugQuery } from '@/api/nodes';
import { PlaceReadme } from '@/features/documents/Readme';
import { ActivityFeed, OrgActivity } from '@/features/org/Activity';
import { UsageSection } from '@/features/usage/Board';
import { Members } from '@/features/settings/org/Members';
import { DepartmentCard, ProjectCard } from '@/features/org/ProjectCards';
import { OrgWork } from '@/features/org/Work';
import { OrgHeaderMenu } from '@/features/org/HeaderMenu';
import { orgPageTitle } from '@/features/org/title';
import { useRetainedOrg } from '@/features/org/useRetainedOrg';
import { StarButton } from '@/features/navigation/favorites';
import { Checklist } from '@/features/onboarding/Checklist';
import { CreationPermissionLink } from '@/features/help/CreationPermissionLink';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { orgColor, orgInitial } from '@/features/navigation/OrgSwitcher';
import { placeOfNode, placeStats } from '@/features/navigation/places';
import type { Org, Project } from '@/generated/api-types';
import { denial, holds } from '@/lib/permissions';
import { useOrgTreeFeed } from '@/realtime/live';
import { useOrgTree } from '@/realtime/stores/tree';
import { selectOrg } from '@/shell/org';
import { boxClasses, Page, PageHeader, PageSectionContext, Section } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { NodeFailure } from '@/features/access/NoAccess';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { PermissionGate } from '@/ui/PermissionGate';
import { Pagination } from '@/ui/Pagination';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { Tabs } from '@/ui/Tabs';

export type OrgTab = 'overview' | 'members' | 'activity';

/** How many lines of Activity the overview shows before the Activity tab. */
const ACTIVITY_SHOWN = 5;

function HomeSkeleton() {
  return (
    <Page>
      <SkeletonGroup className="flex flex-col gap-6">
        <div className="flex items-center gap-4">
          <Skeleton className="size-10 rounded-lg" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-6 w-1/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        </div>
        <div className="flex flex-col gap-3">
          {Array.from({ length: 4 }, (_, row) => <Skeleton key={row} className="h-12 w-full" />)}
        </div>
      </SkeletonGroup>
    </Page>
  );
}

function Overview({ org, onTab }: { org: Org; onTab: (tab: OrgTab) => void }) {
  const [participating, setParticipating] = useState(false);
  return <div className="flex flex-col gap-6">
    <OrgWork key={org.id} org={org} />
    <FilterBar participation={<ParticipationFilter checked={participating} onCheckedChange={setParticipating} />} />
    <OverviewRows org={org} onTab={onTab} participating={participating} />
    <Checklist org={org} />
    {org.readme !== null && <PlaceReadme readme={org.readme} />}
  </div>;
}

function OverviewRows({ org, onTab, participating }: { org: Org; onTab: (tab: OrgTab) => void; participating: boolean }) {
  const { t } = useTranslation();
  const tree = useOrgTree(org.org_id);
  const children = useInfiniteQuery(childrenQuery(org.id, 'exclude', participating));
  const own = org.org_kind === 'personal';

  const items = children.data?.pages.flatMap((page) => page.items) ?? [];
  const departments = items.filter((node): node is Org => node.kind === 'org');
  const projects = items.filter((node): node is Project => node.kind === 'project');
  const mayMakeProject = holds(org.access, 'org.interact');
  const pagination = <Pagination hasMore={children.hasNextPage} loading={children.isFetchingNextPage} error={children.isFetchNextPageError ? children.error : undefined} onMore={() => void children.fetchNextPage()} />;

  if (children.isPending) return <SkeletonRows rows={4} />;
  if (children.isError && items.length === 0) {
    return <ErrorState compact error={children.error} retrying={children.isRefetching} onRetry={() => void children.refetch()} />;
  }

  // Nothing here yet: a person's own org says what to do first; another org says what is needed to begin.
  if (items.length === 0) {
    return <>
      <EmptyState
        className={boxClasses}
        icon={own ? <FolderOpen aria-hidden /> : <BriefcasePlus aria-hidden />}
        title={own ? t('places.org.ownEmpty.title') : t('places.org.empty.title')}
        description={
          own
            ? t('places.org.ownEmpty.about')
            : mayMakeProject
              ? t('places.org.empty.about')
              : t('places.org.empty.noRight', { name: org.owner.display_name })
        }
        action={mayMakeProject || own ? undefined : <CreationPermissionLink place={{ kind: 'org', id: org.id, name: org.name }} />}
      />
      {pagination}
    </>;
  }

  return (
    <div className="flex flex-col gap-6">
      {departments.length > 0 && (
        <Section
          id="departments"
          title={t('places.org.departments')}
        >
            <ul data-places-list aria-label={t('places.org.departments')} className={`${boxClasses} divide-y divide-border`}>
              {departments.map((department) => (
                <li key={department.id} className="min-w-0"><DepartmentCard org={department} stats={placeStats(tree.nodes, department.id)} /></li>
              ))}
            </ul>
        </Section>
      )}

      <Section id="projects" title={t('places.org.projects')}>
        {projects.length === 0 ? (
          <EmptyState compact title={t('places.org.noProjects')} />
        ) : (
          <ul data-places-list aria-label={t('places.org.projects')} className={`${boxClasses} divide-y divide-border`}>
            {projects.map((project) => (
              <li key={project.id} className="min-w-0"><ProjectCard project={project} stats={placeStats(tree.nodes, project.id)} /></li>
            ))}
          </ul>
        )}
      </Section>

      {pagination}

      <UsageSection nodeId={org.id} name={org.name} />
      <Section
        id="lately"
        title={t('places.org.lately')}
        action={
          <Button size="sm" variant="ghost" onClick={() => onTab('activity')}>
            {t('places.org.allActivity')}
          </Button>
        }
      >
        <ActivityFeed nodeId={org.id} preview={ACTIVITY_SHOWN} />
      </Section>

      {org.access.administers && <OutsideOpenings placeId={org.id} />}
    </div>
  );
}

export interface OrgHomeProps {
  /** A root org's short name, or the address of an org inside one. */
  slug: string;
  tab: OrgTab;
  onTab: (tab: OrgTab) => void;
}

export function OrgHome({ slug, tab, onTab }: OrgHomeProps) {
  const { t } = useTranslation();
  const membersLabel = useId();
  const focusedContent = useContentFocused();
  const userId = useStore(client.session, (state) => state.userId);
  const query = useQuery(orgBySlugQuery(slug));
  const org = query.isSuccess ? query.data : undefined;
  const data = useRetainedOrg(JSON.stringify([userId, slug]), org, query.isPending);

  // The root org of what is in view is the one whose navigation the left bar shows, and whose tree this screen reads.
  useEffect(() => {
    if (focusedContent && org !== undefined) selectOrg(userId, org.org_id);
  }, [org, userId, focusedContent]);
  useOrgTreeFeed(org?.org_id ?? null);
  useScreenHeading({ title: org === undefined ? t('screens.org.title') : orgPageTitle(org, t), icon: LayoutGrid, nodeId: org?.id, views: { ready: org !== undefined, chat: org?.org_kind !== 'personal' } });

  const failure = query.isError ? <NodeFailure className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} /> : null;
  if (data === undefined) return failure ?? <HomeSkeleton />;

  const own = data.org_kind === 'personal';
  const place = placeOfNode(data);
  const mayMakeProject = holds(data.access, 'org.interact');
  const mayCreate = mayMakeProject && !data.archived;
  const whyNot = data.archived ? t('places.project.archived') : denial(t, 'org.interact', data.name, data.owner);

  return (
    <>
    {query.isPending && <HomeSkeleton />}
    <Activity key={JSON.stringify([userId, data.id])} mode={query.isSuccess ? 'visible' : 'hidden'}>
    <Page>
      <PageHeader
        adminAccess={data.access.admin_access} private={data.page === 'restricted'} opening={{ kind: 'org', id: data.id }}
        mark={<Avatar text={orgInitial(data)} color={orgColor(data)} kind={own ? 'human' : 'org'} size={40} />}
        title={orgPageTitle(data, t)}
        lead={data.description === '' ? undefined : data.description}
        actions={
          <>
            <StarButton place={place} />
            <PermissionGate allowed={mayCreate} mode="block" reason={whyNot}>
              <Button size="sm" variant="primary" icon={<BriefcasePlus aria-hidden />} onClick={() => openNodeDialog({ type: 'new-project', parent: place })}>
                {t('places.org.newProject')}
              </Button>
            </PermissionGate>
            <OrgHeaderMenu org={data} />
          </>
        }
      />
      {data.archived && <InlineAlert tone="info">{t('places.project.archived')}</InlineAlert>}
      <Tabs
        label={t('places.org.sections')}
        value={tab}
        onValueChange={onTab}
        tabs={[
          { value: 'overview', label: t('places.org.tabs.overview'), panel: <Overview org={data} onTab={onTab} /> },
          ...(own
            ? []
            : [{ value: 'members' as const, label: <span id={membersLabel}>{t('places.org.tabs.members')}</span>, count: data.member_count,
              panel: <PageSectionContext value={{ title: t('settings.org.sectionNames.members'), id: membersLabel }}><Members org={data} /></PageSectionContext> }]),
          { value: 'activity', label: t('places.org.tabs.activity'), panel: <OrgActivity orgId={data.id} /> },
        ]}
        panelClassName="pt-6"
      />
    </Page>
    </Activity>
    </>
  );
}
