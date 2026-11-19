/** The parts of the status bar: connection, with a slow link said as such, thread, environment, branch, limits, shells, cursor and help, each shown where it means something and opening what it names.
 *
 * The live thread supplies its environment and isolation facts; the views' existing queries supply
 * branch, changed lines, near limits and shells. Every interactive part reveals the view it names.
 * Connection and help also apply on screens that have no thread.
 */
import { useMemo } from 'react';
import { useWorkbench } from './store';
import { findTab } from './layout';
import { pageAddress, parsePageAddress } from './PageView';
import { contentThreadRead } from '@/realtime/threadScope';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { CircleCheck, Gauge, GitBranch, LoaderCircle, Shield, SquareTerminal } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { useQuery } from '@tanstack/react-query';

import { shellsQuery } from '@/api/terminals';
import { placeLimitsQuery } from '@/api/usage';
import { changesQuery, gitStatusQuery } from '@/api/workspace';
import { useThreadDoor } from '@/features/files/door';
import { capSentence, capTitle, usedWords } from '@/features/usage/words';
import { useHarnessNames } from '@/features/resources/harnesses';
import { ProviderDegradedPart } from '@/features/usage/PerformancePlaces';
import { revealView } from '@/features/workbench/openView';
import { isolationWords, nearLimit, workspaceWords } from '@/features/workbench/statusFacts';
import { changeTotals } from '@/features/workbench/Summaries';
import type { Thread } from '@/generated/api-types';
import { useThreadRow } from '@/realtime/stores/thread';
import { EnvironmentDot } from '@/ui/EnvironmentDot';

import { useAppearance, useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useSocketStatus } from '@/realtime/live';
import { StatusPart } from '@/ui/StatusBar';
import { StatusPlace } from '@/features/status-items/StatusPlace';

/** Above this round trip a connection is called slow: the latency pages are commonly measured under. */
export const SLOW_ABOVE_MS = 150;

/** How long the socket may be gone before anything says so: a line that comes and goes does not make the screen flicker. */
export const LOST_SAID_AFTER_MS = 2_000;

/** The connection: nothing while it is there and quick. */
export function ConnectionPart() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const status = useSocketStatus();

  if (status.phase === 'idle') return null;
  if (status.phase !== 'ready') {
    return (
      <StatusPart icon={<LoaderCircle size={16} className="animate-spin" aria-hidden />} hint={t('workbench.status.reconnectingHint')}>
        {t('workbench.status.reconnecting')}
      </StatusPart>
    );
  }
  if (status.roundTripMs === null || status.roundTripMs <= SLOW_ABOVE_MS) return <StatusPart icon={<CircleCheck size={16} aria-hidden />} hint={t('workbench.status.connectedHint')}>{t('workbench.status.connected')}</StatusPart>;
  return (
    <StatusPart tone="warning" icon={<Gauge size={16} aria-hidden />} hint={t('workbench.status.slowHint')}>
      {t('workbench.status.slow', { ms: formatNumber(Math.round(status.roundTripMs), locale) })}
    </StatusPart>
  );
}

/** Status parts share the views' query keys and live updates. */

function WorkspacePart({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const state = useThreadDoor(thread.id);
  const ready = state.kind === 'ready' && state.access.mayView;
  const door = state.kind === 'ready' ? state.access.door : { environmentId: '', threadId: thread.id };
  const git = useQuery({ ...gitStatusQuery(door), enabled: false, staleTime: Number.POSITIVE_INFINITY, refetchOnWindowFocus: false, refetchOnReconnect: false, retry: false });
  const changes = useQuery({ ...changesQuery(door, false), enabled: false });
  const words = ready ? workspaceWords(thread, git.data, t, format) : null;
  if (words === null) return null;
  const totals = changes.data === undefined ? null : changeTotals(changes.data);
  const lines = totals === null ? '' : ` +${totals.added} −${totals.removed}`;
  return <StatusPart icon={<GitBranch size={16} aria-hidden />} label={t('workbench.status.openChanges', { workspace: `${words.label}${lines}` })} hint={words.hint} onOpen={() => revealView('changes')}>
    {words.label}{lines}
  </StatusPart>;
}

function IsolationPart({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const words = isolationWords(thread, t, locale);
  return words === null ? null : <StatusPart icon={<Shield size={16} aria-hidden />} hint={words.hint} tone={words.warning ? 'warning' : 'plain'}>{words.label}</StatusPart>;
}

function LimitPart({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  // The open Usage view owns this read; status observes facts already held, without waking a hidden panel.
  const query = useQuery({ ...placeLimitsQuery(thread.id), enabled: false });
  const limit = nearLimit(query.data?.caps ?? []);
  // Status observes names already read by an open view, without starting another registry request.
  const programs = useHarnessNames(false);
  const blocked = thread.blocked?.reason;
  if (limit === null && blocked !== 'provider_exhausted' && blocked !== 'quota_exceeded') return null;
  const reached = limit?.state === 'reached' || limit === null;
  const name = limit === null ? t('providerPlan.limits') : capTitle(limit.cap, t, programs.title);
  const label = t(reached ? 'workbench.status.limitReached' : 'workbench.status.limitNear', { name });
  const hint = limit === null ? label : `${usedWords(limit, t, locale)}. ${capSentence(limit.cap, t, locale, programs.title)}`;
  return <StatusPart icon={<Gauge size={16} aria-hidden />} tone={reached ? 'danger' : 'warning'} hint={hint} onOpen={() => revealView('usage')}>{label}</StatusPart>;
}

function ShellsPart({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const state = useThreadDoor(thread.id);
  const allowed = state.kind === 'ready' && state.access.mayWatchProcesses;
  const door = state.kind === 'ready' ? state.access.door : { environmentId: '', threadId: thread.id };
  const query = useQuery({ ...shellsQuery(door), enabled: false });
  if (!allowed || query.data === undefined) return null;
  const count = query.data.filter((shell) => shell.state !== 'closed').length;
  return <StatusPart icon={<SquareTerminal size={16} aria-hidden />} hint={t('workbench.status.shellsHint')} onOpen={() => revealView('terminal')}>{t('workbench.status.shells', { count })}</StatusPart>;
}

/** Facts from the live thread row and the same workspace, limits and shell queries its views read. */
export function ThreadStatusParts() {
  const tab = useWorkbench((state) => state.activeContent === null ? null : findTab(state.layout, state.activeContent)?.tab ?? null);
  const href = tab === null ? null : pageAddress(tab);
  const target = useMemo(() => href === null ? null : parsePageAddress(href), [href]);
  const threadId = target?.route === '/thread/$threadId/' ? target.params.threadId : undefined;
  const owner = tab?.id;
  const read = useMemo(() => threadId === undefined || owner === undefined ? null : contentThreadRead(threadId, owner), [threadId, owner]);
  return read === null ? <StatusPlace place="bar" /> : <ThreadReadProvider read={read}><StatusForThread threadId={read.threadId} /></ThreadReadProvider>;
}

function StatusForThread({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const thread = useThreadRow(threadId);
  if (thread === null) return null;
  const environment = thread.environment;
  return <span className="contents" data-thread-status>
    <StatusPlace place="bar" threadId={threadId} fixed={[
      ...(environment === null ? [] : [{ id: 'environment', content: <ResourceName subject={{ kind: 'environment', environment }} asChild><button type="button" className="focus-inset flex min-w-0 items-center gap-1 rounded-sm px-1 hover:bg-bg-muted hover:text-text" aria-label={t('workbench.status.openEnvironment', { name: environment.name })}><EnvironmentDot status={environment.status} /><span className="truncate">{environment.name}</span><ResourceHint subject={{ kind: 'environment', environment }} className="truncate" /></button></ResourceName> }]),
      { id: 'workspace', content: <WorkspacePart thread={thread} /> },
      { id: 'isolation', content: <IsolationPart thread={thread} /> },
      { id: 'limits', content: <LimitPart thread={thread} /> },
      { id: 'provider-degraded', content: <ProviderDegradedPart thread={thread} /> },
      { id: 'shells', content: <ShellsPart thread={thread} /> },
    ]} />
  </span>;
}

