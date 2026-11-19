/** Following a topic never resumes its Settings pane, and visiting Settings preserves its content preference. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { client } from '@/api/client';
import { settingsNavigation } from '@/features/settings/navigation';
import { usePageListSearch, storedListState } from '@/lib/listState';
import { useTopicView } from '@/features/navigation/topicView';
import type { TopicTab } from '@/features/topic/Topic';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';

const session = client.session.getState();
afterEach(() => { cleanup(); client.session.setState(session); });

it('reads an older saved Settings choice as Overview without rewriting the stored choice', () => {
  client.session.setState({ userId: 'topic-reader' });
  writeViewChoice('topic-reader', 'topic', 'tab', 'settings');
  const view = renderHook(() => useTopicView(undefined));
  expect(view.result.current[0]).toBe('overview');
  expect(settingsNavigation('/topic/one', {})).toBeNull();
  expect(readViewChoice('topic-reader', 'topic', 'tab', 'overview')).toBe('settings');
});

it('keeps the last content tab across explicit Settings and follows an explicit Overview breadcrumb', () => {
  client.session.setState({ userId: 'topic-reader' });
  const validate=(search:Record<string,unknown>)=>typeof search.tab==='string'?{tab:search.tab}:{};
  const previous=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/one',{tab:'activity'},validate));previous.unmount();
  const initial: { tab: TopicTab | undefined } = { tab: undefined };
  const view = renderHook(({ tab }: typeof initial) => {
    const search=usePageListSearch('/topic/$topicId/','/topic/one',tab===undefined?{}:{tab},validate);
    return useTopicView(search.tab as TopicTab|undefined);
  }, { initialProps: initial });
  expect(view.result.current[0]).toBe('activity');
  act(() => { view.result.current[1]('settings'); });
  expect(storedListState(readViewChoice('topic-reader','page./topic/one','state','{}'))).toMatchObject({tab:'activity'});
  view.rerender({ tab: 'settings' });
  expect(view.result.current[0]).toBe('settings');
  expect(settingsNavigation('/topic/one', { tab: 'settings' })).toEqual({ kind: 'topic', id: 'one', current: 'general' });
  view.rerender({ tab: undefined });
  expect(view.result.current[0]).toBe('activity');
  view.rerender({ tab: 'overview' });
  expect(view.result.current[0]).toBe('overview');
  const other=renderHook(()=>usePageListSearch('/topic/$topicId/','/topic/other',{},validate));expect(other.result.current).not.toHaveProperty('tab');
});
