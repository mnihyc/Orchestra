/** Name-only readers follow renames and visibility, without subscribing to unrelated activity. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { type ReactNode } from 'react';
import { useKnownPlaces, usePlaceLabel } from '@/features/navigation/placeLabels';
import { useSettingsTabTitle } from '@/features/workbench/settingsTabTitle';
import { foldName } from '@/lib/placeLabel';
import { treeWriter, useTreeStore, useNodePath } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';

afterEach(() => { cleanup(); treeWriter.clear(); });

it.each(['en', 'zh-CN'] as const)('keeps activity cheap and immediately follows names, ancestry and lost visibility in %s', (locale) => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  treeWriter.reset(ACME.org, acmeTree(), 1);
  const rendered = vi.fn();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  const { result } = renderHook(() => {
    rendered();
    return { known: useKnownPlaces(), label: usePlaceLabel(), section: useSettingsTabTitle(locale === 'en' ? 'General' : '常规', undefined) };
  }, { wrapper });
  const initial = result.current.known, draws = rendered.mock.calls.length;
  const thread = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.mainThread);
  if (thread === undefined) throw new Error('Expected the admitted thread fixture.');
  const activity = thread.thread;
  if (activity === null) throw new Error('Expected the admitted thread activity.');
  act(() => treeWriter.apply(ACME.org, [{ ...thread, tree_seq: thread.tree_seq + 1, thread: { ...activity, activity_seq: activity.activity_seq + 1 } }], 2));
  expect(result.current.section).toBe(locale === 'en' ? 'General' : '常规');
  expect(result.current.known).toBe(initial);
  expect(rendered).toHaveBeenCalledTimes(draws);

  const project = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.web);
  if (project === undefined) throw new Error('Expected the admitted project fixture.');
  const name = locale === 'en' ? 'Web services' : '网页服务';
  act(() => treeWriter.apply(ACME.org, [{ ...project, tree_seq: project.tree_seq + 1, name }], 3));
  expect(result.current.known.get(foldName(name))?.some((place) => place.id === project.id)).toBe(true);
  expect(result.current.known).not.toBe(initial);

  const topic = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.fixLogin);
  if (topic === undefined) throw new Error('Expected the admitted topic fixture.');
  act(() => treeWriter.apply(ACME.org, [{ ...topic, tree_seq: topic.tree_seq + 1, parent_id: ACME.infra }], 4));
  expect(result.current.known.get(foldName(topic.name))?.find((place) => place.id === topic.id)?.above.some((place) => place.id === ACME.infra)).toBe(true);
  expect(result.current.label({ id: project.id, name, redacted: true })).toBeNull();

  const admitted = [...(useTreeStore.getState().byOrg.get(ACME.org)?.nodes.values() ?? [])];
  act(() => treeWriter.reset(ACME.org, admitted.filter((node) => node.id !== topic.id), 5));
  expect(result.current.known.get(foldName(topic.name))?.some((place) => place.id === topic.id) ?? false).toBe(false);
  act(() => treeWriter.clear());
  expect(result.current.known.size).toBe(0);
  client.clear();
});


it('keeps every native row of a selected path current without redrawing it for unrelated activity', () => {
  treeWriter.reset(ACME.org, acmeTree(), 1);
  const rendered = vi.fn();
  const { result, rerender } = renderHook(({ id }) => { rendered(); return useNodePath(id); }, { initialProps: { id: ACME.web } });
  const initial = result.current, draws = rendered.mock.calls.length;
  const thread = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.mainThread);
  if (thread === undefined) throw new Error('Expected the admitted thread fixture.');
  const activity = thread.thread;
  if (activity === null) throw new Error('Expected the admitted thread activity.');
  act(() => treeWriter.apply(ACME.org, [{ ...thread, tree_seq: thread.tree_seq + 1, thread: { ...activity, activity_seq: activity.activity_seq + 1 } }], 2));
  expect(result.current).toBe(initial);
  expect(rendered).toHaveBeenCalledTimes(draws);
  rerender({ id: ACME.mainThread });
  expect(result.current.path.at(-1)?.thread?.activity_seq).toBe(activity.activity_seq + 1);
  const current = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.mainThread);
  if (current === undefined) throw new Error('Expected the current admitted thread.');
  const live = current.thread;
  if (live === null) throw new Error('Expected the current admitted activity.');
  act(() => treeWriter.apply(ACME.org, [{ ...current, tree_seq: current.tree_seq + 1, thread: { ...live, activity_seq: live.activity_seq + 1 } }], 3));
  expect(result.current.path.at(-1)?.thread?.activity_seq).toBe(live.activity_seq + 1);
  const root = useTreeStore.getState().byOrg.get(ACME.org)?.nodes.get(ACME.org);
  if (root === undefined) throw new Error('Expected the admitted ancestor.');
  act(() => treeWriter.apply(ACME.org, [{ ...root, tree_seq: root.tree_seq + 1, name: 'Current root name' }], 4));
  expect(result.current.path[0]?.name).toBe('Current root name');
  act(() => treeWriter.clear());
  expect(result.current).toEqual({ orgId: null, path: [] });
});
