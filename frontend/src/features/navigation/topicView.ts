/** A topic remembers its content view; Settings remains an explicit, separately addressed destination. */
import type { TopicTab } from '@/features/topic/Topic';

export const TOPIC_TABS: readonly TopicTab[] = ['overview', 'threads', 'environments', 'documents', 'agents', 'activity', 'settings'];

export function useTopicView(explicit: TopicTab | undefined): [TopicTab, (next: TopicTab) => void] {
  // The addressed page adapter resumes this topic's content; Settings has its own destination.
  return [explicit ?? 'overview', () => undefined];
}
