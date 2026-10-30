/** Every usage door reads from figures through charts and limits to breakdowns and records. */
import { screen } from '@testing-library/react';
import { expect,it,vi } from 'vitest';
import type { AnchorHTMLAttributes,ReactNode } from 'react';
import type * as RouterModule from '@tanstack/react-router';
import type * as UsageApi from '@/api/usage';
import type { EventParams,UsageParams } from '@/api/usage';
import { Report } from '@/features/usage/Report';
import { ProviderUsage } from '@/features/resources/providers/Usage';
import { Section } from '@/shell/Page';
import { me,org,userRef } from '../doubles/fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router',async(load)=>({...await load<typeof RouterModule>(),Link:({to,params,children,...anchor}:AnchorHTMLAttributes<HTMLAnchorElement> & {to:string;params?:Record<string,string>;search?:unknown;hash?:unknown;children?:ReactNode})=><a {...anchor} href={to.replace(/\$(\w+)/g,(_,key:string)=>params?.[key] ?? '')}>{children}</a>}));
vi.mock('@/shell/screen',()=>({useScreenHeading:()=>undefined}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/usage/Export',()=>({Export:()=>null}));
vi.mock('@/features/usage/Limits',()=>({Limits:()=> <Section id="order-limits" title="Limits"><p>One cap</p></Section>}));
vi.mock('@/api/planUsage',()=>({PLAN_PAGE_SIZE:20,planUsageQuery:()=>({queryKey:['order-plan'],queryFn:()=>Promise.resolve({items:[],next_cursor:null})})}));
vi.mock('@/api/nodes',()=>({nodeQuery:(id:string)=>({queryKey:['order-node',id],queryFn:()=>Promise.resolve(org(id,'acme','Acme',userRef(me(1,'ada','Ada'))))}),orgsQuery:{queryKey:['order-orgs'],queryFn:()=>Promise.resolve([])},fetchTree:()=>Promise.resolve({nodes:[]})}));
vi.mock('@/api/resources',()=>({providerQuery:(id:string)=>({queryKey:['order-provider',id],queryFn:()=>Promise.resolve(planProvider())})}));
vi.mock('@/api/usage',async(load)=>({...await load<typeof UsageApi>(),
  eventsQuery:(params:EventParams)=>({queryKey:['order-records',params],initialPageParam:null,queryFn:()=>Promise.resolve({items:[],next_cursor:null}),getNextPageParam:()=>null}),
  usageQuery:(params:UsageParams)=>({queryKey:['order-usage',params],queryFn:()=>{
    const totals={turns:1,tokens:{input_tokens:10,cached_input_tokens:2,output_tokens:5,reasoning_tokens:1,total_tokens:15},credits:'1',estimated_credits:'0',unrated_tokens:0};
    return Promise.resolve({scope:params.scope,from:params.from,to:params.to,bucket:'day',depth:'reports',me:null,totals,series:[{at:`${params.from}T00:00:00Z`,figures:totals,by:[]}],breakdowns:params.breakdowns.map((by)=>({by,more:0,rows:[]}))});
  }}),
}));

it.each(['me','place','provider'] as const)('keeps the summary-to-detail section order at the %s door',async(door)=>{
  const rendered=show(door==='provider' ? <ProviderUsage provider={planProvider()} /> : <Report scope={door==='me' ? {kind:'me'} : {kind:'node',id:'00000000-0000-4000-8000-000000000001'}} />);
  await screen.findByRole('heading',{name:'Records',level:2});
  const headings=screen.getAllByRole('heading',{level:2});
  expect(headings.map((heading)=>heading.textContent)).toEqual([...(door==='provider' ? ['Plan'] : []),'Over time','Limits','Breakdown','Records']);
  const ordered=Array.from(rendered.container.querySelectorAll('ul[aria-label="Figures of the span"],h2'));
  expect(ordered[0]).toBe(screen.getByRole('list',{name:'Figures of the span'}));
  expect(screen.getAllByRole('heading',{name:'Limits',level:2})).toHaveLength(1);
});
