/** A scoped live model-request log with historical cursor pages and a pause that keeps one's place. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useEffect, useId, useMemo, useReducer, useRef, useState } from 'react';
import { ListFilter } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { eventsQuery, fetchEventsPage } from '@/api/usage';
import { orgsQuery } from '@/api/nodes';
import type { NodeRef, UsageEvent } from '@/generated/api-types';
import { emptyLog, matchesRequest, newestRequests, requestLog } from '@/features/usage/requests';
import { OUTCOMES, readOutcome } from '@/features/usage/outcomes';
import { measured, requestSpeed, speedText, timeText } from '@/features/usage/performance';
import { filtersOf, recordsParams, scopeText, type Span, type UsageScope, type UsageSearch } from '@/features/usage/search';
import { turnOrdinal } from '@/features/thread/turnLink';
import { accessProgramDisplayName, effortDisplayName, modelDisplayName } from '@/features/models/names';
import { programNameIn } from '@/features/resources/programNames';
import { Facts } from '@/features/resources/Facts';
import { offeredChoice } from '@/i18n/choices';
import { useFormatContext } from '@/lib/appearance';
import { formatExact } from '@/lib/time';
import { socket } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Select } from '@/ui/Select';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Table, type Column } from '@/ui/Table';

type Patch = { [K in keyof UsageSearch]?: UsageSearch[K] | undefined };
/** Select requires a nonempty item; this UI sentinel is never sent as an API filter. */
const ALL_FILTER = '__all__';
const MODEL_COLUMN = 'model';
const SPEED_COLUMN = 'speed';
const TOKENS_COLUMN = 'tokens';
const FIRST_COLUMN = 'first';
const WHOLE_COLUMN = 'whole';
const OUTCOME_OPTIONS = ['all', ...OUTCOMES] as const;
const REQUEST_FILTERS = ['person', 'model', 'provider', 'agent', 'environment', 'org', 'project', 'topic', 'thread', 'tier', 'effort', 'program'] as const;
/** The API's explicit absent-choice filter, rather than a translated option label. */
const NO_CHOICE = 'none';
export function Requests({ scope, search, span, onSearch, orgChoices = [] }: { scope: UsageScope; search: UsageSearch; span: Span; onSearch: (patch: Patch) => void; orgChoices?: readonly NodeRef[] }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const orgs=useQuery({...orgsQuery,staleTime:Infinity});
  const knownOrgs:readonly NodeRef[]=orgChoices.length>0 ? orgChoices : (orgs.data ?? []).map((org)=>({id:org.id,name:org.name,kind:'org' as const,redacted:false}));
  const [log, dispatch] = useReducer(requestLog, undefined, emptyLog);
  const [liveError, setLiveError] = useState(false);
  const [retry, setRetry] = useState(0);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const filtersId = useId();
  const params = useMemo(() => ({ ...recordsParams(scope, search, span), includeRunning: true, requestsOnly: true }), [scope, search, span]);
  const history = useInfiniteQuery(eventsQuery(params));
  const orgFilter = search.org;
  const scopeName = scopeText(scope);
  const filters = filtersOf(search);
  const known = useRef(new Set<string>());
  useEffect(() => { for (const row of [...log.rows.values(), ...log.waiting.values()]) if (!row.running) known.current.add(row.id); }, [log]);
  useEffect(() => {
    const rows=history.data?.pages.flatMap((page) => page.items) ?? [];
    for (const row of rows) if (!row.running) known.current.add(row.id);
    dispatch({ type: 'history', rows });
  }, [history.data]);
  useEffect(() => {
    const abort = new AbortController();
    const cancelled = () => abort.signal.aborted;
    let deliveries = Promise.resolve();
    const accept = async (items: UsageEvent[], cursor: string | null) => {
      const seen = new Set(known.current);
      const overlaps = (page: UsageEvent[]) => page.some((row) => !row.running && seen.has(row.id));
      const gap = seen.size > 0 && !overlaps(items);
      dispatch({ type: 'live', rows: items });
      for (const row of items) if (!row.running) known.current.add(row.id);
      if (!gap) return;
      try {
        // Read to a row already received, so bursts beyond the live frame's bound lose no completions.
        while (cursor !== null && !cancelled()) {
          const page = await fetchEventsPage(params, cursor, 200, abort.signal);
          if (cancelled()) return;
          dispatch({ type: 'live', rows: page.items });
          for (const row of page.items) if (!row.running) known.current.add(row.id);
          if (overlaps(page.items)) break;
          cursor = page.next_cursor;
        }
      } catch { if (!cancelled()) setLiveError(true); }
    };
    const subscription = socket.subscribe(`usage:${scopeName}${orgFilter === undefined ? '' : `:org:${orgFilter}`}`, { since: () => null, onFrame: (frame) => {
      // The initial historical query and the feed's own bounded snapshot already cover this
      // opening. On reconnect accept() repairs only a demonstrated gap through its cursor.
      if (frame.op === 'subscribed') return;
      if (frame.op === 'event' && Array.isArray(frame.data.items)) {
        const items = frame.data.items as UsageEvent[];
        const cursor = typeof frame.data.next_cursor === 'string' ? frame.data.next_cursor : null;
        deliveries = deliveries.then(() => { if (!cancelled()) return accept(items, cursor); });
        setLiveError(false);
      } else if (frame.op === 'closed' || frame.op === 'error') { abort.abort(); setLiveError(true); dispatch({ type: 'reset' }); }
    } });
    return () => { abort.abort(); subscription.unsubscribe(); };
  }, [scopeName, orgFilter, params, retry]);
  const inSpan = (row: UsageEvent) => row.at.slice(0, 10) >= span.from && row.at.slice(0, 10) <= span.to && matchesRequest(row, filters);
  // The HTTP pages are also filter-specific; never reuse previously visible rows of another place.
  const rows = liveError ? [] : newestRequests(log.rows).filter(inSpan);
  const pending = [...log.waiting.values()].filter((row) => !log.rows.has(row.id) && inSpan(row)).length;
  const number = (n: number) => new Intl.NumberFormat(format.locale).format(n);
  const outcome = (row: UsageEvent) => row.running ? t('usagePerf.running') : row.timing?.outcome.state === 'told'
    ? t(`usagePerf.outcomes.${row.timing.outcome.value}` as 'usagePerf.outcomes.done', { defaultValue: t('usagePerf.outcomes.harness_error') }) : t('usagePerf.notMeasured');
  const outcomeDetail = (row: UsageEvent) => {
    const code=row.timing?.outcome.state==='told' ? row.timing.outcome.value : null;
    if(code===null || code==='done' || code==='forced') return [];
    return [...new Set([row.timing?.error_message,row.timing?.vendor_words].filter((value):value is string=>typeof value==='string' && value!=='' && value!==code && value!==outcome(row)))];
  };
  const tokens = (row: UsageEvent, output = false) => {
    const reported = row.timing?.reported_tokens;
    const n = row.timing === undefined || row.scope !== 'request' ? output ? row.tokens.output_tokens : row.tokens.input_tokens : output ? reported?.output_tokens : reported?.input_tokens;
    return n === null || n === undefined ? t('usagePerf.dash') : number(n);
  };
  const harness=(row:UsageEvent)=>row.harness_kind===null ? t('usage.key.noAgent') : programNameIn(row,format.locale) ?? programNameIn(row.agent,format.locale) ?? t('resources.harnessNameUnknown');
  const agentLabel=(row:UsageEvent)=>row.agent?.name===row.harness_kind ? harness(row) : row.agent?.name ?? harness(row);
  const modelFacts=(row:UsageEvent)=>{
    const facts=[
    ...(scope.kind==='provider'?[]:[{name:t('usage.chip.provider'),value:row.provider.name}]),
    ...(row.harness_kind===null && row.agent===null?[]:[{name:t('usage.chip.agent'),value:agentLabel(row)}]),
    ...(row.tier===null?[]:[{name:t('usage.chip.tier'),value:offeredChoice(t,'serviceTier',row.tier)?.name??row.tier}]),
    ...(row.effort===null?[]:[{name:t('usage.chip.effort'),value:effortDisplayName(row.effort,t)}]),
    ...(row.access_program===null || row.access_program===undefined?[]:[{name:t('usage.chip.program'),value:accessProgramDisplayName(row.access_program,t)}]),
    ];
    return <div className="flex min-w-0 flex-col gap-1 text-sm wrap-anywhere"><span className="font-medium">{modelDisplayName(row.model_display,t)}</span><Facts compact rows={facts} /></div>;
  };
  const columns: Column<UsageEvent>[] = [
    { id: 'time', header: t('usagePerf.time'), cell: (row, shown) => {
      const ordinal = turnOrdinal(row.turn_ordinal);
      const thread = row.thread;
      const placeName = (place: NodeRef | null) => place === null ? t('usage.records.summary') : place.redacted || place.name===null ? t('usage.key.hidden', { kind: t(`usage.key.kind.${place.kind}` as 'usage.key.kind.thread') }) : place.name;
      const context=[
        ...(scope.kind==='me'?[]:[{name:t('usage.chip.person'),value:row.user?.display_name??t('usage.key.nobody')}]),
        ...[row.project,row.topic].filter((place)=>scope.kind!=='node' || place.id!==scope.id).map((place)=>({name:t(`usage.chip.${place.kind}` as 'usage.chip.project'),value:placeName(place)})),
      ];
      const threadName=scope.kind==='node' && thread?.id===scope.id && ordinal!==undefined ? t('usage.records.turn',{id:ordinal}) : placeName(thread);
      const compact = [
        ...(!shown.has(TOKENS_COLUMN) ? [{name:t('usagePerf.tokensInOut'),value:t('usagePerf.tokenPair', { input: tokens(row), output: tokens(row, true) })}] : []),
        ...(!shown.has(FIRST_COLUMN) ? [{name:t('usagePerf.requestFirstToken'),value:timeText(measured(row.timing?.first_token_ms), format.locale, t)}] : []),
        ...(!shown.has(SPEED_COLUMN) ? [{name:t('usagePerf.requestSpeed'),value:speedText(requestSpeed(row.tokens.output_tokens, row.timing), format.locale, t)}] : []),
        ...(!shown.has(WHOLE_COLUMN) ? [{name:t('usagePerf.wholeTime'),value:timeText(measured(row.timing?.request_ms), format.locale, t)}] : []),
      ];
      return <div className="flex min-w-0 flex-col gap-1 wrap-anywhere">
        {thread === null || thread.redacted || ordinal === undefined ? <span className="font-medium">{threadName}</span> : <Link to="/thread/$threadId" params={{ threadId: thread.id }} search={{ turn: ordinal }} className="font-medium text-accent-text hover:underline">{threadName}</Link>}
        <time dateTime={row.at} className="text-sm text-text-muted">{formatExact(row.at, format)}</time>
        {context.length>0 && <Facts compact rows={context} />}
        {!shown.has(MODEL_COLUMN) && modelFacts(row)}
        {compact.length>0 && <Facts compact rows={compact} />}
        <span className="text-sm text-text-muted">{outcome(row)}</span>
        {outcomeDetail(row).map((line)=><p key={line} className="text-sm wrap-anywhere text-text-muted">{line}</p>)}
      </div>;
    } },
    { id: MODEL_COLUMN, header: t('usagePerf.modelAndProvider'), from: 'md', cell: modelFacts },
    { id: TOKENS_COLUMN, header: t('usagePerf.tokensInOut'), from: 'lg', align: 'end', cell: (row) => <span className="tabular-nums">{tokens(row)} / {tokens(row, true)}</span> },
    { id: FIRST_COLUMN, header: t('usagePerf.requestFirstToken'), from: 'lg', align: 'end', cell: (row) => timeText(measured(row.timing?.first_token_ms), format.locale, t) },
    { id: SPEED_COLUMN, header: t('usagePerf.requestSpeed'), from: 'md', align: 'end', cell: (row) => <span className="tabular-nums">{speedText(requestSpeed(row.tokens.output_tokens, row.timing), format.locale, t)}</span> },
    { id: WHOLE_COLUMN, header: t('usagePerf.wholeTime'), from: 'lg', align: 'end', cell: (row) => timeText(measured(row.timing?.request_ms), format.locale, t) },
  ];
  return <Section id="usage-requests" title={t('usagePerf.requestsView')} action={<>
      <Button size="sm" onClick={() => dispatch({ type: log.paused ? 'resume' : 'pause' })}>{t(log.paused ? 'usagePerf.resume' : 'usagePerf.pause')}</Button>
      {log.paused && <Button size="sm" variant="ghost" onClick={() => dispatch({ type: 'resume' })}>{t('usagePerf.newRows', { count: pending, value: number(pending) })}</Button>}
      <ChoiceLocator size="sm" label={t('usagePerf.outcome')} value={search.outcome ?? 'all'} onValueChange={(value) => onSearch({ outcome: readOutcome(value) })} options={OUTCOME_OPTIONS.map((value) => ({ value, name: t(`usagePerf.choices.outcomes.${value}.name` as 'usagePerf.choices.outcomes.done.name'), description: t(`usagePerf.choices.outcomes.${value}.description` as 'usagePerf.choices.outcomes.done.description') }))} />
    </>}>
    <div className="sm:hidden"><Button size="sm" icon={<ListFilter aria-hidden />} aria-expanded={filtersOpen} aria-controls={filtersId} onClick={() => setFiltersOpen((open) => !open)}>{t('usagePerf.filters')}</Button></div>
    <fieldset id={filtersId} aria-label={t('usagePerf.requestFilters')} className={`${filtersOpen ? 'flex' : 'hidden'} flex-wrap items-end gap-3 border-b border-border pb-3 sm:flex`}>
      <legend className="mb-2 text-sm font-medium text-text">{t('usagePerf.requestFilters')}</legend>
      {REQUEST_FILTERS.map((key) => {
      const options = new Map<string, string>();
      if (key === 'org') for (const org of knownOrgs) if (!org.redacted && org.name !== null) options.set(org.id, org.name);
      for (const row of log.rows.values()) {
        const choice = key === 'person' ? row.user === null ? null : [row.user.id, row.user.display_name]
          : key === 'provider' ? [row.provider.id, row.provider.name] : key === 'model' ? [row.model, modelDisplayName(row.model_display,t)]
          : key === 'project' ? row.project.redacted ? null : [row.project.id, row.project.name ?? '']
          : key === 'topic' ? row.topic.redacted ? null : [row.topic.id, row.topic.name ?? '']
          : key === 'thread' ? row.thread === null || row.thread.redacted ? null : [row.thread.id, row.thread.name ?? '']
          : key === 'agent' ? row.harness_kind === null ? null : [row.harness_kind, agentLabel(row)]
          : key === 'environment' ? row.environment_kind === null ? null : [row.environment_kind, t(row.environment_kind === 'managed' ? 'usage.key.managed' : 'usage.key.connected')]
          : key === 'tier' ? [row.tier ?? NO_CHOICE, row.tier===null?t('usage.key.standard'):offeredChoice(t,'serviceTier',row.tier)?.name??row.tier]
          : key === 'effort' ? [row.effort ?? NO_CHOICE, row.effort===null?t('usage.key.noEffort'):effortDisplayName(row.effort,t)]
          : key === 'program' ? [row.access_program ?? NO_CHOICE, row.access_program === null || row.access_program === undefined ? t('usagePerf.noProgram') : accessProgramDisplayName(row.access_program,t)] : null;
        if (choice?.[0] !== undefined && choice[1] !== undefined) options.set(choice[0], choice[1]);
      }
      if (search[key] !== undefined && !options.has(search[key])) options.set(search[key], key === 'program' ? search[key] === NO_CHOICE ? t('usagePerf.noProgram') : accessProgramDisplayName(search[key],t) : t('usagePerf.selectedFilter'));
      return <Select key={key} size="sm" label={t(`usage.chip.${key}`)} value={search[key] ?? ALL_FILTER} onValueChange={(value) => onSearch({ [key]: value === ALL_FILTER ? undefined : value })} options={[{ value: ALL_FILTER, name: t('usagePerf.choices.all.name'), description: t('usagePerf.choices.all.description') }, ...[...options].map(([value, label]) => ({ value, name: t('usagePerf.choices.member.name', { name: label }), description: t('usagePerf.choices.member.description', { kind: t(`usage.chip.${key}`) }) }))]} />;
    })}</fieldset>
    {(history.isError || liveError) && <ErrorState error={history.error} onRetry={() => { setRetry((before) => before + 1); void history.refetch(); }} />}
    <p role="status" className="text-sm text-text-muted">{t(log.paused ? 'usagePerf.paused' : 'usagePerf.live')}</p>
    {rows.length === 0 && (history.isPending ? <SkeletonRows rows={4} /> : <p className="text-sm text-text-muted">{t('usagePerf.noRequests')}</p>)}
    <Table label={t('usagePerf.requestsView')} rows={rows} columns={columns} rowKey={(row) => row.id} />
    {history.hasNextPage && <Button size="sm" variant="ghost" disabled={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>{t('usagePerf.older')}</Button>}
  </Section>;
}
