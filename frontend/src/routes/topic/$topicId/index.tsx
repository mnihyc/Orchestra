/** Topic sections and thread search carried in the page address.
 *
 * The screen is the topic's: its overview, threads, environments, files, documents, Activity and
 * settings. The tab that is open is in the address (`tab`), so that a link reproduces it.
 */
import { usePageParams, usePageSearch, usePageNavigate } from '@/features/workbench/pageContext';
import { TOPIC_TABS, useTopicView } from '@/features/navigation/topicView';
import { createFileRoute } from '@tanstack/react-router';

import { traceSearch, type TraceSearch } from '@/features/trace/params';
import { Topic, type TopicTab } from '@/features/topic/Topic';

interface TopicSearch extends TraceSearch {
  tab?: TopicTab;
  q?: string;
  /** The section of the Settings tab, when it is not the first. */
  section?: string;
  /** The setting the finder led here for. */
  setting?: string;
}

export const Route = createFileRoute('/topic/$topicId/')({
  staticData: { screen: 'topic' },
  validateSearch: (search: Record<string, unknown>): TopicSearch => {
    const query = typeof search.q === 'string' && search.q !== '' ? { q: search.q } : {};
    if (!TOPIC_TABS.includes(search.tab as TopicTab)) return query;
    const tab = search.tab as TopicTab;
    if (tab !== 'settings') return { tab, ...query };
    return {
      tab,
      ...traceSearch(search),
      ...(typeof search.section === 'string' && search.section !== '' && search.section !== 'general' ? { section: search.section } : {}),
      ...(typeof search.setting === 'string' ? { setting: search.setting } : {}),
    };
  },
  component: TopicRoute,
});

function TopicRoute() {
  const { topicId } = usePageParams(Route);
  const { tab, section, setting, q } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const [picked, choose] = useTopicView(tab);
  // A setting locator opens Defaults when no section is addressed.
  const shown = section ?? (setting === undefined ? 'general' : 'defaults');
  return (
    <Topic
      topicId={topicId}
      tab={picked}
      q={q}
      onThreadQuery={(query) => void navigate({ search: (before) => ({ ...before, q: query === '' ? undefined : query }), replace: true })}
      onTab={(next) => { choose(next); void navigate({ search: { tab: next }, replace: true }); }}
      section={shown}
      onSection={(next) => void navigate({ search: { tab: 'settings', ...(next === 'general' ? {} : { section: next }) }, replace: true })}
    />
  );
}
