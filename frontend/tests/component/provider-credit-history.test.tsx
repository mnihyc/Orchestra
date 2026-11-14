/** Real provider history helpers and current projections share one panel without converting vendor amounts into Orch costs. */
import { act,fireEvent,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useQuery } from '@tanstack/react-query';
import { afterEach,beforeEach,expect,it,vi } from 'vitest';
import type * as Client from '@/api/client';
import type * as RouterModule from '@tanstack/react-router';
import type {AnchorHTMLAttributes,ReactNode} from 'react';
import type { Provider,ProviderCreditSeries,ProviderPlanSeries } from '@/generated/api-types';
import { providerQuery } from '@/api/resources';
import { Plan } from '@/features/usage/Plan';
import { CreditChart } from '@/features/usage/CreditChart';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { i18n,setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const transport=vi.hoisted(()=>({get:vi.fn<(_path:string,options:{params:{query:{metric?:string}}})=>Promise<ReturnType<typeof answer>>>(),post:vi.fn()}));
vi.mock('@/api/client',async(load)=>{const actual=await load<typeof Client>();return {...actual,api:{...actual.api,GET:transport.get,POST:transport.post}};});
vi.mock('@tanstack/react-router',async(load)=>({...await load<typeof RouterModule>(),Link:({to,params,search,hash,children,...anchor}:AnchorHTMLAttributes<HTMLAnchorElement> & {to:string;params?:Record<string,string>;search?:Record<string,string>;hash?:string;children?:ReactNode})=><a {...anchor} href={`${to.replace(/\$(\w+)/g,(_,key:string)=>params?.[key] ?? '')}?${new URLSearchParams(search)}${hash===undefined ? '' : `#${hash}`}`}>{children}</a>}));
vi.mock('@/features/help/open',async (loadOriginal) => ({ ...await loadOriginal<object>(),useHelpOpener:()=>vi.fn()}));
const span={from:'2026-11-01',to:'2026-11-03',days:3,range:'custom' as const};
const at='2026-11-03T12:00:00Z';
function row(patch:Partial<ProviderCreditSeries>={}):ProviderCreditSeries {
  return {kind:'credits',series_id:'balance',group_id:'balance-unit',member:'balances',item_id:'plan',quantity:'remaining',label:'Plan credits',unit:'credits',decimal_places:null,points:[{at,amount:'9007199254740993.123456789',unlimited:false,enabled:null,gap_before:false}],...patch};
}
function subscription():ProviderPlanSeries {return {window_id:'primary',starts_at:at,points:[],history:[],resets:[],current_reset:null,summary:{mean_used_percent:null,limit_buckets:0,early_resets:0}};}
function answer(data:unknown){return {data,response:new Response(null,{status:200})};}
beforeEach(()=>{transport.get.mockReset();transport.post.mockReset();transport.get.mockImplementation((_path:string,options:{params:{query:{metric?:string}}})=>Promise.resolve(answer({items:options.params.query.metric==='credits' ? [row()] : [subscription()],next_cursor:null})));});
afterEach(async()=>{await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)it(`shares one current summary and history area, keeping credit text exact (${locale})`,async()=>{
  await setLocale(locale);
  const provider=planProvider();
  provider.facts_state.fields={'balances.plan.total':'unsupported','balances.plan.used':'unsupported'};
  const view=show(<Plan provider={provider} span={span} mode="history" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />);
  await waitFor(()=>expect(transport.get).toHaveBeenCalledTimes(1));
  expect(view.container.querySelectorAll('[data-provider-current]')).toHaveLength(1);
  await userEvent.setup().click(screen.getByRole('radio',{name:i18n.t('providerCredits.credits')}));
  await waitFor(()=>expect(view.container.querySelector('[data-provider-history="credits"]')).toBeInTheDocument());
  expect(view.container.querySelector('[data-provider-history="subscription"]')).not.toBeInTheDocument();
  expect(screen.getByText(/9007199254740993\.123456789 credits/)).toBeVisible();
  const call=transport.get.mock.calls.at(-1);
  expect(call?.[0]).toBe('/providers/{id}/plan');
  expect(call?.[1].params.query).toMatchObject({metric:'credits',from:span.from,to:span.to,bucket:'day'});
  fireEvent.focus(screen.getByRole('img',{name:'Plan credits · credits'}));
  expect(screen.getByRole('status')).toHaveTextContent('9007199254740993.123456789 credits');
});

it('hides unsupported fields while preserving supported missing values and the current override marker',()=>{
  const provider=planProvider();
  provider.facts_state.fields={'balances.plan.total':'unsupported','balances.plan.used':'unsupported'};
  provider.facts_overridden=['balances'];
  const view=show(<ProviderUsageCard provider={provider} />);
  const balance=screen.getByRole('group',{name:'Balance'});
  expect(balance).toHaveTextContent('12.4 vendor credits remaining');
  expect(balance).not.toHaveTextContent('Not reported');
  expect(balance).toHaveTextContent('Manually set');
  expect(within(balance).getByRole('link',{name:'Over time'})).toHaveAttribute('href',expect.stringContaining('plan_metric=credits'));
  provider.facts_state.fields['balances.plan.used']='empty';
  view.rerender(<ProviderUsageCard provider={structuredClone(provider)} />);
  expect(screen.getByRole('group',{name:'Balance'})).toHaveTextContent('Used: Not reported');
});

it('keeps unsupported credit reasons visible without offering or fetching credit history',async()=>{
  const provider=planProvider();
  provider.facts_state.balances='unsupported';provider.facts_state.spend='unsupported';provider.facts_state.extra_usage='unsupported';
  const view=show(<Plan provider={provider} span={span} mode="current" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />);
  await waitFor(()=>expect(transport.get).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole('radio',{name:i18n.t('providerCredits.credits')})).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-current-member="balances"]')).toHaveTextContent(i18n.t('providerDetails.notSupplied'));
  expect(transport.get.mock.calls.every(([,options])=>options.params.query.metric!=='credits')).toBe(true);
});

it('shows scaled paid usage and its spending limit without inventing a balance',()=>{
  const provider=planProvider();
  provider.facts.balances=[];provider.facts_state.balances='unsupported';provider.facts_state.spend='unsupported';
  provider.facts.extra_usage={enabled:true,used:'1234.56789',limit:'2000',used_percent:61.7,unit:'USD',decimal_places:2,disabled_reason:null,limit_reached:false};provider.facts_state.extra_usage='told';
  show(<ProviderUsageCard provider={provider} />);
  const extra=screen.getByRole('group',{name:'Beyond the plan'});
  expect(extra).toHaveTextContent('12.3456789 USD used of 20 USD');
  expect(extra).not.toHaveTextContent(/remaining|balance/i);
  expect(within(extra).getByRole('meter')).toHaveAttribute('aria-valuenow','62');
  expect(within(extra).getByRole('meter')).toHaveAttribute('aria-valuetext','61.7%');
});

it('breaks a credit line at actual missing/unlimited observations and retains known zero',()=>{
  const series=row({points:[
    {at:'2026-11-03T10:00:00Z',amount:'3',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:05:00Z',amount:'2',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:10:00Z',amount:null,unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:15:00Z',amount:'1',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:20:00Z',amount:'0',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:25:00Z',amount:null,unlimited:true,enabled:null,gap_before:false},
  ]});
  const view=show(<CreditChart series={[series]} from={Date.parse('2026-11-03T10:00:00Z')} to={Date.parse('2026-11-03T10:30:00Z')} />);
  expect(view.container.querySelectorAll('[data-credit-segment]')).toHaveLength(2);
  expect(screen.getByText('Credit balance: Unlimited')).toBeVisible();
  const chart=screen.getByRole('img');fireEvent.focus(chart);fireEvent.keyDown(chart,{key:'ArrowLeft'});
  expect(screen.getByRole('status')).toHaveTextContent('Credit balance: 0 credits');
});

function ObservedPlan({provider}:{provider:Provider}) {
  const read=useQuery({...providerQuery(provider.id),initialData:provider,enabled:false});
  return <Plan provider={read.data} span={span} metric="credits" mode="history" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />;
}

it('refreshes current facts and the selected history once through the shared source clock',async()=>{
  const provider=planProvider();const fresh={...provider,facts_at:'2026-11-03T12:01:00Z',revision:provider.revision+1};
  transport.post.mockResolvedValue(answer(fresh));
  show(<ObservedPlan provider={provider} />);
  await waitFor(()=>expect(transport.get).toHaveBeenCalledTimes(1));
  await userEvent.setup().click(screen.getByRole('button',{name:'Refresh'}));
  await waitFor(()=>expect(transport.get).toHaveBeenCalledTimes(2));
  expect(transport.post).toHaveBeenCalledTimes(1);
  expect(transport.post.mock.calls[0]?.[0]).toBe('/providers/{id}/refresh');
  expect(transport.get.mock.calls.every(([,options])=>options.params.query.metric==='credits')).toBe(true);
});

it('honours an omitted missing marker without connecting the two observed runs',()=>{
  const series=row({points:[
    {at:'2026-11-03T10:00:00Z',amount:'5',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:05:00Z',amount:'4',unlimited:false,enabled:null,gap_before:false},
    {at:'2026-11-03T10:15:00Z',amount:'3',unlimited:false,enabled:null,gap_before:true},
    {at:'2026-11-03T10:20:00Z',amount:'2',unlimited:false,enabled:null,gap_before:false},
  ]});
  const view=show(<CreditChart series={[series]} from={Date.parse('2026-11-03T10:00:00Z')} to={Date.parse('2026-11-03T10:30:00Z')} />);
  expect(view.container.querySelectorAll('[data-credit-segment]')).toHaveLength(2);
});


it('keeps a pending source refresh shared while switching the displayed metric',async()=>{
  const provider=planProvider();
  let finish:((value:ReturnType<typeof answer>)=>void) | undefined;
  transport.post.mockImplementation(()=>new Promise<ReturnType<typeof answer>>((resolve)=>{finish=resolve;}));
  show(<Plan provider={provider} span={span} mode="history" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />);
  await waitFor(()=>expect(transport.get).toHaveBeenCalledTimes(1));
  const user=userEvent.setup();
  await user.click(screen.getByRole('button',{name:'Refresh'}));
  expect(await screen.findByRole('button',{name:'Reading…'})).toHaveAttribute('aria-disabled','true');
  await user.click(screen.getByRole('radio',{name:'Credits / paid usage'}));
  expect(screen.getByRole('button',{name:'Reading…'})).toHaveAttribute('aria-disabled','true');
  expect(transport.post).toHaveBeenCalledTimes(1);
  const complete=finish;
  if(complete===undefined)throw new Error('The source refresh was not started');
  await act(async()=>{complete(answer(provider));await Promise.resolve();});
  await waitFor(()=>expect(screen.getByRole('button',{name:'Refresh'})).toBeEnabled());
});


it('keeps generic status-only facts while hiding only fully unsupported amount history',async()=>{
  transport.get.mockResolvedValue(answer({items:[],next_cursor:null}));
  const provider=planProvider();
  provider.facts_state.windows='unsupported';provider.facts_state.balances='unsupported';provider.facts_state.spend='unsupported';provider.facts_state.extra_usage='told';
  provider.facts.extra_usage={enabled:false,used:null,limit:null,used_percent:null,unit:'points',decimal_places:null,disabled_reason:null,limit_reached:null};
  provider.facts_state.fields={'extra_usage.enabled':'told','extra_usage.unit':'told','extra_usage.used':'unsupported','extra_usage.limit':'unsupported'};
  const render=()=> <Plan provider={structuredClone(provider)} span={span} mode="current" bucket="day" onMode={vi.fn()} onBucket={vi.fn()} />;
  const view=show(render());
  expect(view.container.querySelector('[data-current-member="extra_usage"]')).toHaveTextContent(i18n.t('providerPlan.extraOff'));
  expect(screen.getByRole('button',{name:'Refresh'})).toBeEnabled();
  expect(screen.queryByText(i18n.t('providerCredits.history'))).not.toBeInTheDocument();
  expect(transport.get).not.toHaveBeenCalled();
  // A supported missing amount, or older metadata, is still an available unknown measurement.
  provider.facts_state.fields['extra_usage.used']='empty';
  view.rerender(render());
  expect(screen.getByText(i18n.t('providerCredits.history'))).toBeVisible();
  await waitFor(()=>expect(screen.getByText(i18n.t('planUsage.noReadings'))).toBeVisible());
  expect(transport.get).toHaveBeenCalledTimes(1);
  provider.facts_state.fields={};
  view.rerender(render());
  expect(screen.getByText(i18n.t('providerCredits.history'))).toBeVisible();
  // A partial field map never turns a missing remaining-state entry into unsupported balance history.
  provider.facts_state.extra_usage='unsupported';provider.facts_state.balances='told';
  provider.facts_state.fields={'balances.plan.used':'unsupported','balances.plan.total':'unsupported'};
  view.rerender(render());
  expect(screen.getByText(i18n.t('providerCredits.history'))).toBeVisible();
  // Reported unlimited is a real balance representation even without a numeric amount.
  provider.facts.balances[0]!.remaining=null;provider.facts.balances[0]!.unlimited=true;
  provider.facts_state.fields={'balances.plan.used':'unsupported','balances.plan.total':'unsupported','balances.plan.remaining':'unsupported','balances.plan.unlimited':'told'};
  view.rerender(render());
  expect(screen.getByText(i18n.t('providerCredits.history'))).toBeVisible();
  expect(view.container.querySelector('[data-current-member="balances"]')).toHaveTextContent(i18n.t('providerPlan.unlimited'));
  provider.facts.balances[0]!.unlimited=null;provider.facts_state.fields['balances.plan.unlimited']='unsupported';
  view.rerender(render());
  expect(screen.queryByText(i18n.t('providerCredits.history'))).not.toBeInTheDocument();
});


it.each([
  {amount:'1234.56789',shown:'12.3456789 USD'},
  {amount:null,shown:'Not reported'},
])('shows every quantity observed at the pointed time, including $shown',({amount,shown})=>{
  const used=row({series_id:'paid',group_id:'paid-usd',member:'extra_usage',item_id:'',quantity:'used',label:'Paid usage',unit:'USD',decimal_places:2,
    points:[{at,amount,enabled:true,unlimited:false,gap_before:false}]});
  const limit=row({...used,series_id:'limit',quantity:'limit',points:[{at,amount:'2000',enabled:true,unlimited:false,gap_before:false}]});
  const later=row({...used,series_id:'later',quantity:'remaining',points:[{at:'2026-11-03T12:01:00Z',amount:'9999',enabled:true,unlimited:false,gap_before:false}]});
  const view=show(<CreditChart series={[used,limit,later]} from={Date.parse(at)} to={Date.parse('2026-11-03T12:02:00Z')} />);
  const marker=view.container.querySelector('[data-credit-series="limit"] circle');
  if(marker===null)throw new Error('The reported spending limit needs its actual point marker');
  const chart=screen.getByRole('img');
  fireEvent.pointerMove(chart,{clientX:Number(marker.getAttribute('cx'))+chart.getBoundingClientRect().left});
  const tooltip=screen.getByRole('status');
  expect(tooltip).toHaveTextContent(`Paid usage: ${shown}`);
  expect(tooltip).toHaveTextContent('Spending limit: 20 USD');
  expect(tooltip).not.toHaveTextContent('99.99 USD');
  fireEvent.pointerLeave(chart);
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
});
