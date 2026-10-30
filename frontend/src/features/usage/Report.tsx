/** The usage screen, one for a person, a place and a provider: the span, the line that says how much the viewer may see there, and the three figures in turns, tokens and credits.
 *
 * One screen with three doors: one's own usage, a place's (an org, a project, a topic, a thread) and a
 * provider's. It is drawn from the figures and the records the API gives, and what it shows is as much
 * as the viewer may see there: the head line says how deep that is, a table of a dimension the viewer
 * may not see is not offered, and a place they cannot see is a line without a name that is counted all
 * the same. Everything the span asks (the figures, the series, a table for each dimension) comes in one
 * answer, including the span before it for comparison; the records page is read
 * beneath. A span in which nothing was used says so and draws no chart. Credits are an estimate and
 * Orch bills nobody, which the screen says once at its foot.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ChartColumn, X } from 'lucide-react';
import { type ReactNode, useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { providerQuery } from '@/api/resources';
import { eventsQuery, usageQuery } from '@/api/usage';
import type { Provider, UsageAnalytics, UsageDimension, UsageEvent, UsageKey } from '@/generated/api-types';
import { programNameIn } from '@/features/resources/programNames';
import { Breakdowns, type TableState } from '@/features/usage/Breakdowns';
import { UsageChart } from '@/features/usage/Chart';
import { Export } from '@/features/usage/Export';
import { Figures, TokenLine } from '@/features/usage/Figures';
import { Limits } from '@/features/usage/Limits';
import { chartEmptyText } from '@/features/usage/figures';
import { filterValue, keyLabel } from '@/features/usage/keys';
import { Period } from '@/features/usage/Period';
import { ScopePicker } from '@/features/usage/ScopePicker';
import { ReportFrame } from '@/features/usage/ReportFrame';
import { PerformanceView } from '@/features/usage/PerformanceView';
import { PerformanceComparison, PerformanceErrors, PerformanceFigures } from '@/features/usage/Performance';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { useHelpOpener } from '@/features/help/open';
import { HelpHint } from '@/ui/HelpHint';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Requests } from '@/features/usage/Requests';
import { readOutcome } from '@/features/usage/outcomes';
import { RatesDialog } from '@/features/usage/RateCard';
import { Records } from '@/features/usage/Records';
import { barsOf } from '@/features/usage/series';
import {
  activeFilters,
  bucketsFor,
  DEFAULT_MEASURE,
  DEFAULT_SPLIT,
  type FilterName,
  figuresParams,
  FILTER_DIMENSION,
  MEASURES,
  type Measure,
  offersThreads,
  patchSearch,
  parseScope,
  recordsParams,
  scopeText,
  ROWS,
  spanBefore,
  spanBetween,
  spanOf,
  type Split,
  tablesFor,
  today,
  type UsageScope,
  type UsageSearch,
} from '@/features/usage/search';
import { holds } from '@/lib/permissions';
import { boxClasses, Section } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { Tabs } from '@/ui/Tabs';
import { UsageVisibility } from './Visibility';
import { USAGE_VIEWS } from '@/features/usage/navigation';

import { useUsageChoices, type SearchPatch } from './choices';
import { Plan,usePlanUsage } from './Plan';

export interface ReportProps {
  providerData?:Provider | undefined;
  limitsSlot?:ReactNode;
  scope: UsageScope;
  /** What the address says. Without it the screen keeps its own, as a part of another screen does. */
  search?: UsageSearch;
  onSearch?: (patch: SearchPatch) => void;
  /** Stands inside another screen: no page of its own and no heading. */
  embedded?: boolean;
}

/** Says what the screen is, for the title of the window and the path above it; a screen that stands inside another says nothing. */
function Heading({ title, nodeId }: { title: string; nodeId: string | undefined }) {
  const { t } = useTranslation();
  useScreenHeading({ title, tabTitle: t('usage.screenTitle'), icon: ChartColumn, nodeId });
  return null;
}

function Skeletons({plan}:{plan?:ReactNode}={}) {
  return (
    <SkeletonGroup className="flex flex-col gap-4">
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-3">
        <Skeleton className="h-[6rem] rounded-lg" />
        <Skeleton className="h-[6rem] rounded-lg" />
        <Skeleton className="h-[6rem] rounded-lg" />
      </div>
      {plan}
      <Skeleton className="h-[15rem] rounded-lg" />
      <Skeleton className="h-[10rem] rounded-lg" />
    </SkeletonGroup>
  );
}

/** The label of a filter that is set, found among the rows of the table it narrows. */
function labelOfFilter(analytics: UsageAnalytics | undefined, records:readonly UsageEvent[], name: FilterName, value: string, t: TFunction, locale: string): string {
  if (name === 'turn') return t('usagePerf.selectedTurn');
  if(name==='outcome') return t(`usagePerf.choices.outcomes.${readOutcome(value) ?? 'all'}.name` as 'usagePerf.choices.outcomes.done.name');
  const dimension = FILTER_DIMENSION[name];
  if (dimension === undefined) return t(`usage.chip.${name}`);
  const found = analytics?.breakdowns.find((table) => table.by === dimension)?.rows.find((row) => filterValue(dimension, row.key) === value);
  if (found !== undefined) return keyLabel(dimension, found.key, t);
  for(const row of records) {
    const key:UsageKey={person:row.user,model:row.model,model_display:row.model_display,provider:row.provider,agent:row.harness_kind===null?null:{kind:row.harness_kind,name:programNameIn(row,locale) ?? programNameIn(row.agent,locale) ?? t('resources.harnessNameUnknown')},environment:row.environment_kind,project:row.project,topic:row.topic,thread:row.thread,effort:row.effort,tier:row.tier,program:row.access_program};
    if(filterValue(dimension,key)===value) return keyLabel(dimension,key,t);
  }
  if (name === 'environment') return value === 'managed' ? t('usage.key.managed') : t('usage.key.connected');
  return t(`usage.chip.${name}`);
}

export function Report({ scope: givenScope, search: given, onSearch, embedded = false,providerData:givenProvider,limitsSlot }: ReportProps) {
  const { t, i18n } = useTranslation();
  const now = useNow();
  const openHelp=useHelpOpener();
  const [own, setOwn] = useState<UsageSearch>({});
  const [openedAt,setOpenedAt] = useState(Date.now);
  const [ratesOpen, setRatesOpen] = useState(false);
  const [search,keepChoices]=useUsageChoices(given ?? own,embedded ? 'providerUsage' : 'usage',tablesFor(givenScope,null)[0] ?? 'model');
  const [colors,setColors]=useState<ReadonlyMap<string,number>>(new Map());
  const scope = given === undefined && own.scope !== undefined ? parseScope(own.scope) : givenScope;
  const view = search.view ?? 'usage';
  const change = useCallback((patch: SearchPatch) => {
    keepChoices(patch);
    if(patch.range!==undefined || patch.from!==undefined || patch.to!==undefined) setOpenedAt(Date.now());
    if(onSearch===undefined) setOwn((before)=>patchSearch(before,patch));else onSearch(patch);
  }, [onSearch,keepChoices,setOwn]);

  // The dated report is a snapshot. A midnight clock tick must not start another API read.
  const day = today(openedAt);
  const span = useMemo(() => spanOf(search, Date.parse(`${day}T12:00:00Z`)), [search, day]);
  const before = useMemo(() => spanBefore(span), [span]);

  const place = useQuery({ ...nodeQuery(scope.kind === 'node' ? scope.id : ''), enabled: scope.kind === 'node' });
  const provider = useQuery({ ...providerQuery(scope.kind === 'provider' ? scope.id : ''), enabled: scope.kind === 'provider' && givenProvider===undefined });
  const providerData=givenProvider ?? provider.data;
  usePlanUsage(scope.kind==='provider' ? providerData : undefined,span,search.plan_bucket,view==='usage');
  const placeName = scope.kind === 'node' ? (place.data?.name ?? null) : scope.kind === 'provider' ? (providerData?.name ?? null) : null;

  const offered = useMemo(() => {
    const original = tablesFor(scope, place.data?.kind ?? null);
    return view === 'performance' ? [...new Set<UsageDimension>([...original, 'person', 'provider', 'org', 'project', 'topic'])] : original;
  }, [scope, place.data?.kind, view]);
  const requested=search.series ?? DEFAULT_SPLIT;
  const seriesAllowed=requested==='none' || requested==='token_kind' && search.show==='tokens' || requested!=='token_kind' && (offered.includes(requested) || requested==='thread' && (scope.kind!=='node' || place.data!==undefined && (holds(place.data.access,'usage.view') || !holds(place.data.access,'usage.summary'))));
  const requestedSplit=seriesAllowed ? requested : DEFAULT_SPLIT;
  const chartBucket=search.bucket!==undefined && search.bucket!=='auto' && bucketsFor(span.days).includes(search.bucket) ? search.bucket : 'auto';
  const rows = search.more === true ? ROWS.many : ROWS.few;
  const params = useMemo(() => figuresParams(scope, {...search,series:requestedSplit}, span, offered, rows), [scope, search, requestedSplit, span, offered, rows]);
  const figures = useQuery({ ...usageQuery(params), enabled: view !== 'requests' });
  const analytics = figures.data;
  const depth = analytics?.depth ?? null;

  // The tables by thread are asked for when they are opened: a place's summary refuses them, and a reader cannot be known to have the reports before the first answer.
  const tabs: UsageDimension[] = useMemo(() => [...(offersThreads(scope, depth) && depth !== null ? (['thread'] as const) : []), ...offered], [offered, scope, depth]);
  const tab = search.by !== undefined && tabs.includes(search.by) ? search.by : (offered[0] ?? 'model');
  const threads = useQuery({ ...usageQuery({ ...params, breakdowns: ['thread'], seriesBy: undefined, bucket: undefined }), enabled: view !== 'requests' && tab === 'thread' && depth !== null });

  const performanceMeasure = search.show === 'first_token' || search.show === 'errors' ? search.show : 'speed';
  const measure: Measure = view === 'performance' ? performanceMeasure : search.show === 'tokens' || search.show === 'turns' || search.show === 'requests' || search.show === 'credits' ? search.show : DEFAULT_MEASURE;
  const wanted = search.series ?? DEFAULT_SPLIT;
  const split: Split = wanted === 'none' || wanted === 'token_kind' && measure === 'tokens' || wanted !== 'token_kind' && tabs.includes(wanted) ? wanted : DEFAULT_SPLIT;
  const stale = figures.isPlaceholderData;
  const bars = useMemo(() => (analytics === undefined ? null : barsOf(analytics, measure, split, t, span.range === 'all')), [analytics, measure, split, t, span.range]);

  const select = useCallback(
    (fromMs: number, toMs: number) => {
      const chosen = spanBetween(fromMs, toMs);
      change({ from: chosen.from, to: chosen.to });
    },
    [change],
  );

  const title =
    scope.kind === 'me' ? t('usage.title.me') : placeName === null ? t('usage.title.loading') : scope.kind === 'node' ? t('usage.title.place', { name: placeName }) : t('usage.title.provider', { name: placeName });
  const headline =
    depth === null
      ? null
      : scope.kind !== 'node'
        ? null
        : depth === 'own'
            ? t('usage.head.own', { name: placeName ?? '' })
            : depth === 'summary'
              ? t('usage.head.summary', { name: placeName ?? '' })
              : null;

  const tableOf = (dimension: UsageDimension): TableState => {
    if (dimension === 'thread') {
      if (threads.isError) return { state: 'failed', error: threads.error, retry: () => void threads.refetch() };
      const table = threads.data?.breakdowns.find((candidate) => candidate.by === 'thread');
      return table === undefined ? { state: 'loading' } : { state: 'ready', table };
    }
    const table = analytics?.breakdowns.find((candidate) => candidate.by === dimension);
    return table === undefined ? { state: 'loading' } : { state: 'ready', table };
  };

  // What bounds work where this is looked at: a place's limits, a provider's on its own tab, and, on one's own screen, the share of each allowance that covers one.
  const mayRates = scope.kind === 'node' && place.data !== undefined && holds(place.data.access, 'usage.view');
  const openRates = useCallback(() => setRatesOpen(true), []);
  const onRates = mayRates ? openRates : undefined;
  const placeLimits = useMemo(
    () => (place.data === undefined ? null : ({ kind: 'node', id: place.data.id, name: place.data.name, manage: holds(place.data.access, 'usage.manage'), reports: holds(place.data.access, 'usage.view') } as const)),
    [place.data],
  );
  const limits = limitsSlot ?? (scope.kind==='provider' ? providerData===undefined ? null : <Limits of={{kind:'provider',id:providerData.id,name:providerData.name,manage:holds(providerData.access,'provider.manage')}} /> : embedded ? null : scope.kind==='node' ? placeLimits===null ? null : <Limits of={placeLimits} onRates={onRates} /> : <Limits of={{kind:'mine'}} />);

  const filters = activeFilters(search);
  const recordParams = recordsParams(scope, search, span);
  // Observe the same history cache for human filter labels, without a second read.
  const labelRecords=useInfiniteQuery({...eventsQuery(recordParams),enabled:false});
  const records=labelRecords.data?.pages.flatMap((page)=>page.items) ?? [];
  const accessAndHelp = <>{scope.kind === 'node' && <UsageVisibility nodeId={scope.id} />}<ArticleGuideLink article="speed-first-output-and-errors" title={t('usagePerf.help')} /></>;
  const selectView = (next: 'usage' | 'performance' | 'requests') => change({view:next,show:undefined});
  const period = <Period span={span} onSearch={change} now={now} />;
  const exportAction = <Export scope={scope} span={span} figures={params} records={recordParams} />;
  const chooseScope = (next: string) => change({ scope: next, by: undefined, more: undefined, ...Object.fromEntries(activeFilters(search).map(({ name }) => [name, undefined])) });
  const planSection=scope.kind==='provider' && providerData!==undefined ? <Plan key={JSON.stringify([providerData.id,providerData.facts_at,span.from,span.to,search.plan_bucket])} provider={providerData} span={span} mode={search.plan ?? 'current'} bucket={search.plan_bucket} onMode={(plan)=>change({plan})} onBucket={(plan_bucket)=>change({plan_bucket})} /> : null;

  const body = (
    <div className="flex min-w-0 flex-col gap-4">
      {embedded && <Period span={span} onSearch={change} now={now} actions={<>
        <SegmentedControl size="sm" label={t('usagePerf.views')} value={view} onValueChange={selectView} segments={USAGE_VIEWS.map((value)=>({value,name:t(`usagePerf.viewNames.${value}`)}))} />
        {exportAction}
        <HelpHint term={t('usagePerf.help')} what={t('usagePerf.guideHint')} guide={{article:'speed-first-output-and-errors',heading:null}} more={{label:t('usagePerf.help'),onOpen:()=>openHelp('speed-first-output-and-errors')}} />
      </>} />}

      {headline !== null && <p className="text-sm text-text-muted">{headline}</p>}

      {filters.length > 0 && (
        <div role="group" aria-label={t('usage.chip.list')} className="flex flex-wrap items-center gap-2">
          {filters.map(({ name, value }) => {
            const label=labelOfFilter(analytics,records,name,value,t,i18n.resolvedLanguage ?? i18n.language);const what=t(`usage.chip.${name}`);const named=label!==what;
            return (
              <Button key={name} size="sm" className="max-w-full" icon={<X aria-hidden />}
                aria-label={named?t('usage.chip.removeValue',{what,value:label}):t('usage.chip.removeUnknown',{what})}
                onClick={() => change({ [name]: undefined })}
              >
                <span className="min-w-0 text-left whitespace-normal wrap-anywhere">{named?t('usage.chip.label',{what,value:label}):what}</span>
              </Button>
          );})}
          {filters.length > 1 && (
              <Button size="sm" variant="ghost" onClick={() => change(Object.fromEntries(filters.map(({ name }) => [name, undefined])))}>
                {t('usage.chip.clear')}
              </Button>
          )}
        </div>
      )}

      {view === 'requests' ? <Requests key={`${scopeText(scope)}:${search.org ?? ''}`} scope={scope} search={search} span={span} onSearch={change} orgChoices={analytics?.breakdowns.filter((table) => table.by === 'org').flatMap((table) => table.rows.flatMap((row) => row.key.org === null || row.key.org === undefined ? [] : [row.key.org])) ?? []} />
        : view === 'performance' ? figures.isPending ? <Skeletons /> : figures.isError && analytics === undefined ? <ErrorState error={figures.error} onRetry={() => void figures.refetch()} /> : analytics === undefined || bars === null ? null : <PerformanceView
          measure={performanceMeasure} onMeasure={(show) => change({ show })}
          figures={<PerformanceFigures current={analytics.totals.performance} previous={before === null ? undefined : analytics.performance_before} before={before === null ? '' : t('usage.figures.before', { count: span.days })} />}
          controls={<><ChoiceLocator size="sm" label={t('usage.chart.splitBy')} value={split} onValueChange={(series) => change({ series })} options={[...(measure==='tokens' ? [{value:'token_kind' as const,name:t('usagePerf.choices.splits.token_kind.name'),description:t('usagePerf.choices.splits.token_kind.description')}] : []),...tabs.map((one) => ({ value: one, name: t(`usagePerf.choices.splits.${one}.name`), description: t(`usagePerf.choices.splits.${one}.description`) })), { value: 'none', name: t('usagePerf.choices.splits.none.name'), description: t('usagePerf.choices.splits.none.description') }]} /><Select size="sm" label={t('usage.chart.per')} value={chartBucket} onValueChange={(bucket) => change({bucket})} options={[{ value: 'auto' as const, name: t('usagePerf.choices.buckets.auto.name'), description: t('usagePerf.choices.buckets.auto.description') }, ...bucketsFor(span.days).map((one) => ({ value: one, name: t(`usagePerf.choices.buckets.${one}.name`), description: t(`usagePerf.choices.buckets.${one}.description`) }))]} /></>}
          chart={<UsageChart onColors={setColors} label={t('usagePerf.chart')} bars={bars} measure={measure} onSelect={select} empty={<span>{t('usagePerf.notMeasured')}</span>} stale={stale} />}
          errors={<PerformanceErrors performance={analytics.totals.performance} onOutcome={(outcome) => change({ view: 'requests', outcome: readOutcome(outcome) })} />}
          comparison={<PerformanceComparison rows={analytics.comparison ?? []} modelNames={new Map(analytics.breakdowns.filter((table)=>table.by==='model').flatMap((table)=>table.rows.flatMap((row)=>row.key.model===undefined || row.key.model===null || row.key.model_display===undefined || row.key.model_display===null ? [] : [[row.key.model,row.key.model_display] as const])))} onFilter={change} />}
          breakdowns={<Breakdowns colors={colors} seriesBy={split} performance tabs={tabs} tab={tab} onTab={(by) => change({ by })} tableOf={tableOf} search={search} onFilter={(name, value) => change({ [name]: search[name] === value ? undefined : value })} showingMore={search.more === true} onMore={() => change({ more: true })} />}
        />
        : figures.isPending ? (
        <Skeletons plan={planSection} />
      ) : figures.isError && analytics === undefined ? (
        <><ErrorState error={figures.error} retrying={figures.isRefetching} onRetry={() => void figures.refetch()} />{planSection}</>
      ) : analytics === undefined || bars === null ? null : (
        <>
          <Figures
            totals={analytics.totals}
            previous={before === null ? null : (analytics.figures_before ?? null)}
            before={before === null ? '' : t('usage.figures.before', { count: span.days })}
            onRates={onRates}
          />
          <TokenLine tokens={analytics.totals.tokens} />
          {planSection}
          <Section id="usage-over-time" title={t('usage.sections.overTime')} className={`${boxClasses} min-w-0 p-4`} action={<>
            <Select size="sm" label={t('usage.chart.show')} value={measure} onValueChange={(show)=>change({show})} options={MEASURES.filter((one)=>one==='turns' || one==='requests' || one==='tokens' || one==='credits').map((value)=>({value,name:t(`usagePerf.choices.measures.${value}.name`),description:t(`usagePerf.choices.measures.${value}.description`)}))} />
            <ChoiceLocator size="sm" label={t('usage.chart.splitBy')} value={split} onValueChange={(series)=>change({series})} options={[...(measure==='tokens' ? [{value:'token_kind' as const,name:t('usagePerf.choices.splits.token_kind.name'),description:t('usagePerf.choices.splits.token_kind.description')}] : []),...tabs.map((value)=>({value,name:t(`usagePerf.choices.splits.${value}.name`),description:t(`usagePerf.choices.splits.${value}.description`)})),{value:'none',name:t('usagePerf.choices.splits.none.name'),description:t('usagePerf.choices.splits.none.description')}]} />
            <Select size="sm" label={t('usage.chart.per')} value={chartBucket} onValueChange={(bucket)=>change({bucket})} options={[{value:'auto' as const,name:t('usagePerf.choices.buckets.auto.name'),description:t('usagePerf.choices.buckets.auto.description')},...bucketsFor(span.days).map((value)=>({value,name:t(`usagePerf.choices.buckets.${value}.name`),description:t(`usagePerf.choices.buckets.${value}.description`)}))]} />
          </>}>
            <UsageChart onColors={setColors} label={t('usage.chart.label')} bars={bars} measure={measure} onSelect={select} empty={<span>{chartEmptyText(analytics.totals,measure,t)}</span>} stale={stale} />
          </Section>
          {limits}
              <Breakdowns colors={colors} seriesBy={split}
                tabs={tabs}
                tab={tab}
                onTab={(next) => change({ by: next })}
                tableOf={tableOf}
                search={search}
                onFilter={(name, value) => change({ [name]: search[name] === value ? undefined : value })}
                showingMore={search.more === true}
                onMore={() => change({ more: true })}
              />
              <Records scope={scope} search={search} span={span} depth={depth} />
        </>
      )}

      <p className="flex max-w-prose flex-wrap items-baseline gap-x-3 text-sm text-text-muted">
        <span>{t('usage.note')}</span>
        {onRates !== undefined && (
          <Button size="sm" variant="ghost" className="-ml-2" onClick={onRates}>
            {t('usage.rates.open')}
          </Button>
        )}
      </p>
      {mayRates && (
        <RatesDialog
          open={ratesOpen}
          onOpenChange={setRatesOpen}
          node={{ id: place.data.id, name: place.data.name, kind: place.data.kind }}
          manage={place.data.kind === 'org' && holds(place.data.access, 'settings.manage')}
        />
      )}
    </div>
  );

  const viewOptions = USAGE_VIEWS.map((value) => ({ value, label: t(`usagePerf.viewNames.${value}`), panel: body }));
  const views = <Tabs label={t('usagePerf.views')} value={view} onValueChange={selectView} panelClassName="pt-4" tabs={viewOptions} />;
  if (embedded) return body;
  return (
    <ReportFrame
      heading={<Heading title={title} nodeId={scope.kind === 'node' ? scope.id : undefined} />}
      title={t('usage.screenTitle')} icon={<ChartColumn aria-hidden />}
      actions={<>{accessAndHelp}{exportAction}</>}
      filters={<><ScopePicker scope={scope} name={placeName} rootOrgId={place.data?.org_id} onChange={chooseScope} />{period}</>}
    >{views}</ReportFrame>
  );
}

