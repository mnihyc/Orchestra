/** The provider's plan, with one card per window and one shared choice between the current window and history. */
import { useEffect,useRef,useState } from 'react';
import { useQuery,useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { PLAN_PAGE_SIZE,planUsageQuery } from '@/api/planUsage';
import type { PlanProvider } from '@/features/resources/providers/plan';
import { observedAt, privateProvider,planSectionId } from '@/features/resources/providers/plan';
import { detailState } from '@/features/resources/providers/details';
import { auditEntryAnchor } from '@/features/trace/params';
import type { ProviderPlanSeries, UsageBucket, UsageWindow } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatPercent } from '@/lib/format';
import { DAY_MS, formatSpan, formatTimeOfDay, useNow } from '@/lib/time';
import { boxClasses, Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { useHelpOpener } from '@/features/help/open';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { Skeleton } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { ForecastLine } from './ForecastLine';
import { PlanChart } from './PlanChart';
import { bucketAsked, bucketsFor, defaultBucket, type Span, type UsageSearch } from './search';

/** Wire timestamps cover a millisecond, including the database's finer precision within it. */
const AUDIT_TIMESTAMP_STEP_MS=1;

export interface PlanProps {
  provider:PlanProvider;
  span:Span;
  mode:'current' | 'history';
  bucket:UsageSearch['plan_bucket'];
  onMode:(mode:'current' | 'history')=>void;
  onBucket:(bucket:NonNullable<UsageSearch['plan_bucket']>)=>void;
}

/** The visible provider report starts its one shared plan read alongside analytics. */
export function usePlanUsage(provider:PlanProvider | undefined,span:Span,bucket:UsageSearch['plan_bucket'],enabled=true,cursor?:string) {
  const offered=bucketsFor(span.days);
  const chosen:NonNullable<UsageSearch['plan_bucket']>=bucket!==undefined && bucket!=='auto' && offered.includes(bucket) ? bucket : 'auto';
  const resolved=bucketAsked({bucket:chosen},span.days) ?? defaultBucket(span.days);
  const allowed=enabled && provider!==undefined && !privateProvider(provider) && detailState(provider,'windows')==='told' && provider.facts.windows.length>0;
  const query=useQuery({...planUsageQuery(provider?.id ?? '',span.from,span.to,resolved,provider?.facts_at ?? null,cursor),enabled:allowed});
  return {offered,chosen,resolved,allowed,query};
}

export function Plan(props:PlanProps) {
  return <PlanPage key={JSON.stringify([props.provider.id,props.provider.facts_at,props.span.from,props.span.to,props.bucket])} {...props} />;
}

function PlanPage({provider,span,mode,bucket,onMode,onBucket}:PlanProps) {
  const { t } = useTranslation();
  const openHelp=useHelpOpener();
  const client=useQueryClient();
  const [pages,setPages]=useState<(string | undefined)[]>([undefined]);
  const {allowed,offered,chosen,resolved,query}=usePlanUsage(provider,span,bucket,true,pages.at(-1));
  const retry=()=>{
    if(pages.length===1){void query.refetch();return;}
    // An expired snapshot must read the first page again, even while its source clock is cached.
    void client.invalidateQueries({queryKey:planUsageQuery(provider.id,span.from,span.to,resolved,provider.facts_at ?? null).queryKey,exact:true});
    void client.invalidateQueries({queryKey:['provider',provider.id]});
    setPages([undefined]);
  };
  if(!allowed) return null;
  const known=new Map(provider.facts.windows.map((window)=>[window.id,window]));
  const windows=query.data===undefined ? provider.facts.windows.filter((window)=>Number.isFinite(window.used_percent)).toSorted((one,other)=>Number(other.is_active===true)-Number(one.is_active===true) || (one.duration_minutes ?? Infinity)-(other.duration_minutes ?? Infinity)).slice(0,PLAN_PAGE_SIZE) : query.data.items.flatMap((series)=>{const window=known.get(series.window_id);return window===undefined ? [] : [window];});
  return <Section id={planSectionId(provider.id)} title={<span className="inline-flex items-center gap-2">{t('planUsage.title')}<HelpHint term={t('planUsage.forecast')} what={t('planUsage.hint')} guide={{article:'settings-and-usage',heading:'plan-windows-and-forecasts'}} more={{label:t('planUsage.help'),onOpen:()=>openHelp('settings-and-usage','plan-windows-and-forecasts')}} /></span>} className="@container" action={<>
    <SegmentedControl size="sm" label={t('planUsage.mode')} value={mode} onValueChange={onMode} segments={[{value:'current',name:t('planUsage.current')},{value:'history',name:t('planUsage.history')}]} />
    {mode==='history' && <Select size="sm" label={t('usage.chart.per')} value={chosen} onValueChange={onBucket} options={[{value:'auto' as const,name:t('usagePerf.choices.buckets.auto.name'),description:t('usagePerf.choices.buckets.auto.description')},...offered.map((value)=>({value,name:t(`usagePerf.choices.buckets.${value}.name`),description:t(`usagePerf.choices.buckets.${value}.description`)}))]} />}
  </>}>
    {query.isError ? <ErrorState compact error={query.error} onRetry={retry} action={pages.length>1 ? <Button size="sm" onClick={retry}>{t('common.tryAgain')}</Button> : undefined} /> : <div className="grid min-w-0 auto-rows-fr grid-cols-1 gap-4 @min-[640px]:grid-cols-2">{windows.map((window)=><PlanCard key={window.id} provider={provider} window={window} series={query.data?.items.find((series)=>series.window_id===window.id)} loading={query.isPending} mode={mode} span={span} bucket={resolved} />)}</div>}
    {(pages.length>1 || query.data?.next_cursor!==null && query.data?.next_cursor!==undefined) && <div className="flex justify-end gap-2 pt-2"><Button size="sm" variant="ghost" disabled={pages.length===1 || query.isFetching} onClick={()=>setPages((before)=>before.slice(0,-1))}>{t('planUsage.previousWindows')}</Button><Button size="sm" disabled={query.data?.next_cursor===null || query.data?.next_cursor===undefined || query.isFetching} onClick={()=>{const next=query.data?.next_cursor;if(next!==null && next!==undefined)setPages((before)=>[...before,next]);}}>{t('planUsage.nextWindows')}</Button></div>}
  </Section>;
}

export function PlanCard({provider,window,series,mode,span,bucket,loading=false}: {provider:PlanProvider;window:UsageWindow;series:ProviderPlanSeries | undefined;mode:'current' | 'history';span:Span;bucket:UsageBucket;loading?:boolean}) {
  const { t } = useTranslation();
  const format=useFormatContext();
  const now=useNow();
  const host=useRef<HTMLElement>(null);
  const [visible,setVisible]=useState(()=>typeof IntersectionObserver==='undefined');
  useEffect(()=>{const element=host.current;if(element===null || typeof IntersectionObserver==='undefined')return;const observer=new IntersectionObserver(([entry])=>setVisible(entry?.isIntersecting===true),{rootMargin:'128px'});observer.observe(element);return()=>observer.disconnect();},[]);
  const name=[window.label,window.model_family].filter((part)=>part!==null && part!=='').join(' · ');
  const reset=window.resets_at===null ? null : Date.parse(window.resets_at);
  const from=mode==='history' ? Date.parse(`${span.from}T00:00:00Z`) : series?.starts_at===null || series?.starts_at===undefined ? now : Date.parse(series.starts_at);
  const to=mode==='history' ? Date.parse(`${span.to}T00:00:00Z`)+DAY_MS : reset ?? now;
  const resetTime=reset===null ? t('providerPlan.resetUnknown') : reset-now<=DAY_MS
    ? t('planUsage.resetNear',{duration:formatSpan(Math.max(0,reset-now),format),time:formatTimeOfDay(reset,format,now)})
    : t('planUsage.resetFar',{time:new Intl.DateTimeFormat(format.locale,{weekday:'short',hour:'2-digit',minute:'2-digit',timeZone:format.timeZone}).format(reset)});
  const history=series?.history ?? [];
  const mean=series?.summary.mean_used_percent ?? null;
  const reached=series?.summary.limit_buckets ?? 0;
  const resets=series?.summary.early_resets ?? 0;
  const summary=[mean!==null && mean>0 ? t(`planUsage.average.${bucket}`,{percent:formatNumber(mean,format.locale,{maximumFractionDigits:1})}) : null,reached>0 ? t(`planUsage.limitBuckets.${bucket}`,{count:reached}) : null,resets>0 ? t('planUsage.earlyResets',{count:resets}) : null].filter((part):part is string=>part!==null);
  const markers=mode==='history' ? series?.resets ?? [] : series?.current_reset===null || series?.current_reset===undefined ? [] : [series.current_reset];
  return <article ref={host} className={`${boxClasses} flex h-full min-w-0 flex-col gap-3 p-4`} aria-label={name} data-plan-window={window.id}>
    <div className="flex min-w-0 items-baseline justify-between gap-3"><h3 className="min-w-0 text-base font-medium wrap-anywhere text-text">{name}</h3><span className="shrink-0 text-2xl font-semibold tabular-nums">{formatPercent(window.used_percent/100,format.locale)}</span></div>
    <p className="text-sm text-text-muted">{resetTime}</p>
    {loading || !visible ? <Skeleton className="h-32 w-full rounded" /> : (mode==='history' ? history.length===0 : (series?.points.length ?? 0)===0) ? <p className="flex h-32 items-center text-sm text-text-muted">{t('planUsage.noReadings')}</p> : <PlanChart label={name} mode={mode} from={from} to={to} now={now} points={series?.points ?? []} history={history} resets={markers} forecast={provider.plan_forecasts?.[window.id]} durationMinutes={window.duration_minutes}
      auditHref={(reset)=>`/trace/${provider.home.id}?${new URLSearchParams({action:reset.cause==='orch' ? 'provider.reset_used' : 'provider.window_started_over',from:reset.at,to:new Date(Date.parse(reset.at)+AUDIT_TIMESTAMP_STEP_MS).toISOString()})}#${auditEntryAnchor(reset.id)}`} />}
    <div className="mt-auto min-h-10">{mode==='current' ? <ForecastLine forecast={provider.plan_forecasts?.[window.id]} usedPercent={window.used_percent} resetsAt={window.resets_at} lastAt={observedAt(provider,`windows.${window.id}`) ?? provider.facts_at} /> : <p className="text-sm text-text-muted">{summary.length===0 ? t(history.length===0 ? 'planUsage.noReadings' : 'planUsage.noUsage') : summary.join(' · ')}</p>}</div>
  </article>;
}
