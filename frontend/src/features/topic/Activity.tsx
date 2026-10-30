/** A topic's Activity: what its threads did, and the lines of its own history and of the management of its threads.
 *
 * Opened from the topic's Activity tab: its history as a feed, newest first: the topic's own name,
 * status, tags, settings and page changed, each thread made, forked, archived or deleted, and what its
 * threads did. Every line names who did it, and a line about a thread opens that thread. Chips keep to
 * the management of the topic or to the work.
 */
import { ActivityFeed } from '@/features/org/Activity';

export function TopicActivity({ topicId }: { topicId: string }) {
  return <ActivityFeed nodeId={topicId} chips />;
}
