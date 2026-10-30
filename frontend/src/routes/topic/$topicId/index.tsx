/** Topic tabs, including settings. Loader, permission handling and search schema are pending.
 *
 * The screen is the topic's: its overview, threads, environments, files, documents, Activity and
 * settings. The tab that is open is in the address (`tab`), so that a link reproduces it.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { TOPIC_TABS, useTopicView } from '@/features/navigation/topicView';
import { createFileRoute } from '@tanstack/react-router';

import { traceSearch, type TraceSearch } from '@/features/trace/params';
import { isTopicSection, type TopicSection } from '@/features/settings/sections';
import { Topic, type TopicTab } from '@/features/topic/Topic';

interface TopicSearch extends TraceSearch {
  tab?: TopicTab;
  /** The section of the Settings tab, when it is not the first. */
  section?: TopicSection;
  /** The setting the finder led here for. */
  setting?: string;
}

export const Route = createFileRoute('/topic/$topicId/')({
  staticData: { screen: 'topic' },
  validateSearch: (search: Record<string, unknown>): TopicSearch => {
    if (!TOPIC_TABS.includes(search.tab as TopicTab)) return {};
    const tab = search.tab as TopicTab;
    if (tab !== 'settings') return { tab };
    return {
      tab,
      ...traceSearch(search),
      ...(isTopicSection(search.section) && search.section !== 'general' ? { section: search.section } : {}),
      ...(typeof search.setting === 'string' ? { setting: search.setting } : {}),
    };
  },
  component: TopicRoute,
});

function TopicRoute() {
  const { topicId } = usePageParams(Route);
  const { tab, section, setting } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const [picked, choose] = useTopicView(tab);
  // A setting is at home among the policies.
  const shown: TopicSection = section ?? (setting === undefined ? 'general' : 'policies');
  return (
    <Topic
      topicId={topicId}
      tab={picked}
      onTab={(next) => { choose(next); void navigate({ search: { tab: next }, replace: true }); }}
      section={shown}
      onSection={(next) => void navigate({ search: { tab: 'settings', ...(next === 'general' ? {} : { section: next }) }, replace: true })}
    />
  );
}
