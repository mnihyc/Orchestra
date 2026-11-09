/** Actual vendor amount observations, grouped by the server's source/unit identity; no consumption arithmetic. */
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ProviderCreditPoint, ProviderCreditSeries } from '@/generated/api-types';
import { scaledAmountText } from '@/features/resources/providers/details';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatExact } from '@/lib/time';
import { nearestIndex, scaleTop } from '@/ui/timeSeries';

// Match the existing PlanChart's compact height and time-label density.
const HEIGHT=160,LEFT=58,RIGHT=12,TOP=16,FLOOR=130,FONT=11;
const TICKS=[0,0.5,1] as const;
const SLOTS={remaining:1,total:4,used:2,limit:3} as const;

/** Only SVG coordinates become numbers; underflow never turns a nonzero amount into a plotted zero. */
export function creditCoordinate(point:ProviderCreditPoint,series:ProviderCreditSeries):number | null {
  if(point.amount===null || point.unlimited===true || point.enabled===false)return null;
  const value=Number(point.amount)*10**-(series.decimal_places ?? 0);
  return !Number.isFinite(value) || value===0 && !/^-?0+(?:\.0+)?$/.test(point.amount) ? null : value;
}

export function creditGroups(series:readonly ProviderCreditSeries[]):ProviderCreditSeries[][] {
  const groups=new Map<string,ProviderCreditSeries[]>();
  for(const row of series){const group=groups.get(row.group_id);if(group===undefined)groups.set(row.group_id,[row]);else group.push(row);}
  return [...groups.values()];
}

export function CreditChart({series,from,to}:{series:readonly ProviderCreditSeries[];from:number;to:number}) {
  const {t}=useTranslation();
  const format=useFormatContext();
  const host=useRef<HTMLDivElement>(null);
  const [width,setWidth]=useState(400),[hover,setHover]=useState<string | null>(null),[cursor,setCursor]=useState(0);
  const title=useId();
  useEffect(()=>{
    const element=host.current;if(element===null)return;
    const resize=()=>setWidth(Math.max(1,element.clientWidth));resize();
    const observer=new ResizeObserver(resize);observer.observe(element);return()=>observer.disconnect();
  },[]);
  const pointText=(point:ProviderCreditPoint,row:ProviderCreditSeries)=>point.enabled===false ? t('providerPlan.extraOff') : point.unlimited===true ? t('providerPlan.unlimited') : scaledAmountText(point.amount,row.unit,row.decimal_places,t('providerPlan.notReported'),t('providerPlan.unitUnknown'));
  const quantity=(row:ProviderCreditSeries)=>t(`providerCredits.quantity.${row.member==='balances' && row.quantity==='remaining' ? 'balance' : row.member==='extra_usage' && row.quantity==='used' ? 'paid' : row.quantity}`);
  const rows=series.map((row)=>({...row,points:row.points.filter((point)=>Date.parse(point.at)>=from && Date.parse(point.at)<=to)}));
  const readings=rows.flatMap((row)=>row.points.map((point)=>({row,point,at:Date.parse(point.at)}))).sort((one,other)=>one.at-other.at);
  const values=rows.flatMap((row)=>row.points.flatMap((point)=>{const value=creditCoordinate(point,row);return value===null ? [] : [value];}));
  const minimum=Math.min(0,...values),maximum=scaleTop(values.length===0 ? null : Math.max(...values));
  const x=(at:number)=>LEFT+(at-from)/Math.max(1,to-from)*Math.max(1,width-LEFT-RIGHT);
  const y=(value:number)=>FLOOR-(value-minimum)/Math.max(Number.MIN_VALUE,maximum-minimum)*(FLOOR-TOP);
  const when=(at:number)=>new Intl.DateTimeFormat(format.locale,{month:'short',day:'numeric',timeZone:format.timeZone}).format(at);
  const unit=rows[0]?.unit ?? t('providerPlan.unitUnknown');
  const name=rows[0]?.label ?? t('providerCredits.history');
  const reveal=(index:number,allAtTime=false)=>{
    const reading=readings[index];if(reading===undefined)return;
    const selected=allAtTime ? readings.filter((one)=>one.at===reading.at) : [reading];
    setCursor(index);
    setHover(`${formatExact(reading.at,format)} · ${selected.map((one)=>`${quantity(one.row)}: ${pointText(one.point,one.row)}`).join(' · ')}`);
  };
  return <div ref={host} className="flex min-w-0 flex-col gap-2" data-credit-group={rows[0]?.group_id}>
    <h3 className="text-sm font-medium text-text">{name}</h3>
    <ul className="flex flex-wrap gap-x-4 gap-y-1 text-sm" aria-label={t('providerCredits.quantities')}>{rows.map((row)=>{
      const last=row.points.at(-1);
      return <li key={row.series_id} className="flex items-baseline gap-1"><span aria-hidden className="inline-block h-0.5 w-4" style={{background:`var(--chart-${SLOTS[row.quantity]})`}} /><span>{quantity(row)}{': '}{last===undefined ? t('planUsage.noReadings') : pointText(last,row)}</span></li>;
    })}</ul>
    {readings.length===0 ? <p className="flex h-40 items-center text-sm text-text-muted">{t('planUsage.noReadings')}</p> : <div className="relative min-w-0" data-credit-chart>
      <svg width="100%" height={HEIGHT} viewBox={`0 0 ${width} ${HEIGHT}`} role="img" aria-labelledby={title} tabIndex={0}
        onFocus={()=>reveal(readings.length-1)} onBlur={()=>setHover(null)} onPointerLeave={()=>setHover(null)}
        onPointerMove={(event)=>{const rect=event.currentTarget.getBoundingClientRect();const at=from+(event.clientX-rect.left-LEFT)/Math.max(1,width-LEFT-RIGHT)*(to-from);const index=nearestIndex(readings.map((reading)=>reading.at),at);if(index!==null)reveal(index,true);}}
        onKeyDown={(event)=>{if(event.key==='Escape'){setHover(null);return;}const next=event.key==='Home' ? 0 : event.key==='End' ? readings.length-1 : event.key==='ArrowLeft' ? Math.max(0,cursor-1) : event.key==='ArrowRight' ? Math.min(readings.length-1,cursor+1) : null;if(next!==null){event.preventDefault();reveal(next);}}}>
        <title id={title}>{`${name} · ${unit}`}</title>
        {values.length>0 && TICKS.map((fraction)=>{const value=minimum+(maximum-minimum)*fraction;return <g key={fraction}><line x1={LEFT} x2={width-RIGHT} y1={y(value)} y2={y(value)} stroke="var(--border)" /><text x={LEFT-6} y={y(value)+4} textAnchor="end" fontSize={FONT} fill="var(--text-muted)">{formatNumber(value,format.locale,{maximumSignificantDigits:4})}</text></g>;})}
        {rows.map((row)=>{
          const segments:{path:string;first:number;last:number;value:number}[]=[];let previous:number | null=null;
          for(const point of row.points){const at=Date.parse(point.at),value=creditCoordinate(point,row);if(value===null){previous=null;continue;}const segment=segments.at(-1);if(previous===null || point.gap_before || segment===undefined)segments.push({path:`M${x(at)},${y(value)}`,first:at,last:at,value});else {segment.path+=` L${x(at)},${y(value)}`;segment.last=at;}previous=at;}
          return <g key={row.series_id} data-credit-series={row.series_id}>{segments.map((segment,index)=><g key={index}>{segment.first===segment.last ? <circle cx={x(segment.first)} cy={y(segment.value)} r={2.5} fill={`var(--chart-${SLOTS[row.quantity]})`} /> : <path data-credit-segment d={segment.path} stroke={`var(--chart-${SLOTS[row.quantity]})`} strokeWidth={2} strokeDasharray={row.quantity==='limit' ? '4 3' : undefined} fill="none" />}</g>)}</g>;
        })}
        {TICKS.map((fraction)=><text key={fraction} x={x(from+(to-from)*fraction)} y={HEIGHT-5} textAnchor={fraction===0 ? 'start' : fraction===1 ? 'end' : 'middle'} fontSize={FONT} fill="var(--text-muted)">{when(from+(to-from)*fraction)}</text>)}
      </svg>
      {values.length===0 && <p className="absolute inset-x-0 top-10 text-center text-sm text-text-muted">{t('providerCredits.noAmounts')}</p>}
      {hover!==null && <span role="status" className="pointer-events-none absolute inset-x-0 top-5 z-10 w-fit max-w-full rounded border border-border bg-surface-raised px-2 py-1 text-xs text-text shadow-sm">{hover}</span>}
    </div>}
  </div>;
}
