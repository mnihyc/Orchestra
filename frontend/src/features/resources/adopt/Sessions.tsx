/** Native sessions use one selected source, captured server search and a measured row window. */
import { useInfiniteQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { RefreshCw } from 'lucide-react';
import { type ReactNode, useCallback, useId, useMemo, useState, useSyncExternalStore } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { harnessSessionsPageQuery } from '@/api/adoption';
import { ApiError, client } from '@/api/client';
import { placePath } from '@/features/navigation/places';
import { AdoptDialog, sessionName } from '@/features/resources/adopt/AdoptDialog';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { PathValue } from '@/features/resources/TechnicalValue';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { AgentRef, Machine, NativeSession, Provider } from '@/generated/api-types';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { useListChoice } from '@/lib/listState';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { FilterBar } from '@/ui/FilterBar';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { Table, type Column } from '@/ui/Table';
import { ACTIONS } from '@/ui/actions';

export interface SessionHarness { providerId: string; providerName: string; agent: AgentRef }
/** Compatible profiles share their array; a machine never materializes the Cartesian product. */
export interface SessionHarnessGroup { providerId: string; providerName: string; agents: readonly AgentRef[] }

function activitySnapshot(cache: QueryClient, sources: ReadonlyMap<string, ReadonlySet<string>>, total: number): string {
  const latest = new Map<string, { at: number; state: 'success' | 'error' }>();
  let loading = false;
  for (const query of cache.getQueryCache().getAll()) {
    const [kind, provider, agent] = query.queryKey;
    if (kind !== 'harness-sessions' || typeof provider !== 'string' || typeof agent !== 'string' || sources.get(provider)?.has(agent) !== true) continue;
    loading ||= query.state.fetchStatus === 'fetching';
    const at = Math.max(query.state.dataUpdatedAt, query.state.errorUpdatedAt);
    if (at === 0 || query.state.status === 'pending') continue;
    const key = provider + ':' + agent;
    if ((latest.get(key)?.at ?? -1) <= at) latest.set(key, { at, state: query.state.status });
  }
  if (loading) return 'loading';
  const states = [...latest.values()], failed = states.filter((value) => value.state === 'error').length;
  if (failed > 0) return failed < states.length ? 'partial' : 'unavailable';
  return states.length === total && total > 0 ? 'available' : 'unknown';
}

/** Header status observes only reads that exist; it never creates a thousand disabled observers. */
export function ProviderActivityStatus({ harnesses }: { harnesses: readonly SessionHarnessGroup[] }) {
  const { t } = useTranslation(), cache = useQueryClient();
  const sources = useMemo(() => {
    const shared = new Map<readonly AgentRef[], ReadonlySet<string>>();
    return new Map(harnesses.map((group) => {
      let ids = shared.get(group.agents);
      if (ids === undefined) { ids = new Set(group.agents.map((agent) => agent.id)); shared.set(group.agents, ids); }
      return [group.providerId, ids] as const;
    }));
  }, [harnesses]);
  const total = harnesses.reduce((count, group) => count + group.agents.length, 0);
  const subscribe = useCallback((notify: () => void) => cache.getQueryCache().subscribe((event) => {
    const key = event.query.queryKey as readonly unknown[];
    const [kind, provider, agent] = key;
    if (kind === 'harness-sessions' && typeof provider === 'string' && typeof agent === 'string' && sources.get(provider)?.has(agent) === true) notify();
  }), [cache, sources]);
  const read = useCallback(() => activitySnapshot(cache, sources, total), [cache, sources, total]);
  const state = useSyncExternalStore(subscribe, read, read);
  if (state === 'unknown') return null;
  if (state === 'loading') return <StatusDot tone="info" label={t('adopt.historyLoading')} showLabel />;
  if (state === 'partial' || state === 'unavailable') return <StatusDot tone="warning" label={t(state === 'partial' ? 'adopt.historyPartial' : 'adopt.historyUnavailable')} showLabel />;
  return <StatusDot tone="idle" label={t('adopt.historyAvailable')} showLabel />;
}

export function harnessesOfMachine(machine: Pick<Machine, 'providers' | 'agents'>): SessionHarnessGroup[] {
  const profiles = new Map<string, AgentRef[]>();
  for (const agent of machine.agents) if (!agent.disabled) {
    const group = profiles.get(agent.harness_kind);
    if (group === undefined) profiles.set(agent.harness_kind, [agent]); else group.push(agent);
  }
  return machine.providers.flatMap((provider): SessionHarnessGroup[] => {
    if (provider.harness_kind === null) return [];
    const agents = profiles.get(provider.harness_kind);
    return agents === undefined ? [] : [{ providerId: provider.id, providerName: provider.name, agents }];
  });
}

export function harnessesOfProvider(provider: Pick<Provider, 'id' | 'name' | 'agent' | 'harnesses'>): SessionHarnessGroup[] {
  const agents = new Map<string, AgentRef>();
  if (provider.agent !== null) agents.set(provider.agent.id, provider.agent);
  for (const harness of provider.harnesses) agents.set(harness.agent.id, harness.agent);
  return agents.size === 0 ? [] : [{ providerId: provider.id, providerName: provider.name, agents: [...agents.values()] }];
}

function SessionRow({ session, onAdopt }: { session: NativeSession; onAdopt: () => void }) {
  const { t } = useTranslation(), name = sessionName(session, t('adopt.untitled'));
  return <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
    <div className="flex min-w-0 flex-1 flex-col">
      <span className="truncate font-medium text-text" title={name}>{name}</span>
      <span className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
        {session.cwd === null ? <span>{t('adopt.noFolder')}</span> : <PathValue path={session.cwd} />}
        {session.updated_at !== null && <span>{t('adopt.changed')} <RelativeTime at={session.updated_at} /></span>}
      </span>
    </div>
    {session.adopted_thread_id !== null ? <Link to={placePath({ id: session.adopted_thread_id, kind: 'thread' })}
      aria-label={t(ACTIONS.openThread.named, { name })} className="flex items-center gap-1 text-sm font-medium text-accent-text hover:underline">
      <ACTIONS.openThread.Icon aria-hidden /> {t(ACTIONS.openThread.label)}
    </Link> : <Button size="sm" icon={<ACTIONS.adoptSession.Icon aria-hidden />} onClick={onAdopt} aria-label={t(ACTIONS.adoptSession.named, { name })}>{t(ACTIONS.adoptSession.label)}</Button>}
  </div>;
}

interface Capture { source: string | null; generation: number; refused: unknown }
const EMPTY_CAPTURE: Capture = { source: null, generation: 0, refused: null };
function HarnessSessionList({ harness, showProvider, controls, view, capture, accept, restart, refuse }: {
  harness: SessionHarness; showProvider: boolean; controls: ReactNode; view: string; capture: Capture;
  accept: (source: string) => void; restart: () => void; refuse: (error: unknown) => void;
}) {
  const { t } = useTranslation(), shown = useViewShown();
  const search = useCollectionSearch(harness.providerId + ':' + harness.agent.id);
  const q = capture.source === null ? '' : search.q;
  const query = useInfiniteQuery({ ...harnessSessionsPageQuery(harness.providerId, harness.agent.id, q, {
    view, generation: capture.generation, source: capture.source, captured: accept, expired: restart, refused: refuse,
  }), enabled: shown && capture.refused === null, throwOnError: false });
  const [adopting, setAdopting] = useState<NativeSession | null>(null);
  const rows = capture.refused === null ? query.data?.pages.flatMap((page) => page.items) ?? [] : [];
  const title = showProvider ? t('adopt.harness', { agent: harness.agent.name, provider: harness.providerName }) : harness.agent.name;
  const error = capture.refused ?? query.error;
  const code = error instanceof ApiError ? error.code : null;
  const columns: Column<NativeSession>[] = [{ id: 'name', header: t('resources.listName'), cell: (session) => <SessionRow session={session} onAdopt={() => setAdopting(session)} /> }];
  return <div className="flex min-w-0 flex-col gap-3">
    <FilterBar search={<Input type="search" hideLabel label={t('ui.collection.find', { name: title })} placeholder={t('ui.combobox.search')} value={search.text} onChange={(event) => search.onText(event.target.value)} />}
      action={<IconButton size="sm" label={t('actions.refresh.label')} icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={restart} />}>{controls}</FilterBar>
    {capture.refused !== null ? <ErrorState compact error={capture.refused} onRetry={restart} />
      : query.isPending ? <SkeletonRows read={query} rows={3} />
      : query.isError && rows.length === 0 ? code === 'capability.unsupported' ? <p className="text-sm text-text-muted">{t('adopt.cannotList')}</p>
        : code === 'source.offline' ? <p className="text-sm text-text-muted">{t('adopt.offline')}</p>
        : <ErrorState compact error={error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
      : rows.length === 0 ? <EmptyState compact title={t('adopt.none')} />
      : <Table key={String(capture.generation) + ':' + q} label={t('adopt.sessionRows', { name: title })} rows={rows} rowKey={(row) => row.native_session_ref} columns={columns} virtual />}
    <Pagination hasMore={capture.refused === null && query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : undefined} onMore={() => void query.fetchNextPage()} />
    {adopting !== null && <AdoptDialog session={adopting} providerId={harness.providerId} agentId={harness.agent.id} onClose={() => setAdopting(null)} />}
  </div>;
}

export function HarnessSessions(props: { harnesses: readonly SessionHarnessGroup[]; showProvider?: boolean; scopeKey?: string }) {
  const reader = useStore(client.session, (state) => state.readerVersion ?? 0);
  return <SessionSources key={reader} {...props} />;
}
function SessionSources({ harnesses, showProvider = true, scopeKey }: { harnesses: readonly SessionHarnessGroup[]; showProvider?: boolean; scopeKey?: string }) {
  const { t } = useTranslation(), view = useId();
  const purpose = 'native-sessions:' + (scopeKey ?? harnesses[0]?.providerId ?? 'none');
  const [provider, setProvider] = useListChoice<string>(purpose, 'provider', '');
  const [agent, setAgent] = useListChoice<string>(purpose, 'profile', '');
  const selected = harnesses.find((group) => group.providerId === provider) ?? harnesses[0];
  const profile = selected?.agents.find((row) => row.id === agent) ?? selected?.agents[0];
  const [captures, setCaptures] = useState<ReadonlyMap<string, Capture>>(new Map());
  const pair = selected === undefined || profile === undefined ? '' : selected.providerId + ':' + profile.id;
  const capture = captures.get(pair) ?? EMPTY_CAPTURE;
  const change = (next: (before: Capture) => Capture) => setCaptures((before) => {
    const old = before.get(pair) ?? EMPTY_CAPTURE, value = next(old);
    if (value === old) return before;
    const updated = new Map(before); updated.set(pair, value); return updated;
  });
  const expected = capture.generation;
  const restart = () => change((before) => before.generation !== expected ? before : { source: null, generation: expected + 1, refused: null });
  const accept = (source: string) => change((before) => before.generation !== expected || before.source === source ? before : { ...before, source });
  const refuse = (error: unknown) => change((before) => before.generation !== expected || before.refused === error ? before : { ...before, refused: error });
  const controls = <>
    {showProvider && harnesses.length === 1 && selected !== undefined && <ResourceChip subject={{ kind: 'provider', provider: { id: selected.providerId, name: selected.providerName } }} hint={false} />}
    {selected?.agents.length === 1 && profile !== undefined && <ResourceChip subject={{ kind: 'agent', agent: profile }} hint={false} />}
    {harnesses.length > 1 && selected !== undefined && <ChoiceLocator label={t('adopt.provider')} value={selected.providerId} onValueChange={setProvider}
      options={harnesses.map((group) => ({ value: group.providerId, name: group.providerName }))} />}
    {selected !== undefined && selected.agents.length > 1 && profile !== undefined && <ChoiceLocator label={t('adopt.profile')} value={profile.id} onValueChange={setAgent}
      options={selected.agents.map((row) => ({ value: row.id, name: row.name }))} />}
  </>;
  return <Section id="harness-sessions" title={t('adopt.title')} lead={t('adopt.lead')}>
    {selected === undefined || profile === undefined ? <EmptyState compact title={t('adopt.noHarness')} />
      : <HarnessSessionList key={pair} harness={{ providerId: selected.providerId, providerName: selected.providerName, agent: profile }} showProvider={showProvider}
        controls={controls} view={view} capture={capture} accept={accept} restart={restart} refuse={refuse} />}
  </Section>;
}
