/** What Orch gave the thread's agent, exactly as it was sent: what each session opened with, and each message with
 * what went before it, block by block, the newest first.
 *
 * Whoever reads the thread reads this, without managing it. A block's words are the very text the agent received;
 * the conversation keeps the person's own words where they were said, and nothing here changes it. A message's menu
 * opens this view narrowed to its turn ("What the agent received"). Above them, the instructions this agent reads,
 * source by source (`InstructionSources`).
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { ScrollText } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { modelInputText, modelInputsQuery } from '@/api/modelInputs';
import type { ModelInput, Thread } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { Badge, type BadgeTone } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScrollArea } from '@/ui/ScrollArea';
import { copyText } from '@/ui/clipboard';
import { ErrorAlert } from '@/ui/ErrorState';
import { GivenBlock } from './GivenBlock';
import { clearGiven } from './givenFocus';
import { InstructionSources } from './InstructionSources';
import { ThreadPanel } from './ThreadPanel';
import { givenPurposeWord, givenStateWord } from './words';

/** A record that was taken is the ordinary case and stays quiet; one refused, or of which nobody knows whether it
 * arrived, is marked. */
const STATE_TONES: Record<ModelInput['state'], BadgeTone> = {
  prepared: 'neutral',
  sent: 'neutral',
  failed: 'danger',
  uncertain: 'warning',
};

/** The inline portion; full blocks are fetched only when read or copied. */
const wholeText = (record: ModelInput): string => record.blocks.map((block) => block.text.inline).join('\n\n');

export function GivenRecord({ record, search, first }: { record: ModelInput; search: string; first: boolean }) {
  const { t } = useTranslation();
  const [copyError, setCopyError] = useState<unknown>(null);
  const copyAll = async () => { try { await copyText((await Promise.all(record.blocks.map((block) => modelInputText(record, block)))).join('\n\n')); } catch (failure) { setCopyError(failure); } };
  const whole = wholeText(record);
  const blocks = search === '' ? record.blocks : record.blocks.filter((block) => block.text.inline.toLocaleLowerCase().includes(search));
  return <li className="border-b border-border py-2" data-testid="given-record">
    <details open={first || search !== ''}>
      <summary className="flex cursor-pointer flex-wrap items-center gap-2 text-sm">
        <span className="font-medium">{givenPurposeWord(t, record.purpose)}</span>
        <Badge tone={STATE_TONES[record.state]}>{givenStateWord(t, record.state)}</Badge>
        <RelativeTime at={record.created_at} className="text-text-muted" />
        <span className="text-text-muted">{t(record.blocks.some((block) => block.text.truncated) ? 'threadViews.given.visibleCharacters' : 'threadViews.given.characters', { count: Array.from(whole).length })}</span>
      </summary>
      <div className="mt-2 flex justify-end"><Button size="sm" variant="ghost" onClick={() => void copyAll()}>{t('threadViews.given.copyAll')}</Button></div>
      {copyError !== null && <ErrorAlert error={copyError} />}
      <ol className="mt-1 space-y-2">
        {blocks.map((block, index) => <GivenBlock key={`${block.key}-${String(index)}`} record={record} block={block} />)}
      </ol>
    </details>
  </li>;
}

function Given({ thread, tab }: { thread: Thread; tab: ViewProps['tab'] }) {
  const { t } = useTranslation();
  const turn = tab.of?.givenThread === thread.id ? tab.of.givenTurn ?? null : null;
  const record = tab.of?.givenThread === thread.id ? tab.of.givenRecord ?? null : null;
  const query = useInfiniteQuery(modelInputsQuery(thread.id, turn, record));
  const [search, setSearch] = useState('');
  if (query.data === undefined) return query.error === null ? <SkeletonRows rows={4} /> : <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />;
  const unique = new Map(query.data.pages.flatMap((page) => page.items).map((record) => [record.id, record]));
  const wanted = search.trim().toLocaleLowerCase();
  const records = [...unique.values()]
    .sort((one, other) => other.seq_created - one.seq_created)
    .filter((record) => wanted === '' || record.blocks.some((block) => block.text.inline.toLocaleLowerCase().includes(wanted)));
  return <ScrollArea className="focus-inset h-full" data-testid="thread-given" role="region" tabIndex={0} aria-label={t('threadViews.given.title')}>
    <InstructionSources threadId={thread.id} />
    <div className="flex flex-wrap items-end gap-2 border-b border-border p-2">
      <Input label={t('threadViews.given.search')} value={search} onChange={(event) => setSearch(event.target.value)} />
      {(turn !== null || record !== null) && <Button size="sm" variant="ghost" onClick={() => clearGiven(tab.id)}>{t('threadViews.given.showAll')}</Button>}
    </div>
    {(turn !== null || record !== null) && <p className="px-3 pt-2 text-sm text-text-muted"><span className="font-medium">{thread.name}</span> · {t('threadViews.given.narrowed')}</p>}
    <div className="p-2">
      {records.length === 0 && <EmptyState compact icon={<ScrollText aria-hidden />} title={t(unique.size === 0 ? 'threadViews.given.empty' : 'threadViews.given.noMatches')} />}
      <ol aria-label={t('threadViews.given.title')}>{records.map((record, index) => <GivenRecord key={record.id} record={record} search={wanted} first={index === 0} />)}</ol>
      <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.error} onMore={() => void query.fetchNextPage()} />
    </div>
  </ScrollArea>;
}

export function GivenView({ tab }: ViewProps) {
  const threadId = tab.scope?.mode === 'fixed' && tab.scope.target.kind === 'thread' ? tab.scope.target.id : tab.of?.thread;
  return <ThreadPanel threadId={threadId}>{(thread) => <Given key={thread.id} thread={thread} tab={tab} />}</ThreadPanel>;
}
