/** Unpriced service requests remain visible in the actual limits card, without token equivalents. */
import {cleanup,screen,within} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import userEvent from '@testing-library/user-event';
import type * as UsageApi from '@/api/usage';
import type {CapStatus} from '@/generated/api-types';
import {Limits} from '@/features/usage/Limits';
import {i18n,setLocale} from '@/i18n';
import {useAppearance} from '@/lib/appearance';
import {formatExact} from '@/lib/time';
import {show} from './support';

const fixture=vi.hoisted(()=>({unpriced:0,rows:null as CapStatus[]|null}));
vi.mock('@/api/usage',async(original)=>({...await original<typeof UsageApi>(),myLimitsQuery:{queryKey:['service-limit-coverage'],queryFn:()=>Promise.resolve(fixture.rows??[status()])}}));
function status():CapStatus {
  return {cap:{id:'service-credit-cap',node:null,provider:null,subject:{kind:'everyone'},split:'shared',models:[],services:['@lookup'],purposes:['web_search'],
    tiers:[],harness_kinds:[],meter:'credits',amount:'10',period:'week',counted_from:null,at_limit:'stop',warn_percent:80,uncounted:'allow',note:'',enabled:true,revision:1,
    created_by:{id:'creator',kind:'human',username:'ada',display_name:'Ada',avatar:{text:'A',color:1}},created_at:'2026-10-01T00:00:00Z',updated_at:'2026-10-01T00:00:00Z',can_edit:false},
    person:null,used:'0',in_progress:'0',waiting:0,left_out:{unrated_tokens:0,unmetered_threads:0,unrated_services:fixture.unpriced},state:'ok',period_ends_at:null,
    replaced_by:null,can_bypass:false,bypasses:{count:0,last:null}};
}
const appearance=useAppearance.getState();
afterEach(async()=>{cleanup();useAppearance.setState(appearance);await setLocale('en');fixture.unpriced=0;fixture.rows=null;});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it(`names the allowance that replaces another and its UTC measurement and bypass rules (${locale}/${theme})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale,theme,resolvedTheme:theme,timeZone:'Asia/Tokyo'});
  const fallback=status(),personal=status();
  fallback.cap.split='each';fallback.person=fallback.cap.created_by;fallback.replaced_by='personal';
  personal.cap={...personal.cap,id:'personal',split:'shared',subject:{kind:'user',user:personal.cap.created_by},amount:'20',uncounted:'block'};
  personal.person=personal.cap.created_by;personal.period_ends_at='2026-11-02T00:00:00.000Z';
  personal.bypasses={count:1,last:{at:'2026-11-01T10:00:00.000Z',by:personal.cap.created_by}};
  fixture.rows=[fallback,personal];
  show(<Limits of={{kind:'mine'}}/>);
  const replacement=await screen.findByText(locale==='en'?'Replaced by':'由此限额替代');
  expect(within(replacement.parentElement!).getByRole('button',{name:'Ada'})).toBeVisible();
  expect(screen.getByText(locale==='en'?'Personal allowance takes priority':'个人限额优先')).toBeVisible();
  expect(screen.getAllByText(locale==='en'?'Stop at limit · Unmeasured use blocked':'达到限额时停止 · 无法计量时阻止')).toHaveLength(2);
  const reset=screen.getByText(i18n.t('caps.startsAgain',{when:formatExact(personal.period_ends_at,{locale,timeZone:'UTC'})}));
  expect(reset).toHaveAttribute('datetime',personal.period_ends_at);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('caps.row.passed',{count:1})}));
  expect(await screen.findByText(locale==='en'?'Rechecked at turn start':'轮次开始时重新核验')).toBeVisible();
});

it.each(['allow','block'] as const)('says whether unmeasured work is %s while a current turn may finish',async(uncounted)=>{
  const cap=status();cap.cap.at_limit='finish_turn';cap.cap.uncounted=uncounted;fixture.rows=[cap];
  show(<Limits of={{kind:'mine'}}/>);
  expect(await screen.findByText(`Finish current turn · Unmeasured use ${uncounted==='allow'?'allowed':'blocked'}`)).toBeVisible();
});

it('does not claim stopping or measurement enforcement for an unlimited allowance',async()=>{
  const cap=status();cap.cap.amount=null;cap.cap.uncounted='block';fixture.rows=[cap];
  show(<Limits of={{kind:'mine'}}/>);
  await screen.findByText('Ada',{exact:true});
  expect(screen.queryByText(/Stop at limit|Unmeasured use blocked/u)).not.toBeInTheDocument();
});
for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it(`shows actual unpriced service requests in Limits (${locale}/${theme})`,async()=>{
  await setLocale(locale);useAppearance.setState({locale,theme,resolvedTheme:theme});fixture.unpriced=12345;
  show(<Limits of={{kind:'mine'}}/>);
  const expected=locale==='en'?'12,345 unpriced requests':'12,345 次请求未计价';
  expect(await screen.findByText(expected,{exact:true})).toBeVisible();
  const section=screen.getByRole('region',{name:locale==='en'?'Limits':'限额'});
  expect(within(section).queryByText(/12,345.*(?:tokens|令牌)/)).not.toBeInTheDocument();
});
it('does not add an unpriced-service warning to a fully counted ordinary cap',async()=>{
  fixture.unpriced=0;show(<Limits of={{kind:'mine'}}/>);
  await screen.findByText('Ada',{exact:true});
  expect(screen.queryByText(/unpriced requests/)).not.toBeInTheDocument();
});
