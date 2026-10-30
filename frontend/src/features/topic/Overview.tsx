/** Live thread cards tagged and grouped by environment, announcements, environments and jobs.
 *
 * The topic at a glance: its threads as cards, how each stands, and beside them what the topic is: its
 * description, who owns it, when it was made and where its environments stand. Live: a thread that starts,
 * waits for an approval or ends changes its card as the org's tree says so. The announcements and the
 * running jobs of a topic come with the thread data this screen does not read yet.
 */
import { OutsideOpenings } from '@/features/ports/Outside';
import { MessagesSquare } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { PlaceChain, placesAbove } from '@/features/navigation/PlaceChain';
import type { PlaceRef } from '@/features/navigation/places';
import { canQuickStart, QuickStart } from '@/features/start/QuickStart';
import { NewThreadButton, threadsOfTopic } from '@/features/topic/Threads';
import { ThreadCard } from '@/features/topic/ThreadCard';
import { UsageSection } from '@/features/usage/Board';
import type { Topic } from '@/generated/api-types';
import { useOrgTree } from '@/realtime/stores/tree';
import { boxClasses, Section } from '@/shell/Page';
import { EmptyState } from '@/ui/EmptyState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

export function TopicOverview({ topic, place }: { topic: Topic; place: PlaceRef }) {
  const { t } = useTranslation();
  const tree = useOrgTree(topic.org_id);
  const threads = useMemo(() => threadsOfTopic(tree.nodes, topic.id), [tree.nodes, topic.id]);
  const allowed = topic.access.permissions.includes('topic.interact');
  // The project a topic lies in is the one it lies in directly, a folder or not, and the folders on the way are its place too.
  const places = placesAbove(topic.path);

  return (
    <div className="flex flex-col gap-6">
      <QuickStart node={topic} />
      <Section id="topic-threads" title={t('places.topic.threads')} action={canQuickStart(topic) ? undefined : <NewThreadButton topic={place} allowed={allowed && !topic.archived} owner={topic.owner} />}>
        {!tree.loaded && tree.error === null ? (
          <SkeletonRows rows={2} />
        ) : threads.length === 0 ? (
          <EmptyState
            className={boxClasses}
            icon={<MessagesSquare aria-hidden />}
            title={t('places.topic.noThreads')}
            description={t('places.topic.noThreadsAbout')}
          />
        ) : (
          <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2">
            {threads.map((thread) => (
              <ThreadCard key={thread.id} thread={thread} />
            ))}
          </ul>
        )}
      </Section>

      <UsageSection nodeId={topic.id} name={topic.name} />
      <div className="grid grid-cols-1 gap-6 sm:grid-cols-2">
        <Section id="topic-about" title={t('places.topic.about')}>
          <dl className={`${boxClasses} grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 p-4 text-base`}>
            <dt className="text-text-muted">{t('places.topic.owner')}</dt>
            <dd>
              <UserChip user={topic.owner} size={24} thing={{ kind: 'topic', id: topic.id }} />
            </dd>
            <dt className="text-text-muted">{t('places.topic.made')}</dt>
            <dd>
              <RelativeTime at={topic.created_at} />
            </dd>
            {places.length > 0 && (
              <>
                <dt className="text-text-muted">{t('places.topic.in')}</dt>
                <dd className="min-w-0">
                  <PlaceChain places={places} />
                </dd>
              </>
            )}
          </dl>
        </Section>
        <Section id="topic-environments" title={t('places.topic.environments')}>
          <div className={`${boxClasses} flex flex-col gap-2 p-4`}>
            {topic.environments.length === 0 ? (
              <EmptyState compact title={t('places.environments.empty')} />
            ) : (
              topic.environments.map((environment) => (
                <div key={environment.id} className="flex items-center justify-between gap-3">
                  <span className="truncate text-base text-text">{environment.name}</span>
                  <EnvironmentDot status={environment.status} showLabel />
                </div>
              ))
            )}
          </div>
        </Section>
      </div>
      {topic.access.administers && <OutsideOpenings placeId={topic.id} />}
    </div>
  );
}
