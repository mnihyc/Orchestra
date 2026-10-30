/** A compact project header and one filter bar above its topics.
 *
 * The header owns the name, one New topic and the named List, Board and Activity views. Description
 * and tags open on demand; secondary container creation stays in the place menu. The active collection
 * has one shared filter bar, then its content. Provider defaults are edited by name in the project's
 * existing settings row, so this screen mounts neither a setup form nor its provider inventory.
 *
 * A folder is a project that lies in a project, and this is its screen as well, with a folder glyph in
 * place of the project's own. Where there are folders their compact rows follow the topics; the list
 * and the board show the project's own topics, and its Activity takes in its folders and says so.
 *
 * What is shown is in the address, so that a link reproduces it: the view, the filters and the search.
 */
import { useContentFocused } from '@/features/workbench/instance';
import { OutsideOpenings } from '@/features/ports/Outside';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useRouter } from '@tanstack/react-router';
import { Archive, Briefcase, Folder, ListFilter, Plus, Settings, Tags } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption } from '@/i18n/choices';
import { ParticipationFilter } from '@/features/access/ParticipationFilter';
import { FilterBar } from '@/ui/FilterBar';
import { PlaceAnnouncements } from '@/features/announcements/Composer';
import { SubscriptionBell } from '@/features/thread/SubscriptionBell';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { DEFAULT_TOPIC_FILTERS, projectQuery, tagsQuery, type TopicFilters, unarchiveNode } from '@/api/nodes';
import { PlaceReadme } from '@/features/documents/Readme';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import { usePlaceHeaderPanels } from '@/features/navigation/PlaceHeaderPanels';
import { PlaceDetails, PlaceSettingsLink } from '@/features/navigation/PlaceDetails';
import { placeOfNode, type PlaceRef } from '@/features/navigation/places';
import { ProjectActivity } from '@/features/project/Activity';
import { UsageSection } from '@/features/usage/Board';
import { FolderCards } from '@/features/project/FolderCards';
import { Topics } from '@/features/project/Topics';
import { ProjectStar } from '@/features/project/ProjectStar';
import { searchFromView, viewFromSearch } from '@/features/project/viewState';
import type { Project as ProjectNode, Tag, TopicSort, TopicStatusFilter, ProjectView as PersonalProjectView } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { denial, holds } from '@/lib/permissions';
import { useOrgTreeFeed } from '@/realtime/live';
import { selectOrg } from '@/shell/org';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { NodeFailure } from '@/features/access/NoAccess';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { NodeIcon } from '@/ui/NodeIcon';
import { Popover } from '@/ui/Popover';
import { PermissionGate } from '@/ui/PermissionGate';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { TagChip } from '@/ui/TagChip';
import { toast } from '@/ui/Toast';

export type ProjectView = 'list' | 'board' | 'activity';

/** What the address of a project says about what is shown; whatever it leaves out is the default. */
export interface ProjectSearch {
  view?: ProjectView;
  status?: TopicStatusFilter;
  tag?: string[];
  attention?: Exclude<PersonalProjectView['attention'], null>;
  sort?: TopicSort;
  q?: string;
  archived?: boolean;
  with_folders?: boolean;
  participating?: boolean;
}

/** The project's "…": its menu, with a named entry for each icon of its header (Details, Announcements, Settings, Notifications). */
function ProjectHeaderMenu({ project, place }: { project: ProjectNode; place: PlaceRef }) {
  const { t } = useTranslation();
  const router = useRouter();
  const panels = usePlaceHeaderPanels(project, ['details', 'announcements', 'notifications'], t('notificationBell.place'));
  const settings: MenuEntry = { id: 'project-settings', label: t('places.org.settings'), icon: <Settings aria-hidden />,
    onSelect: () => void router.navigate({ to: '/project/$projectId/settings', params: { projectId: project.id } }) };
  return <>
    <NodeMenuButton place={place} node={project} where="screen" extraEntries={[...panels.entries, settings]} triggerRef={panels.opener} />
    {panels.dialogs}
  </>;
}

/** The request the filters of an address make. The board shows every status, since its columns are the statuses. */
export function filtersOf(search: ProjectSearch, view: ProjectView): TopicFilters {
  return {
    status: view === 'board' ? 'all' : (search.status ?? DEFAULT_TOPIC_FILTERS.status),
    tags: search.tag ?? [],
    attention: search.attention ?? null,
    sort: search.sort ?? DEFAULT_TOPIC_FILTERS.sort,
    q: search.q ?? '',
    archived: search.archived === true ? 'include' : 'exclude',
    ...(search.with_folders === true ? { withFolders: true } : {}),
    ...(search.participating === true ? { participating: true } : {}),
  };
}

/** Whether the address narrows what is shown beyond the defaults. */
export function isFiltered(search: ProjectSearch): boolean {
  return (
    (search.status !== undefined && search.status !== DEFAULT_TOPIC_FILTERS.status) ||
    (search.tag?.length ?? 0) > 0 ||
    search.attention !== undefined ||
    (search.q ?? '') !== '' ||
    search.archived === true ||
    search.with_folders === true || search.participating === true
  );
}

function TagsFilter({ tags, chosen, onChange }: { tags: readonly Tag[]; chosen: readonly string[]; onChange: (tags: string[]) => void }) {
  const { t } = useTranslation();
  return (
    <Popover
      label={t('places.project.tags')}
      trigger={
        <Button size="sm" icon={<Tags aria-hidden />}>
          {t('places.project.tags')}
          {chosen.length > 0 && <span className="rounded-full bg-accent-solid px-1 text-xs text-text-on-accent">{chosen.length}</span>}
        </Button>
      }
      className="flex min-w-(--menu-min-width) flex-col gap-3"
    >
      {tags.length === 0 ? (
        <EmptyState compact title={t('places.project.noTags')} />
      ) : (
        tags.map((tag) => (
          <Checkbox
            key={tag.id}
            label={<TagChip name={tag.name} color={tag.color} />}
            checked={chosen.includes(tag.id)}
            onCheckedChange={(on) => onChange(on ? [...chosen, tag.id] : chosen.filter((id) => id !== tag.id))}
          />
        ))
      )}
    </Popover>
  );
}

function ProjectSkeleton() {
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
        <Skeleton className="h-8 w-full" />
        <div className="flex flex-col gap-3">{Array.from({ length: 5 }, (_, row) => <Skeleton key={row} className="h-10 w-full" />)}</div>
      </SkeletonGroup>
    </Page>
  );
}

export interface ProjectProps {
  projectId: string;
  search: ProjectSearch;
  /** Changes what the address says: the keys named, and nothing else. */
  onSearch: (patch: Partial<ProjectSearch>) => void;
}

export function Project(props: ProjectProps) {
  return <ProjectScreen key={props.projectId} {...props} />;
}

/** Navigation stays in the header; each active collection has exactly one filter bar below it. */
function ProjectViews({ value, onValueChange }: { value: ProjectView; onValueChange: (view: ProjectView) => void }) {
  const { t } = useTranslation();
  return <SegmentedControl size="sm" label={t('places.project.view')} value={value} onValueChange={onValueChange}
    segments={[
      { value: 'list', action: 'list', name: t('choiceCatalog.projectView.list.name') },
      { value: 'board', action: 'board', name: t('choiceCatalog.projectView.board.name') },
      { value: 'activity', action: 'activity', name: t('choiceCatalog.projectView.activity.name') },
    ]} />;
}

function ProjectScreen({ projectId, search, onSearch: navigate }: ProjectProps) {
  const { t } = useTranslation();
  const focusedContent = useContentFocused();
  const userId = useStore(client.session, (state) => state.userId);
  const query = useQuery(projectQuery(projectId));
  const project = query.data;
  const tags = useQuery(tagsQuery(projectId));
  const view = search.view ?? 'list';
  const onSearch = useCallback((patch: Partial<ProjectSearch>) => {
    if (patch.view === 'activity') { navigate({ ...search, ...patch }); return; }
    const merged = { ...search, ...patch };
    const next = viewFromSearch(merged);
    navigate({ ...searchFromView(next), status: next.status, tag: next.tag_ids, attention: next.attention ?? undefined, sort: next.sort, q: next.q, archived: next.show_archived, with_folders: next.with_folders, participating: merged.participating });
  }, [search, navigate]);

  // What is typed in the search is the box's own until its reader rests; then the address says it.
  const [typed, setTyped] = useState(search.q ?? '');
  const [previousQuery, setPreviousQuery] = useState(search.q ?? '');
  const settled = useDebounced(typed);
  const asked = search.q ?? '';
  if (previousQuery !== asked) { setPreviousQuery(asked); setTyped(asked); }
  useEffect(() => {
    if (settled === typed && settled !== asked) onSearch({ q: settled === '' ? undefined : settled });
  }, [settled, typed, asked, onSearch]);

  useEffect(() => {
    if (focusedContent && project !== undefined) selectOrg(userId, project.org_id);
  }, [project, userId, focusedContent]);
  useOrgTreeFeed(project?.org_id ?? null);
  const isFolder = project !== undefined && placeOfNode(project).folder;
  useScreenHeading({ title: project?.name ?? t('screens.project.title'), icon: isFolder ? Folder : Briefcase, nodeId: project?.id });

  const clearFilters = () => {
    setTyped('');
    onSearch({ view: 'list', status: 'open', tag: [], attention: undefined, sort: 'activity', q: '', archived: false, with_folders: false, participating: undefined });
  };

  const unarchive = useMutation({
    mutationFn: () => unarchiveNode(projectId),
    onSuccess: (node) => {
      toast({ tone: 'success', title: t('places.toast.unarchived', { name: node.name }) });
    },
  });

  if (query.isPending) return <ProjectSkeleton />;
  if (query.isError) {
    // A screen the server refused shows what is missing and whom to ask, and the way on, in place of its content.
    return <NodeFailure className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  }
  const data = query.data;
  const place = placeOfNode(data);
  const mayOpen = holds(data.access, 'project.interact');
  const mayManage = holds(data.access, 'project.manage');
  const mayArchive = mayManage || holds(data.access, 'project.archive');
  const mayCreate = mayOpen && !data.archived;
  const whyNot = data.archived ? t('places.project.archived') : denial(t, 'project.interact', data.name, data.owner);

  return (
    <Page>
      <PageHeader
        adminAccess={data.access.admin_access} private={data.page === 'restricted'} opening={{ kind: 'project', id: data.id }}
        mark={
          <PlaceMark {...(mayManage ? { label: t('places.colour.change', { name: data.name }), onPress: () => openNodeDialog({ type: 'colour', node: place }) } : {})}>
            <NodeIcon kind={place.folder ? 'folder' : 'project'} color={data.color} own={data.custom_color} />
          </PlaceMark>
        }
        title={data.name}
        lead={data.description === '' ? undefined : data.description}
        actions={
          <>
            <PermissionGate allowed={mayCreate} mode="block" reason={whyNot}>
              <Button size="sm" variant="primary" icon={<Plus aria-hidden />} onClick={() => openNodeDialog({ type: 'new-topic', parent: place })}>{t('places.project.newTopic')}</Button>
            </PermissionGate>
            <ProjectViews value={view} onValueChange={(next) => onSearch({ view: next })} />
            <PlaceDetails node={data} />
            <PlaceAnnouncements place={data} compact />
            <PlaceSettingsLink node={data} />
            <div className="flex items-center gap-2">
            <ProjectStar projectId={data.id} name={data.name} path={data.path} view={viewFromSearch(search)} readableTags={(tags.data ?? []).map((tag) => tag.id)} />
            <SubscriptionBell thread={data} label={t('notificationBell.place')} />
            <ProjectHeaderMenu project={data} place={place} />
            </div>
          </>
        }
      />

      {data.archived && (
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-bg-subtle px-4 py-3 text-base text-text">
          <Archive className="text-text-subtle" aria-hidden />
          <span className="flex-1">{t('places.project.archived')}</span>
          {mayArchive && (
            <Button size="sm" pending={unarchive.isPending} onClick={() => unarchive.mutate()}>
              {t('places.menu.unarchive')}
            </Button>
          )}
          {unarchive.error !== null && <ErrorAlert error={unarchive.error} className="w-full" />}
        </div>
      )}

      {view !== 'activity' && <div className="flex flex-col gap-3">
        <FilterBar search={<Input
            label={t('places.project.search')} hideLabel type="search" placeholder={t('places.project.searchPlaceholder')}
            value={typed} onChange={(event) => setTyped(event.target.value)} autoComplete="off" />}
          participation={<ParticipationFilter checked={search.participating === true} onCheckedChange={(on) => onSearch({ participating: on ? true : undefined })} />}>
            {view === 'list' && (
              <SegmentedControl
                size="sm"
                label={t('places.project.status')}
                value={search.status ?? 'open'}
                onValueChange={(next) => onSearch({ status: next === 'open' ? undefined : next })}
                segments={[
                  { value: 'open', name: t('choiceCatalog.topicState.open.name') },
                  { value: 'done', name: t('choiceCatalog.topicState.done.name') },
                  { value: 'all', name: t('choiceCatalog.topicState.all.name') },
                ]}
              />
            )}
            <TagsFilter tags={tags.data ?? []} chosen={search.tag ?? []} onChange={(next) => onSearch({ tag: next.length === 0 ? undefined : next })} />
            <Menu label={t('places.project.attention')}
              trigger={<Button size="sm" variant="secondary">{t('places.project.attention')}{': '}{choiceOption(t, `attention.${search.attention ?? 'any'}`).name}</Button>}
              entries={[{ type: 'radio', id: 'attention', label: t('places.project.attention'), value: search.attention ?? 'any',
                options: (['any', 'needs_attention', 'running', 'waiting_approval', 'blocked'] as const).map((value) => ({ value, ...choiceOption(t, `attention.${value}`) })),
                onValueChange: (next) => { if (next === 'any' || next === 'needs_attention' || next === 'running' || next === 'waiting_approval' || next === 'blocked') onSearch({ attention: next === 'any' ? undefined : next }); },
              }]} />
            <Menu label={t('places.project.sort')}
              trigger={<Button size="sm" variant="secondary">{t('places.project.sort')}{': '}{choiceOption(t, `sortBy.${search.sort ?? 'activity'}`).name}</Button>}
              entries={[{ type: 'radio', id: 'sort', label: t('places.project.sort'), value: search.sort ?? 'activity',
                options: (['activity', 'created', 'name'] as const).map((value) => ({ value, ...choiceOption(t, `sortBy.${value}`) })),
                onValueChange: (next) => { if (next === 'activity' || next === 'created' || next === 'name') onSearch({ sort: next === 'activity' ? undefined : next }); },
              }]} />
            <Menu label={t('everything.filters')} trigger={<Button size="sm" variant="secondary" icon={<ListFilter aria-hidden />}>{t('everything.filters')}</Button>}
              entries={[
                { type: 'check', id: 'archived', name: t('day100.views.showArchived'), checked: search.archived === true, onCheckedChange: (on) => onSearch({ archived: on ? true : undefined }) },
                { type: 'check', id: 'folders', name: t('day100.views.withFolders'), checked: search.with_folders === true, onCheckedChange: (on) => onSearch({ with_folders: on }) },
              ]} />
            {(isFiltered(search) || view === 'board' || (search.sort !== undefined && search.sort !== 'activity')) && (
              <Button size="sm" variant="ghost" icon={<ListFilter aria-hidden />} onClick={clearFilters}>
                {t('places.project.clear')}
              </Button>
            )}
        </FilterBar>
      </div>}

      {view === 'activity' ? (
        <ProjectActivity projectId={data.id} name={data.name} orgId={data.org_id} />
      ) : (
        <>
          <Topics project={data} search={search} onSearch={onSearch} />
          <FolderCards
            projectId={data.id}
            orgId={data.org_id}
            expected={data.project_count}
            filters={{ q: search.q ?? '', tags: search.tag?.length ?? 0, attention: search.attention ?? null, archived: search.archived === true, participating: search.participating === true }}
          />
        </>
      )}
      {data.readme !== null && <PlaceReadme readme={data.readme} />}
      {view !== 'activity' && <UsageSection nodeId={data.id} name={data.name} compact />}
      {data.access.administers && <OutsideOpenings placeId={data.id} />}
    </Page>
  );
}
