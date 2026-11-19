/** The Settings tab of a topic: its general settings, what new threads and new environments made for it start with, and where what happens in it is sent.
 *
 * The same sections a project's settings have that apply to a topic, in the same order; the tab opens on the first. The editor
 * is the one an org and a project have.
 */
import { TopicScopeSettings, type TopicSection } from '@/features/settings/ScopeSettings';
import type { Topic } from '@/generated/api-types';

export function TopicSettings({ topic, section, onSection }: { topic: Topic; section: TopicSection; onSection: (section: TopicSection) => void }) {
  return <TopicScopeSettings topic={topic} section={section} onSection={onSection} />;
}
