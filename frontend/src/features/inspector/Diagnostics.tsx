/** Raw protocol frames and gated agent-terminal rescue.
 * Segment frames remain separate from the conversation, with a bounded live read and older pages on demand.
 */
import { useListChoice } from '@/lib/listState';
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { Bug } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { viewedRawFramesQuery } from '@/api/threadHistory';
import { segmentsQuery } from '@/api/threads';
import { agentQuery } from '@/api/resources';
import type { RawDirection, Segment, Thread } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { useThreadView } from '@/realtime/stores/thread';
import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { JsonTree } from '@/ui/JsonTree';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { Select } from '@/ui/Select';
import { Combobox } from '@/ui/Combobox';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScrollArea } from '@/ui/ScrollArea';
import { Tabs } from '@/ui/Tabs';
import { AgentTerminal } from './AgentTerminal';
import { ThreadPanel } from './ThreadPanel';

function Frames({ threadId, segment }: { threadId: string; segment: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  useThreadReadScope(read);
  const view = useThreadView(threadId);
  const [direction, setDirection] = useListChoice<RawDirection | ''>(`diagnostics.${threadId}`, 'direction', '');
  const search = useCollectionSearch(`raw:${threadId}:${segment}`);
  const query = useInfiniteQuery(viewedRawFramesQuery(read, segment, { ...(search.q === '' ? {} : { q: search.q }), ...(direction === '' ? {} : { direction }) }));
  if (view.error !== null && view.read?.ready !== true) return <ErrorState compact error={view.error} onRetry={() => reloadThread(read)} />;
  const unique = new Map((query.data?.pages ?? []).flatMap((page) => page.events).map((event) => [event.n, event]));
  const events = [...unique.values()].sort((one, other) => one.n - other.n);
  return <div className="flex min-w-0 flex-col">
    <div className="flex flex-wrap gap-2 p-2"><Select<RawDirection | ''> label={t('threadViews.diagnostics.direction')} value={direction} onValueChange={setDirection} options={[{ value: '', ...choiceOption(t, 'rawDirection.all') }, { value: 'to_harness', ...choiceOption(t, 'rawDirection.to_harness') }, { value: 'from_harness', ...choiceOption(t, 'rawDirection.from_harness') }]} /><CollectionSearch name={t('threadViews.diagnostics.search')} {...search} /></div>
    <div className="p-2">{query.data === undefined && (query.error === null ? <SkeletonRows read={query} rows={4} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />)}<Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.error} onMore={() => void query.fetchNextPage()} />
      {query.data !== undefined && events.length === 0 && <EmptyState compact icon={<Bug aria-hidden />} title={t(search.q === '' && direction === '' ? 'threadViews.diagnostics.empty' : 'ui.collection.none')} />}
      <ol aria-label={t('threadViews.diagnostics.frames')}>{events.map((event) => <li key={event.n} className="border-b border-border py-2"><details><summary className="cursor-pointer text-sm">{event.n} · {t(event.direction === 'to_harness' ? 'threadViews.diagnostics.to' : 'threadViews.diagnostics.from')} · <RelativeTime at={event.at} /></summary><JsonTree value={event.payload} search={search.text} label={t('threadViews.diagnostics.frame', { n: event.n })} /></details></li>)}</ol>
    </div>
  </div>;
}

function Diagnostics({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const view = useThreadView(thread.id);
  const segments = view.segments;
  const segmentSearch = useCollectionSearch(`segments:${thread.id}`);
  const search = segmentSearch.text, setSearch = segmentSearch.onText;
  const owner = JSON.stringify([segmentSearch.scope, view.read?.projection?.viewed_continuation_id, view.read?.projection?.projection_revision]);
  const [chosen, setChosen] = useState<{owner:string; value:Segment | null}>({owner,value:null});
  const [choosing, setChoosing] = useState(false);
  const read = useThreadReadRef(thread.id);
  const segmentOptions = segmentsQuery(read, segmentSearch.q);
  const history = useInfiniteQuery({ ...segmentOptions, enabled: choosing && segmentOptions.enabled });
  const [tab, setTab] = useState<'frames' | 'terminal'>('frames');
  const agentId = thread.binding?.agent.id ?? '';
  const agent = useQuery({ ...agentQuery(agentId), enabled: agentId !== '', retry: false });
  const descriptor = agent.data?.descriptor as { capabilities?: { agent_terminal?: string } } | undefined;
  const terminal = descriptor?.capabilities?.agent_terminal;
  const historical = history.data?.pages.flatMap((page) => page.segments) ?? [];
  const allSegments = [...new Map([...segments, ...historical].map((one) => [one.id, one])).values()].sort((a, b) => a.ordinal - b.ordinal);
  const segment = chosen.owner === owner ? chosen.value ?? segments.at(-1) : segments.at(-1);
  const nameOf = (one: { ordinal: number; agent: { name: string } | null; provider: { name: string } | null }) => `${one.ordinal} · ${one.agent?.name ?? t('threadViews.diagnostics.imported')} · ${one.provider?.name ?? t('threadViews.diagnostics.imported')}`;
  const choices = (segmentSearch.q === '' ? allSegments : historical).toReversed().map((one) => ({ value: one.id, label: nameOf(one), text: nameOf(one) }));
  const frames = <ScrollArea className="focus-inset h-full" role="region" tabIndex={0} aria-label={t('threadViews.diagnostics.frames')}><div className="p-2">
    <Combobox label={t('threadViews.diagnostics.segment')} query={search} onQueryChange={setSearch} displayValue={segment === undefined ? '' : nameOf(segment)} onOpenChange={setChoosing} loading={history.isFetching} groups={[{ id: 'segments', label: t('threadViews.diagnostics.segment'), options: choices }]} onSelect={(one) => { const value = allSegments.find((entry) => entry.id === one.value); if (value !== undefined) setChosen({owner,value}); setSearch(''); }} empty={t('threadViews.diagnostics.noMatchingSegments')} />
    {history.error !== null && !history.isFetchNextPageError && <ErrorState compact error={history.error} onRetry={() => void history.refetch()} />}
    <Pagination hasMore={history.hasNextPage} loading={history.isFetchingNextPage} error={history.isFetchNextPageError ? history.error : null} onMore={() => void history.fetchNextPage()} />
  </div>{segment === undefined ? <EmptyState compact icon={<Bug aria-hidden />} title={t('threadViews.diagnostics.noSegments')} /> : <Frames key={segment.id} threadId={thread.id} segment={segment.id} />}</ScrollArea>;
  return <div className="flex h-full min-h-0 flex-col" data-testid="thread-diagnostics">{terminal === 'view' || terminal === 'interactive' ? <Tabs label={t('threadViews.diagnostics.title')} value={tab} onValueChange={setTab} tabs={[{ value: 'frames', label: t('threadViews.diagnostics.frames'), panel: frames }, { value: 'terminal', label: t('threadViews.diagnostics.terminal'), panel: <AgentTerminal thread={thread} mayControl={terminal === 'interactive'} /> }]} className="flex min-h-0 flex-1 flex-col" panelClassName="min-h-0 flex-1" /> : frames}</div>;
}

export function DiagnosticsView(_props: ViewProps) { return <ThreadPanel manage>{(thread) => <Diagnostics key={thread.id} thread={thread} />}</ThreadPanel>; }
