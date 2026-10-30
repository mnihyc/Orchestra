/** Scope charts preserve request counts, token composition, missing observations and the meaning of plan forecasts. */
import { act,screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import type * as RouterModule from '@tanstack/react-router';
import type { ProviderPlanSeries, UsageAnalytics, UsageFigures } from '@/generated/api-types';
import { ApiError } from '@/api/client';
import { Figures,TokenLine } from '@/features/usage/Figures';
import { UsageChart } from '@/features/usage/Chart';
import { ShareBar } from '@/features/usage/Breakdowns';
import { ForecastLine } from '@/features/usage/ForecastLine';
import { Plan,PlanCard } from '@/features/usage/Plan';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { barsOf } from '@/features/usage/series';
import { i18n } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { performanceOf,timingFixture } from '../doubles/performance';
import { seedPlan,planReply } from '../doubles/plan-usage';
import { show } from './support';

const NOW=Date.parse('2026-10-28T12:00:00Z');
const planRead=vi.hoisted(()=>vi.fn<()=>Promise<{items:ProviderPlanSeries[];next_cursor:string | null}>>());
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/api/planUsage',()=>({PLAN_PAGE_SIZE:20,planUsageQuery:(id:string,from:string,to:string,bucket:string,at:string | null,cursor?:string)=>({queryKey:['chart-plan',id,from,to,bucket,at,cursor],staleTime:Infinity,queryFn:planRead})}));
vi.mock('@tanstack/react-router',async(load)=>({...await load<typeof RouterModule>(),Link:({to,params,search,hash,children,...anchor}:AnchorHTMLAttributes<HTMLAnchorElement> & {to:string;params?:Record<string,string>;search?:Record<string,string>;hash?:string;children?:ReactNode})=><a {...anchor} href={`${to.replace(/\$(\w+)/g,(_,key:string)=>params?.[key] ?? '')}?${new URLSearchParams(search)}${hash===undefined ? '' : `#${hash}`}`}>{children}</a>}));
beforeEach(()=>{vi.spyOn(Date,'now').mockReturnValue(NOW);planRead.mockReset();});
afterEach(()=>vi.restoreAllMocks());

function figures(requests=4812):UsageFigures {
  return {turns:120,tokens:{input_tokens:1000,cached_input_tokens:600,cache_write_tokens:200,output_tokens:400,reasoning_tokens:100,total_tokens:1400},credits:'1',estimated_credits:'0',unrated_tokens:0,performance:{...performanceOf([timingFixture()]),requests}};
}
function analytics():UsageAnalytics {
  const total=figures();
  return {scope:'me',from:'2026-10-27',to:'2026-10-28',bucket:'day',depth:'own',me:null,totals:total,series:[{at:'2026-10-28T00:00:00Z',figures:total,by:[]}],breakdowns:[]};
}
function seeded() {
  const provider=planProvider();
  const seed=seedPlan(provider.facts,NOW);
  provider.facts=seed.facts;provider.facts_at=seed.at;provider.plan_forecasts=seed.forecasts;
  return {provider,seed};
}
const span={from:'2026-10-21',to:'2026-10-28',days:8,range:'7d' as const};

it('counts completed requests separately from turns in the tile and series',()=>{
  show(<Figures totals={figures()} previous={null} before="" />);
  expect(screen.getByText('4,812 requests')).toBeInTheDocument();
  expect(screen.getByText('120')).toBeInTheDocument();
  expect(barsOf(analytics(),'requests','none',i18n.t).heights).toEqual([4812]);
});

it('shows the same five disjoint token parts in the scope line and stacks, with the cache hit rate',()=>{
  const total=figures();
  const chart=barsOf(analytics(),'tokens','token_kind',i18n.t);
  expect(chart.stacks.map((stack)=>stack.values[0])).toEqual([200,600,200,300,100]);
  expect(chart.heights).toEqual([total.tokens.total_tokens]);
  const rendered=show(<TokenLine tokens={total.tokens} />);
  expect(screen.getByText('Cache hit 60%')).toBeInTheDocument();
  expect(rendered.container.querySelector('[data-part="cache_write"]')).toBeInTheDocument();
  expect(screen.getByRole('img',{name:'Written to the cache 200 (14%)'})).toBeInTheDocument();
});

it('draws one error-rate line without a split and keeps an unobserved bucket as a gap',async()=>{
  const data=analytics();
  const samples=[performanceOf([timingFixture()]),performanceOf([]),performanceOf([timingFixture(),timingFixture({outcome:{state:'told',value:'provider_busy'}})])];
  data.series=samples.map((performance,index)=>({at:`2026-10-${25+index}T00:00:00Z`,figures:{...figures(),performance},by:[]}));
  const bars=barsOf(data,'errors','none',i18n.t);
  expect(bars.lines).toBe(true);expect(bars.stacks).toHaveLength(1);
  expect(bars.stacks[0]?.values[0]).toBe(0);
  expect(bars.stacks[0]?.values[1]).toBeNaN();
  expect(bars.stacks[0]?.values[2]).toBe(0.5);
  show(<UsageChart label="Errors over time" bars={bars} measure="errors" empty="No observations" />);
  await userEvent.click(screen.getByRole('button',{name:'Show as table'}));
  const rows=within(screen.getByRole('table',{name:'Errors over time: Show as table'})).getAllByRole('row');
  expect(rows[2]).toHaveTextContent('—');
  expect(rows[2]).not.toHaveTextContent('0.0%');
  expect(rows[3]).toHaveTextContent('50.0%');
});

it('uses the whole-table share and the matching series colour, or the neutral colour',()=>{
  const shown=show(<ShareBar share={0.25} slot={3} />);
  expect(screen.getByText('25%')).toBeInTheDocument();
  expect(shown.container.querySelector('[style]')).toHaveStyle({width:'25%',background:'var(--chart-3)'});
  shown.rerender(<ShareBar share={0.5} />);
  expect(shown.container.querySelector('[style]')).toHaveStyle({width:'50%',background:'var(--text-subtle)'});
});

it('omits valueless models from the legend while preserving unknown cost in the data table',async()=>{
  const data=analytics();
  const priced=figures();
  const unpriced={...figures(),credits:'0',unrated_tokens:1400};
  const point=data.series[0];
  if(point===undefined)throw new Error('the fixture bucket is missing');
  point.by=[{key:{model:'priced',model_display:'Priced'},figures:priced},{key:{model:'unpriced',model_display:'Unpriced'},figures:unpriced}];
  show(<UsageChart label="Credits over time" bars={barsOf(data,'credits','model',i18n.t)} measure="credits" empty="No observations" />);
  const legend=screen.getByRole('list',{name:i18n.t('usage.chart.legend')});
  expect(legend).toHaveTextContent('Priced');expect(legend).not.toHaveTextContent('Unpriced');
  await userEvent.click(screen.getByRole('button',{name:'Show as table'}));
  const table=screen.getByRole('table',{name:'Credits over time: Show as table'});
  expect(within(table).getByRole('columnheader',{name:'Unpriced'})).toBeInTheDocument();
  expect(table).toHaveTextContent('–');
});

it.each([['five','runs_out'],['week','at_reset'],['reached','reached'],['stale','stale'],['insufficient','insufficient']] as const)('says what the %s plan card can project', (id,state)=>{
  const {provider,seed}=seeded();
  const chosen=id==='insufficient' ? 'five' : id;
  if(id==='insufficient')provider.plan_forecasts={};
  const window=provider.facts.windows.find((window)=>window.id===chosen);
  if(window===undefined)throw new Error('the fixture window is missing');
  const rendered=show(<PlanCard provider={provider} window={window} series={seed.rows.find((row)=>row.window_id===chosen)} mode="current" span={span} bucket="day" />);
  expect(rendered.container.querySelector(`[data-forecast-state="${state}"]`)).toBeInTheDocument();
  expect(rendered.container.querySelector('path[data-forecast-line]')!==null).toBe(state==='runs_out' || state==='at_reset' || state==='reached');
});

it('shows history as bucket usage with even pace and an audit link for an outside reset',()=>{
  const {provider,seed}=seeded();
  const window=provider.facts.windows.find((window)=>window.id==='outside');
  if(window===undefined)throw new Error('the fixture window is missing');
  const rows=planReply(seed,provider.facts.windows,span.from,span.to,'day');
  const rendered=show(<PlanCard provider={provider} window={window} series={rows.find((row)=>row.window_id===window.id)} mode="history" span={span} bucket="day" />);
  expect(rendered.container.querySelector('[data-plan-chart="history"]')).toBeInTheDocument();
  expect(screen.getByText(/Even pace/)).toBeInTheDocument();
  expect(screen.getByText(/a day on average/)).toHaveTextContent('1 early reset');
  expect(screen.getByRole('link',{name:/A reset was spent outside Orch/})).toHaveAttribute('href',expect.stringContaining('provider.window_started_over'));
  expect(rendered.container.querySelector('[data-forecast-line]')).not.toBeInTheDocument();
});

it('uses the same forecast line below the overview meter and a period cap',()=>{
  const {provider}=seeded();
  const shown=show(<ProviderUsageCard provider={provider} />);
  const meter=screen.getByRole('meter',{name:'5-hour window'});
  expect(meter.closest('li')?.querySelector('[data-forecast-state="runs_out"]')).toBeInTheDocument();
  expect(screen.getByRole('link',{name:'Over time'})).toHaveAttribute('href',expect.stringContaining('#usage-plan'));
  shown.unmount();
  show(<ForecastLine cap usedPercent={20} resetsAt={new Date(NOW+12*3_600_000).toISOString()} forecast={{basis:'period',percent_at_end:80,runs_out_at:null}} />);
  expect(screen.getByText(/At this period's pace: about 80 % by/)).toBeInTheDocument();
});

it('adds one plan read only when visible and refreshes it for a new facts reading, not for the mode',async()=>{
  const {provider,seed}=seeded();planRead.mockResolvedValue({items:seed.rows,next_cursor:null});
  const hidden={...provider,access:{...provider.access,permissions:['provider.use']}};
  const onMode=vi.fn(),onBucket=vi.fn();
  const rendered=show(<Plan provider={hidden} span={span} mode="current" bucket="auto" onMode={onMode} onBucket={onBucket} />);
  expect(planRead).not.toHaveBeenCalled();
  rendered.rerender(<Plan provider={provider} span={span} mode="current" bucket="auto" onMode={onMode} onBucket={onBucket} />);
  await waitFor(()=>expect(planRead).toHaveBeenCalledTimes(1));
  rendered.rerender(<Plan provider={provider} span={span} mode="history" bucket="auto" onMode={onMode} onBucket={onBucket} />);
  expect(planRead).toHaveBeenCalledTimes(1);
  rendered.rerender(<Plan provider={{...provider,facts_at:new Date(NOW+60_000).toISOString()}} span={span} mode="history" bucket="auto" onMode={onMode} onBucket={onBucket} />);
  await waitFor(()=>expect(planRead).toHaveBeenCalledTimes(2));
});

it('pages a thousand source windows on demand and mounts charts only near the viewport',async()=>{
  const {provider,seed}=seeded();
  const first=provider.facts.windows[0],series=seed.rows[0];
  if(first===undefined || series===undefined)throw new Error('the fixture window is missing');
  provider.facts.windows=Array.from({length:1000},(_,index)=>({...first,id:`window-${index}`,label:`Window ${index}`}));
  const page=(offset:number)=>({items:provider.facts.windows.slice(offset,offset+20).map((window)=>({...series,window_id:window.id})),next_cursor:offset===0 ? 'page-two' : null});
  planRead.mockResolvedValueOnce(page(0)).mockResolvedValueOnce(page(20));
  const callbacks=new Map<Element,IntersectionObserverCallback>();
  class Observer {
    constructor(private callback:IntersectionObserverCallback){}
    observe(element:Element){callbacks.set(element,this.callback);}
    disconnect(){for(const [element,callback] of callbacks)if(callback===this.callback)callbacks.delete(element);}
  }
  vi.stubGlobal('IntersectionObserver',Observer);
  try {
    const shown=show(<Plan provider={provider} span={span} mode="current" bucket="auto" onMode={vi.fn()} onBucket={vi.fn()} />);
    await waitFor(()=>expect(screen.getByRole('button',{name:'Next windows'})).toBeEnabled());
    expect(planRead).toHaveBeenCalledTimes(1);
    expect(shown.container.querySelectorAll('[data-plan-window]')).toHaveLength(20);
    expect(shown.container.querySelectorAll('[data-plan-chart]')).toHaveLength(0);
    const card=shown.container.querySelector('[data-plan-window]');
    if(card===null)throw new Error('the first card is missing');
    const notify=(visible:boolean)=>callbacks.get(card)?.([{target:card,isIntersecting:visible} as IntersectionObserverEntry],{} as IntersectionObserver);
    act(()=>notify(true));expect(shown.container.querySelectorAll('[data-plan-chart]')).toHaveLength(1);
    act(()=>notify(false));expect(shown.container.querySelectorAll('[data-plan-chart]')).toHaveLength(0);
    await userEvent.click(screen.getByRole('button',{name:'Next windows'}));
    await waitFor(()=>expect(screen.getByRole('article',{name:'Window 20'})).toBeInTheDocument());
    expect(planRead).toHaveBeenCalledTimes(2);
    expect(shown.container.querySelectorAll('[data-plan-window]')).toHaveLength(20);
    expect(screen.queryByRole('article',{name:'Window 0'})).not.toBeInTheDocument();
  } finally {vi.unstubAllGlobals();}
});

it('keeps ten billion aggregate tokens exact without expanding their thousand threads',()=>{
  const data=analytics();
  data.totals={...figures(1_000_000),turns:1000,tokens:{input_tokens:8_000_000_000,cached_input_tokens:2_000_000_000,cache_write_tokens:1_000_000_000,output_tokens:2_000_000_000,reasoning_tokens:500_000_000,total_tokens:10_000_000_000}};
  data.series=[{at:'2026-10-28T00:00:00Z',figures:data.totals,by:[]}];
  const bars=barsOf(data,'tokens','token_kind',i18n.t);
  expect(bars.stacks).toHaveLength(5);
  expect(bars.heights).toEqual([10_000_000_000]);
  expect(bars.stacks.reduce((sum,stack)=>sum+(stack.values[0] ?? 0),0)).toBe(10_000_000_000);
  expect(barsOf(data,'requests','none',i18n.t).heights).toEqual([1_000_000]);
});

it('reopens an expired page with a fresh first read even before its source clock updates',async()=>{
  const {provider,seed}=seeded();
  const first=seed.rows[0];if(first===undefined)throw new Error('the fixture window is missing');
  planRead.mockResolvedValueOnce({items:[first],next_cursor:'expired'}).mockRejectedValueOnce(new ApiError({type:'orch:error:request.invalid',title:'Invalid request',status:400,code:'request.invalid',detail:'The plan changed',request_id:'expired-page',errors:[{field:'cursor',message:'the plan changed; reopen its first page'}]},null)).mockResolvedValueOnce({items:[first],next_cursor:null});
  show(<Plan provider={provider} span={span} mode="current" bucket="auto" onMode={vi.fn()} onBucket={vi.fn()} />);
  await userEvent.click(await screen.findByRole('button',{name:'Next windows'}));
  await screen.findByRole('alert');
  const review=screen.queryByRole('button',{name:i18n.t('ui.error.review')});
  if(review!==null)await userEvent.click(review);
  await userEvent.click(await screen.findByRole('button',{name:'Try again'}));
  await waitFor(()=>expect(planRead).toHaveBeenCalledTimes(3));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});
