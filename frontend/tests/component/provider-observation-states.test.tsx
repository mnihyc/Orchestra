/** D443: provider readiness, native session availability and chart readings are separate facts. */
import { cleanup, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as AdoptionApi from '@/api/adoption';
import { ApiError } from '@/api/client';
import { HarnessSessions, type SessionHarness } from '@/features/resources/adopt/Sessions';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { FACT_MEMBERS } from '@/features/resources/providers/details';
import { UsageChart } from '@/features/usage/Chart';
import { Figures } from '@/features/usage/Figures';
import { chartEmptyText } from '@/features/usage/figures';
import type { UsageFigures } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

let sessionFailure: ApiError;
vi.mock('@/api/adoption',async(load)=>({...await load<typeof AdoptionApi>(),harnessSessionsQuery:()=>({
  queryKey:['observed-sessions'],queryFn:()=>Promise.reject(sessionFailure),retry:false,
})}));
const harness:SessionHarness={providerId:'provider',providerName:'Ready model',agent:{
  id:'agent',name:'Codex',address:'acme.codex',harness_kind:'codex', harness: harnessName('codex'),machine:null,disabled:false,
}};
const totals:UsageFigures={turns:10,credits:'0',estimated_credits:'0',unrated_tokens:0,
  tokens:{input_tokens:0,output_tokens:0,cached_input_tokens:0,reasoning_tokens:0,total_tokens:0}};
afterEach(async()=>{cleanup();document.documentElement.removeAttribute('data-theme');await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const){
  it.each([
    {code:'harness.sessions_no_runtime',status:409},
    {code:'harness.sessions_unavailable',status:503},
  ] as const)(`${locale}/${theme} explains $code without calling its provider unavailable`,async({code,status})=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    sessionFailure=new ApiError({type:'orch:error:'+code,status,code,title:'Agent sessions',detail:'The session read could not be completed',request_id:'native-read'},null);
    show(<HarnessSessions harnesses={[harness]}/>);
    expect(await screen.findByText(i18n.t(`errors.${code}`))).toBeVisible();
    expect(screen.queryByText(i18n.t('errors.provider.unavailable'))).not.toBeInTheDocument();
    expect(screen.queryAllByRole('button',{name:i18n.t('common.tryAgain')})).toHaveLength(status===503?1:0);
  });
  it(`${locale}/${theme} says Not reported once instead of listing unsupported members`,async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    const provider=planProvider();
    for(const member of FACT_MEMBERS)provider.facts_state[member]='unsupported';
    const view=show(<ProviderUsageCard provider={provider}/>);
    expect(screen.getAllByText(i18n.t('providerPlan.notReported'),{exact:true})).toHaveLength(1);
    expect(view.container.querySelectorAll('[data-unsupported-members]')).toHaveLength(1);
    for(const member of FACT_MEMBERS)expect(screen.queryByText(i18n.t(`providerDetails.members.${member}`),{exact:true})).not.toBeInTheDocument();
    expect(screen.queryByRole('meter')).not.toBeInTheDocument();
  });
  it(`${locale}/${theme} reports zero credits beside ten turns without saying nothing was used`,async()=>{
    await setLocale(locale);document.documentElement.dataset.theme=theme;
    show(<><Figures totals={totals} previous={null} before=""/>
      <UsageChart label={i18n.t('usage.chart.label')} bars={{xs:[0],heights:[0],widths:[86400000],stacks:[],bucket:'day'}} measure="credits"
        empty={chartEmptyText(totals,'credits',i18n.t)}/></>);
    expect(screen.getByText(i18n.t('usage.figures.turns'),{exact:true})).toBeVisible();
    expect(screen.getByText('10',{exact:true})).toBeVisible();
    expect(within(screen.getByRole('status')).getByText(i18n.t('usage.chart.zeroCredits'))).toBeVisible();
    expect(screen.queryByText(i18n.t('usage.chart.empty'))).not.toBeInTheDocument();
  });
}

it.each(['en','zh-CN'] as const)('keeps unpriced tokens distinct from zero credits in %s',async(locale)=>{
  await setLocale(locale);
  const unrated:UsageFigures={...totals,tokens:{...totals.tokens,total_tokens:64,input_tokens:64},unrated_tokens:64};
  show(<UsageChart label={i18n.t('usage.chart.label')} bars={{xs:[0],heights:[0],widths:[86400000],stacks:[],bucket:'day'}} measure="credits" empty={chartEmptyText(unrated,'credits',i18n.t)}/>);
  expect(await screen.findByText(i18n.t('usage.chart.unrated'))).toBeVisible();
  expect(screen.queryByText(i18n.t('usage.chart.zeroCredits'))).not.toBeInTheDocument();
});
