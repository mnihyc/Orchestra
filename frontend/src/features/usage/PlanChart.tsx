/** A plan's fixed percentage scale: measured fill and its forecast, or consumption bars with reset markers. */
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { RotateCcw } from 'lucide-react';
import type { ForecastPoint, PlanUsageBucket, PlanWindowReset, UsageForecast } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DAY_MS, formatExact, formatTimeOfDay } from '@/lib/time';
import { Tooltip } from '@/ui/Tooltip';
import { nearestIndex } from '@/ui/timeSeries';
import { PLAN_READING_MAX_AGE, forecastTime } from './ForecastLine';
import { dayWords } from './Period';

/** Eight rem leaves room for the two scales without letting a small card dominate the page. */
const HEIGHT = 128;
const LEFT = 42;
const RIGHT = 12;
/** A marker rail stays above the limit label, including resets near the right edge. */
const TOP = 32;
const FLOOR = 100;
const FONT = 11;
/** Three time labels remain legible on the smallest card. */
const TICK_FRACTIONS = [0,0.5,1] as const;

export interface PlanChartProps {
  label: string;
  mode: 'current' | 'history';
  from: number;
  to: number;
  now: number;
  points: readonly ForecastPoint[];
  history: readonly PlanUsageBucket[];
  resets: readonly PlanWindowReset[];
  forecast?: UsageForecast | null | undefined;
  durationMinutes: number | null;
  auditHref?: (reset: PlanWindowReset) => string;
}

export function PlanChart({label,mode,from,to,now,points,history,resets,forecast:givenForecast,durationMinutes,auditHref}: PlanChartProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const forecast=givenForecast ?? null;
  const chartFormat=mode==='history' ? {...format,timeZone:'UTC'} : format;
  const host = useRef<HTMLDivElement>(null);
  const [width,setWidth] = useState(400);
  const [hover,setHover] = useState<string | null>(null);
  const [cursor,setCursor] = useState(0);
  const title = useId();
  const clip = useId();
  useEffect(() => {
    const element = host.current;
    if (element === null) return;
    const resize = () => setWidth(Math.max(1,element.clientWidth));
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(element);
    return () => observer.disconnect();
  },[]);
  const span = Math.max(1,to-from);
  const x = (at:number) => LEFT + (at-from)/span*Math.max(1,width-LEFT-RIGHT);
  const y = (percent:number) => FLOOR-Math.min(100,Math.max(0,percent))/100*(FLOOR-TOP);
  const measured = points.filter((point)=>Date.parse(point.at)>=from && Date.parse(point.at)<=to);
  const paths:{ path:string; first:number; last:number;firstPercent:number }[] = [];
  let previous:number | null = null;
  for (const point of measured) {
    const at = Date.parse(point.at);
    const segment=paths.at(-1);
    if (previous === null || at-previous > PLAN_READING_MAX_AGE || segment===undefined) paths.push({path:`M${x(at)},${y(point.used_percent)}`,first:at,last:at,firstPercent:point.used_percent});
    else { segment.path += ` L${x(at)},${y(point.used_percent)}`; segment.last=at; }
    previous=at;
  }
  const last=measured.at(-1);
  const runsOutAt=forecast?.runs_out_at ?? null;
  const forecastEnd = runsOutAt===null ? to : Date.parse(runsOutAt);
  const endPercent = forecast===null ? 0 : forecast.runs_out_at===null ? forecast.percent_at_end : 100;
  const hasForecast = mode==='current' && last !== undefined && forecast !== null && now-Date.parse(last.at)<=PLAN_READING_MAX_AGE;
  const atLimit=mode==='current' && last!==undefined && last.used_percent>=100 && now<to && now-Date.parse(last.at)<=PLAN_READING_MAX_AGE;
  const warningFrom=atLimit ? Date.parse(last.at) : hasForecast && forecast.runs_out_at!==null ? forecastEnd : null;
  const percent = (value:number) => formatNumber(value/100,format.locale,{style:'percent',maximumFractionDigits:1});
  const weekday=new Intl.DateTimeFormat(format.locale,{weekday:'short',timeZone:format.timeZone});
  const when = (at:number) => span<=DAY_MS ? formatTimeOfDay(at,chartFormat,now) : mode==='history' ? dayWords(new Date(at).toISOString().slice(0,10),format.locale,now) : weekday.format(at);
  const show = (at:number,value:number,predicted=false,reached=false) => setHover(`${formatExact(at,chartFormat)} · ${percent(value)}${predicted ? ` · ${t('planUsage.forecast')}` : reached ? ` · ${t('planUsage.limitReached')}` : ''}`);
  const markerWords = (reset:PlanWindowReset) => `${reset.count>1 ? `${t('planUsage.earlyResets',{count:reset.count})} · ${t('planUsage.latestCause',{cause:t(`planUsage.resetCause.${reset.cause}`)})}` : t(`planUsage.resetCause.${reset.cause}`)} · ${formatExact(reset.at,format)}`;
  const historyPoints = history.filter((bucket)=>Date.parse(bucket.at)<to && Date.parse(bucket.ends_at)>from);
  const readings:{at:number;value:number;predicted?:boolean;reached?:boolean}[] = mode==='current' ? [...measured.map((point)=>({at:Date.parse(point.at),value:point.used_percent})),...(hasForecast ? [{at:forecastEnd,value:endPercent,predicted:true}] : [])] : historyPoints.map((bucket)=>({at:Date.parse(bucket.at),value:bucket.used_percent,reached:bucket.reached_limit}));
  const reveal = (index:number) => { const point=readings[index]; if(point!==undefined) { setCursor(index); show(point.at,point.value,point.predicted===true,point.reached===true); } };
  const even = durationMinutes!==null && durationMinutes*60_000>=DAY_MS && historyPoints[0]!==undefined
    ? 100*(Date.parse(historyPoints[0].ends_at)-Date.parse(historyPoints[0].at))/(durationMinutes*60_000) : null;
  return <div ref={host} className="relative h-32 min-w-0" data-plan-chart={mode}>
    <svg width="100%" height={HEIGHT} viewBox={`0 0 ${width} ${HEIGHT}`} role="img" aria-labelledby={title} tabIndex={readings.length>0 ? 0 : -1}
      onFocus={()=>reveal(readings.length-1)} onBlur={()=>setHover(null)} onPointerLeave={()=>setHover(null)}
      onPointerMove={(event)=>{
        const frame=event.currentTarget.getBoundingClientRect();
        const at=from+(event.clientX-frame.left-LEFT)/Math.max(1,width-LEFT-RIGHT)*span;
        if(hasForecast && at>Date.parse(last.at)) {
          const duration=Math.max(1,forecastEnd-Date.parse(last.at));
          show(Math.min(at,forecastEnd),last.used_percent+(endPercent-last.used_percent)*Math.min(1,(at-Date.parse(last.at))/duration),true);
        } else { const index=nearestIndex(readings.map((point)=>point.at),at); if(index!==null) reveal(index); }
      }}
      onKeyDown={(event)=>{
        if(event.key==='Escape') { setHover(null); return; }
        const next=event.key==='Home' ? 0 : event.key==='End' ? readings.length-1 : event.key==='ArrowLeft' ? Math.max(0,cursor-1) : event.key==='ArrowRight' ? Math.min(readings.length-1,cursor+1) : null;
        if(next!==null) { event.preventDefault(); reveal(next); }
      }}>
      <title id={title}>{label}</title>
      <defs><clipPath id={clip}><rect x={LEFT} y={TOP} width={Math.max(1,width-LEFT-RIGHT)} height={FLOOR-TOP} /></clipPath></defs>
      {[0,50,100].map((value)=><g key={value}><line x1={LEFT} x2={width-RIGHT} y1={y(value)} y2={y(value)} stroke="var(--border)" strokeWidth={1} /><text x={LEFT-6} y={y(value)+4} textAnchor="end" fontSize={FONT} fill="var(--text-muted)">{percent(value)}</text></g>)}
      <text x={width-RIGHT} y={TOP-5} textAnchor="end" fontSize={FONT} fill="var(--text-muted)">{t('planUsage.limit')}</text>
      <g clipPath={`url(#${clip})`}>
        {warningFrom!==null && <rect x={x(warningFrom)} y={TOP} width={Math.max(0,x(to)-x(warningFrom))} height={FLOOR-TOP} fill="var(--warning-subtle)" />}
        {mode==='current' ? paths.map((segment,index)=><g key={index}><path d={`${segment.path} L${x(segment.last)},${FLOOR} L${x(segment.first)},${FLOOR} Z`} fill="var(--chart-1)" fillOpacity={0.08} /><path d={segment.path} fill="none" stroke="var(--chart-1)" strokeWidth={2} />{segment.first===segment.last && <circle cx={x(segment.first)} cy={y(segment.firstPercent)} r={2} fill="var(--chart-1)" />}</g>) : historyPoints.map((bucket)=>{
          const at=Date.parse(bucket.at),end=Date.parse(bucket.ends_at);
          const height=Math.max(bucket.reached_limit ? 1 : 0,FLOOR-y(bucket.used_percent));
          return <rect key={bucket.at} x={x(at)+1} y={FLOOR-height} width={Math.max(1,x(end)-x(at)-2)} height={height} fill={bucket.reached_limit ? 'var(--warning)' : 'var(--chart-1)'} onPointerEnter={()=>show(at,bucket.used_percent,false,bucket.reached_limit)} onFocus={()=>show(at,bucket.used_percent,false,bucket.reached_limit)}><title>{`${formatExact(at,chartFormat)} · ${percent(bucket.used_percent)}${bucket.reached_limit ? ` · ${t('planUsage.limitReached')}` : ''}`}</title></rect>;
        })}
        {mode==='history' && even!==null && <line x1={LEFT} x2={width-RIGHT} y1={y(even)} y2={y(even)} stroke="var(--text-subtle)" strokeWidth={1} />}
        {hasForecast && <path data-forecast-line d={`M${x(Date.parse(last.at))},${y(last.used_percent)} L${x(forecastEnd)},${y(endPercent)}`} stroke="var(--chart-1)" strokeDasharray="4 3" strokeWidth={2} fill="none" onPointerEnter={()=>show(forecastEnd,endPercent,true)} />}
        {mode==='current' && now>=from && now<=to && <line x1={x(now)} x2={x(now)} y1={TOP} y2={FLOOR} stroke="var(--text-subtle)" strokeWidth={1} />}
      </g>
      {mode==='history' && even!==null && <text x={width-RIGHT} y={Math.max(TOP+FONT,y(even)-4)} textAnchor="end" fontSize={FONT} fill="var(--text-muted)">{t('planUsage.evenPace',{percent:percent(even)})}</text>}
      {hasForecast && <><circle cx={x(forecastEnd)} cy={y(endPercent)} r={3} fill="var(--chart-1)" /><text x={Math.min(width-RIGHT,x(forecastEnd))} y={Math.max(TOP+FONT,y(endPercent)-5)} textAnchor="end" fontSize={FONT} fill="var(--text-muted)">{forecastTime(forecastEnd,now,format)}</text></>}
      {TICK_FRACTIONS.map((fraction)=><text key={fraction} x={x(from+span*fraction)} y={HEIGHT-5} textAnchor={fraction===0 ? 'start' : fraction===1 ? 'end' : 'middle'} fontSize={FONT} fill="var(--text-muted)">{when(from+span*fraction)}</text>)}
    </svg>
    {resets.filter((reset)=>Date.parse(reset.at)>=from && Date.parse(reset.at)<=to).map((reset)=><Tooltip key={reset.id} content={markerWords(reset)}><a href={auditHref?.(reset)} aria-label={markerWords(reset)} className="absolute -top-1 flex size-6 -translate-x-1/2 items-center justify-center rounded-full text-text-muted hover:text-text" style={{left:x(Date.parse(reset.at))}}>{reset.count>1 ? <span className="text-xs tabular-nums">{formatNumber(reset.count,format.locale,{notation:'compact',maximumFractionDigits:0})}</span> : <RotateCcw aria-hidden className="size-3" />}</a></Tooltip>)}
    {hover!==null && <span role="status" className="pointer-events-none absolute inset-x-0 top-5 z-10 w-fit max-w-full rounded border border-border bg-surface-raised px-2 py-1 text-xs text-text shadow-sm">{hover}</span>}
  </div>;
}
