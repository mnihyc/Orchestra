/** Unpriced service requests remain visible in the actual limits card, without token equivalents. */
import {cleanup,screen,within} from '@testing-library/react';
import {afterEach,expect,it,vi} from 'vitest';
import type * as UsageApi from '@/api/usage';
import type {CapStatus} from '@/generated/api-types';
import {Limits} from '@/features/usage/Limits';
import {setLocale} from '@/i18n';
import {useAppearance} from '@/lib/appearance';
import {show} from './support';

const fixture=vi.hoisted(()=>({unpriced:0}));
vi.mock('@/api/usage',async(original)=>({...await original<typeof UsageApi>(),myLimitsQuery:{queryKey:['service-limit-coverage'],queryFn:()=>Promise.resolve([status()])}}));
function status():CapStatus {
  return {cap:{id:'service-credit-cap',node:null,provider:null,subject:{kind:'everyone'},split:'shared',models:[],services:['@lookup'],purposes:['web_search'],
    tiers:[],harness_kinds:[],meter:'credits',amount:'10',period:'week',counted_from:null,at_limit:'stop',warn_percent:80,uncounted:'allow',note:'',enabled:true,revision:1,
    created_by:{id:'creator',kind:'human',username:'ada',display_name:'Ada',avatar:{text:'A',color:1}},created_at:'2026-10-01T00:00:00Z',updated_at:'2026-10-01T00:00:00Z',can_edit:false},
    person:null,used:'0',in_progress:'0',waiting:0,left_out:{unrated_tokens:0,unmetered_threads:0,unrated_services:fixture.unpriced},state:'ok',period_ends_at:null,
    replaced_by:null,can_bypass:false,bypasses:{count:0,last:null}};
}
const appearance=useAppearance.getState();
afterEach(async()=>{cleanup();useAppearance.setState(appearance);await setLocale('en');fixture.unpriced=0;});
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
