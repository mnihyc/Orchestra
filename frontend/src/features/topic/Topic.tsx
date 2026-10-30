/** Topic header and tab selection.
 *
 * A topic is one piece of work. Its header has its name and description, its status, its tags, its star
 * and its bell, and its menu; under it are its tabs: the overview, its threads, its environments, its
 * files and documents, its Activity and its settings. The status and the tags are set by whoever may
 * start work in the topic; the bell and the star are each person's own and show at once.
 *
 * Which tab is open is in the address, so that a link reproduces it. A tab whose part is not built
 * yet says so in words and says what it will show.
 */
import { useContentFocused } from '@/features/workbench/instance';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Archive, Diamond, Settings } from 'lucide-react';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { topicQuery, unarchiveNode } from '@/api/nodes';
import { PlaceReadme } from '@/features/documents/Readme';
import { StarButton } from '@/features/navigation/favorites';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { placeOfNode } from '@/features/navigation/places';
import { TopicActivity } from '@/features/topic/Activity';
import { TopicEnvironments } from '@/features/topic/Environments';
import { TopicOverview } from '@/features/topic/Overview';
import type { TopicSection } from '@/features/settings/ScopeSettings';
import { TopicSettings } from '@/features/topic/Settings';
import { TagPicker } from '@/features/topic/TagPicker';
import { PlaceAnnouncements } from '@/features/announcements/Composer';
import { TopicStatusChip } from '@/features/topic/Status';
import { TopicHeaderMenu } from '@/features/topic/TopicMenu';
import { TopicThreads, threadsOfTopic } from '@/features/topic/Threads';
import { TopicDocuments } from '@/features/topic/Documents';
import { TopicAgents } from '@/features/topic/Agents';
import { SubscriptionBell } from '@/features/thread/SubscriptionBell';
import { holds } from '@/lib/permissions';
import { useOrgTreeFeed } from '@/realtime/live';
import { useOrgTree } from '@/realtime/stores/tree';
import { selectOrg } from '@/shell/org';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { NodeFailure } from '@/features/access/NoAccess';
import { ErrorAlert } from '@/ui/ErrorState';
import { NodeIcon } from '@/ui/NodeIcon';
import { Skeleton, SkeletonGroup, SkeletonRows } from '@/ui/Skeleton';
import { Tabs } from '@/ui/Tabs';
import { toast } from '@/ui/Toast';

export type TopicTab = 'overview' | 'threads' | 'environments' | 'documents' | 'agents' | 'activity' | 'settings';

function TopicSkeleton() {
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
        <SkeletonRows rows={4} />
      </SkeletonGroup>
    </Page>
  );
}

export interface TopicProps {
  topicId: string;
  tab: TopicTab;
  onTab: (tab: TopicTab) => void;
  /** The section of the Settings tab in view. */
  section: TopicSection;
  onSection: (section: TopicSection) => void;
}

export function Topic({ topicId, tab, onTab, section, onSection }: TopicProps) {
  const { t } = useTranslation();
  const focusedContent = useContentFocused();
  const userId = useStore(client.session, (state) => state.userId);
  const query = useQuery(topicQuery(topicId));
  const topic = query.data;
  const tree = useOrgTree(topic?.org_id ?? null);

  useEffect(() => {
    if (focusedContent && topic !== undefined) selectOrg(userId, topic.org_id);
  }, [topic, userId, focusedContent]);
  useOrgTreeFeed(tab === 'settings' ? null : topic?.org_id ?? null);
  useScreenHeading({ title: tab === 'settings' ? t('screens.sections.settings') : topic?.name ?? t('screens.topic.title'), tabTitle: tab === 'settings' ? t('screens.sections.settings') : undefined, icon: tab === 'settings' ? Settings : Diamond, nodeId: topic?.id, path: topic?.path, leaf: tab === 'settings', views: { ready: topic !== undefined } });

  const unarchive = useMutation({
    mutationFn: () => unarchiveNode(topicId),
    onSuccess: (node) => {
      toast({ tone: 'success', title: t('places.toast.unarchived', { name: node.name }) });
    },
  });

  if (query.isPending) return <TopicSkeleton />;
  if (query.isError) {
    // A screen the server refused shows what is missing and whom to ask, and the way on, in place of its content.
    return <NodeFailure className="h-full" error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  }
  const data = query.data;
  const place = placeOfNode(data);
  const mayInteract = holds(data.access, 'topic.interact');
  const mayManage = holds(data.access, 'topic.manage');
  const mayArchive = mayManage || holds(data.access, 'topic.archive');
  const threadCount = threadsOfTopic(tree.nodes, data.id).length;

  if (tab === 'settings') return <TopicSettings topic={data} section={section} onSection={onSection} />;

  return (
    <Page>
      <PageHeader
        adminAccess={data.access.admin_access} private={data.page === 'restricted'} opening={{ kind: 'topic', id: data.id }}
        mark={
          <PlaceMark {...(mayManage ? { label: t('places.colour.change', { name: data.name }), onPress: () => openNodeDialog({ type: 'colour', node: place }) } : {})}>
            <NodeIcon kind="topic" color={data.color} own={data.custom_color} done={data.status === 'done'} />
          </PlaceMark>
        }
        title={data.name}
        lead={
          data.description !== '' ? (
            data.description
          ) : mayManage ? (
            <button type="button" className="rounded-md text-text-subtle hover:text-text hover:underline" onClick={() => openNodeDialog({ type: 'rename', node: place })}>
              {t('places.project.addDescription')}
            </button>
          ) : undefined
        }
        facts={[
          <TopicStatusChip key="status" topic={data} allowed={mayInteract && !data.archived} />,
          <TagPicker key="tags" place={data} allowed={mayInteract && !data.archived} />,
        ]}
        actions={
          <>
            <PlaceAnnouncements place={data} />
            <StarButton place={place} />
            <SubscriptionBell thread={data} label={t('places.topic.bell')} />
            <TopicHeaderMenu topic={data} />
          </>
        }
      />

      {data.archived && (
        <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-bg-subtle px-4 py-3 text-base text-text">
          <Archive className="text-text-subtle" aria-hidden />
          <span className="flex-1">{t('places.topic.archived')}</span>
          {mayArchive && (
            <Button size="sm" pending={unarchive.isPending} onClick={() => unarchive.mutate()}>
              {t('places.menu.unarchive')}
            </Button>
          )}
          {unarchive.error !== null && <ErrorAlert error={unarchive.error} className="w-full" />}
        </div>
      )}

      {data.readme !== null && <PlaceReadme readme={data.readme} />}

      <Tabs
        label={t('places.topic.sections')}
        value={tab}
        onValueChange={onTab}
        tabs={[
          { value: 'overview', label: t('places.topic.tabs.overview'), panel: <TopicOverview topic={data} place={place} /> },
          {
            value: 'threads',
            label: t('places.topic.tabs.threads'),
            count: tree.loaded ? threadCount : undefined,
            panel: <TopicThreads orgId={data.org_id} topicId={data.id} topic={place} allowed={mayInteract} owner={data.owner} />,
          },
          { value: 'environments', label: t('places.topic.tabs.environments'), count: data.environments.length, panel: <TopicEnvironments topic={data} /> },
          { value: 'documents', label: t('places.topic.tabs.documents'), panel: <TopicDocuments topic={data} /> },
          { value: 'agents', label: t('people.agentChoices'), panel: <TopicAgents topic={data} /> },
          { value: 'activity', label: t('places.topic.tabs.activity'), panel: <TopicActivity topicId={data.id} /> },
          { value: 'settings', label: t('places.topic.tabs.settings'), panel: <TopicSettings topic={data} section={section} onSection={onSection} /> },
        ]}
        panelClassName="pt-6"
      />
    </Page>
  );
}
