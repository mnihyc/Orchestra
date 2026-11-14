/** The agents of a thread: "N agents" with a dot in the header, and the panel it opens: Main first, then the sub-agents that are active, then the ones that are done. */
import { Bot, MessageSquareText } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { type AgentEntry, type AgentHistory, agentGroup, agentsSummary } from '@/features/thread/agents';
import type { BackgroundWork } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatTokens } from '@/lib/format';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Input } from '@/ui/Input';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';

/** The groups of the panel, in the order they are drawn. */
const GROUPS = ['active', 'done'] as const;

/** What stands on the header's button: the mark, how many, and a dot where one needs a person, one works or one failed. */
export function AgentsFace({ entries, count: total }: { entries: readonly AgentEntry[]; count?: number | null }) {
  const { t } = useTranslation();
  const { count, dot } = agentsSummary(entries);
  return (
    <>
      <Bot aria-hidden />
      <span>{total === null ? t('thread.agents.title') : t('thread.agents.count', { count: total ?? count })}</span>
      {dot !== null && <StatusDot tone={dot === 'waiting' || dot === 'lost' ? 'warning' : dot === 'failed' ? 'danger' : 'accent'} label={dot === 'lost' ? t('backgroundWake.states.lost') : t(`thread.agents.dot.${dot}`)} />}
    </>
  );
}

/** One entry: name, where it stands, how long, what it wrote where that was recorded; and one line of what it does or came to. */
function Entry({ entry, now, current, onOpen, onEnd }: { entry: AgentEntry; now: number; current: boolean; onOpen: (id: string) => void; onEnd: ((task: BackgroundWork) => void) | null }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const task = entry.task;
  const end = onEnd;
  const took = entry.state === 'running' && entry.startedAt !== null ? Math.max(0, now - Date.parse(entry.startedAt)) : entry.durationMs;
  const state = entry.state === 'recorded' ? t('timeline.step.recordedRunning') : entry.state === 'lost' ? t('backgroundWake.states.lost') : t(`timeline.step.subagentState.${entry.state}`);
  const label = [entry.name, state, entry.waiting ? t('thread.agents.dot.waiting') : null,
    took !== null && took >= 1_000 ? formatSpan(took, format) : null,
    entry.tokensOut === null ? null : t('thread.agents.tokens', { value: formatTokens(entry.tokensOut, format.locale) }), entry.line?.text ?? null].filter((part) => part !== null).join(' · ');
  return (
    <li className="flex items-start gap-1" data-agent={entry.id}>
      <button type="button" aria-label={label} aria-current={current || undefined} onClick={() => onOpen(entry.id)}
        className="focus-inset flex min-w-0 flex-1 flex-col rounded-md px-2 py-1 text-left hover:bg-bg-muted aria-[current=true]:bg-bg-muted">
        <span className="flex min-w-0 items-center gap-2 text-sm">
          <Bot className="shrink-0 text-text-muted" aria-hidden />
          <span className="min-w-0 truncate font-medium text-text">{entry.name}</span>
          <span className="shrink-0 text-text-muted">{state}</span>
          {entry.waiting && <StatusDot tone="warning" label={t('thread.agents.dot.waiting')} />}
          {took !== null && took >= 1_000 && <span className="shrink-0 text-xs tabular-nums text-text-muted">{formatSpan(took, format)}</span>}
          {entry.tokensOut !== null && <span className="shrink-0 text-xs text-text-muted">{t('thread.agents.tokens', { value: formatTokens(entry.tokensOut, format.locale) })}</span>}
        </span>
        {entry.line !== null && <span className="truncate text-xs text-text-muted">{entry.line.text}</span>}
      </button>
      {end !== null && task !== null && task.can_end && <Button size="sm" variant="ghost" aria-label={t('backgroundWake.endNamed', { name: entry.name })} onClick={() => end(task)}>{t('composer.stop')}</Button>}
    </li>
  );
}

export function AgentsPanel({ entries, current, onMain, onOpen, onEnd, history }: {
  history?: AgentHistory;
  entries: readonly AgentEntry[];
  /** The step of the sub-agent shown in place of the main conversation, or `null` while Main is shown. */
  current: string | null;
  onMain: () => void;
  onOpen: (id: string) => void;
  /** Stops a running one; `null` where the reader may not. */
  onEnd: ((task: BackgroundWork) => void) | null;
}) {
  const { t } = useTranslation();
  const now = useRunningClock(entries.some((entry) => entry.state === 'running'));
  const [local, setLocal] = useState('');
  const query = history?.query ?? local;
  const setQuery = history?.setQuery ?? setLocal;
  const wanted = query.trim().toLocaleLowerCase();
  const matches = history !== undefined ? entries : entries.filter((entry) => wanted === '' || `${entry.name}\n${entry.line?.text ?? ''}`.toLocaleLowerCase().includes(wanted));
  return (
    <div className="flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-1" data-agents-panel>
      <Input label={t('thread.agents.search')} hideLabel placeholder={t('thread.agents.search')} value={query} onChange={(event) => setQuery(event.target.value)} />
      <button type="button" aria-current={current === null || undefined} onClick={onMain}
        className="focus-inset flex items-center gap-2 rounded-md px-2 py-1 text-left text-sm font-medium text-text hover:bg-bg-muted aria-[current=true]:bg-bg-muted">
        <MessageSquareText className="shrink-0 text-text-muted" aria-hidden />{t('thread.agents.main')}
      </button>
      <div className="max-h-80 min-h-0 overflow-y-auto" aria-busy={history?.loading === true}>
      {history?.loading === true && <div role="status" aria-label={t('thread.agents.loading')}><SkeletonRows rows={2} /></div>}
      {GROUPS.map((group) => {
        const listed = matches.filter((entry) => agentGroup(entry) === group);
        return listed.length === 0 ? null : (
          <section key={group} aria-label={t(`thread.agents.${group}`)}>
            <h3 className="px-2 pt-1 text-xs font-medium text-text-subtle">{t(`thread.agents.${group}`)}</h3>
            <ul className="flex max-h-72 flex-col overflow-y-auto">
              {listed.map((entry) => <Entry key={entry.id} entry={entry} now={now} current={current === entry.id} onOpen={onOpen} onEnd={onEnd} />)}
            </ul>
          </section>
        );
      })}
      {matches.length === 0 && history?.loading !== true && (history?.error === null || history?.error === undefined) && history?.hasMore !== true && <p className="px-2 py-1 text-sm text-text-muted">{t('thread.agents.noMatch')}</p>}
      {history !== undefined && <>
        {history.error !== null && entries.length === 0 ? <ErrorState compact error={history.error} onRetry={history.more} />
          : <Pagination hasMore={history.hasMore} loading={history.loading} error={history.error} onMore={history.more} autoLoad />}
      </>}
      </div>
    </div>
  );
}
