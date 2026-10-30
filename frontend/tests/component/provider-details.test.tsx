/** Every provider fact has its own unsupported, unread, empty and told presentation, for either credential kind. */
import { screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { ModelsSourceState } from '@/features/resources/providers/Models';
import type { DetailMember } from '@/features/resources/providers/details';
import type { PlanProvider } from '@/features/resources/providers/plan';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { detailAgent, detailMachineRef } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ Link:({children}:{children:ReactNode}) => <a href="#provider-usage">{children}</a> }));
vi.mock('@/features/help/open', () => ({useHelpOpener:() => vi.fn()}));
const MEMBERS: readonly DetailMember[] = ['identity','credential','credential_verified','plan','windows','limit_reached','ordinary_usage_allowed','balances','spend','extra_usage','available_resets'];
const STATES = ['unsupported','not_read_yet','empty','told'] as const;
afterEach(async () => { await setLocale('en'); });
function clear(provider:PlanProvider, member:DetailMember) {
  switch (member) {
    case 'identity': provider.facts.identity=null; break;
    case 'credential': provider.facts.credential=null; break;
    case 'credential_verified': provider.facts.credential_verified=null; break;
    case 'plan': provider.facts.plan=null; break;
    case 'windows': provider.facts.windows=[]; break;
    case 'limit_reached': provider.facts.limit_reached=null; break;
    case 'ordinary_usage_allowed': provider.facts.ordinary_usage_allowed=null; break;
    case 'balances': provider.facts.balances=[]; break;
    case 'spend': provider.facts.spend=[]; break;
    case 'extra_usage': provider.facts.extra_usage=null; break;
    case 'available_resets': provider.facts.available_resets=null; break;
  }
}
function reported(provider:PlanProvider) {
  provider.facts.limit_reached={type:'Weekly allowance',resets_at:'2099-01-01T00:00:00Z'};
  provider.facts.ordinary_usage_allowed=false;
  provider.facts.balances=[{id:'account',label:'Account balance',unit:'USD',remaining:'0.00000000000000000001',total:'10',used:'9.99999999999999999999',unlimited:false,parts:[{label:'Granted',amount:'1.25'}],expires_at:null}];
  provider.facts.spend=[{id:'daily',label:'Daily spend',unit:'USD',period:'day',period_minutes:null,used:'0',limit:'10',remaining:'10',resets_at:null}];
  provider.facts.extra_usage={enabled:true,used:1234,limit:2000,unit:'GBP',decimal_places:2,used_percent:61.7,disabled_reason:null,limit_reached:false};
}

for (const kind of ['api','subscription'] as const) describe(kind + ' credential details', () => {
  for (const member of MEMBERS) it.each(STATES)(member + ' is %s', (state) => {
    const provider=planProvider({kind});
    reported(provider);
    for (const key of MEMBERS) provider.facts_state[key]='unsupported';
    provider.facts_state[member]=state;
    if (state !== 'told') clear(provider,member);
    const label=i18n.t('providerDetails.members.' + member, {defaultValue:member});
    const drawn=show(<ProviderUsageCard provider={provider} refreshing={state === 'not_read_yet'} />);
    const block=drawn.container.querySelector('[data-fact-member="' + member + '"]');
    if (state === 'unsupported') {
      expect(block).not.toBeInTheDocument();
      const foot=drawn.container.querySelectorAll('[data-unsupported-members]');
      expect(foot).toHaveLength(1);
      expect(foot[0]).toHaveTextContent(i18n.t('providerPlan.notReported'));
      expect(foot[0]).not.toHaveTextContent(label);
      return;
    }
    expect(block).toHaveAttribute('data-fact-state',state);
    if (state === 'not_read_yet') {
      const pending=screen.getByRole('status');
      expect(pending).toHaveTextContent(i18n.t('providerDetails.notReadOnce'));
      expect(pending).toHaveTextContent(label);
      expect(pending).toHaveAttribute('aria-busy','true');
      expect(within(pending).queryByRole('meter')).not.toBeInTheDocument();
      return;
    }
    const section=within(screen.getByRole('group',{name:label}));
    if (state === 'empty') {
      expect(section.getByText(i18n.t('providerDetails.empty.' + member,{defaultValue:member}))).toBeVisible();
      expect(section.queryByRole('meter')).not.toBeInTheDocument();
    } else switch(member) {
      case 'identity': expect(section.getByText('team@example.org')).toBeVisible(); break;
      case 'credential': expect(section.getByText(i18n.t('providerDetails.tokenShort'))).toBeVisible(); break;
      case 'credential_verified': expect(section.getByText(i18n.t('providerDetails.verifiedShort'))).toBeVisible(); break;
      case 'plan': expect(section.getByText('Pro')).toBeVisible(); break;
      case 'windows': expect(section.getByRole('meter',{name:'5-hour window'})).toHaveAttribute('aria-valuenow','0'); break;
      case 'limit_reached': expect(section.getByText('Weekly allowance')).toBeVisible(); break;
      case 'ordinary_usage_allowed': expect(section.getByText(i18n.t('providerDetails.refusedShort'))).toBeVisible(); break;
      case 'balances': expect(section.getByText('0.00000000000000000001 USD remaining')).toBeVisible(); expect(section.getByText('Granted: 1.25 USD')).toBeVisible(); break;
      case 'spend': expect(section.getByText('0 USD used of 10 USD, 10 USD remaining')).toBeVisible(); break;
      case 'extra_usage': expect(section.getByText('12.34 GBP used of 20 GBP')).toBeVisible(); break;
      case 'available_resets': expect(section.getByText('Full reset')).toBeVisible(); break;
    }
  });
});

it('uses windows, then balance, then spend in a compact card while preserving precision and real zero', () => {
  const provider=planProvider({kind:'api'});
  reported(provider);
  const {rerender}=show(<ProviderUsageCard provider={provider} compact />);
  expect(screen.getByText('Weekly window (Fable) 81% used')).toBeVisible();
  provider.facts_state.windows='empty';
  provider.facts.windows=[];
  rerender(<ProviderUsageCard provider={provider} compact />);
  expect(screen.getByText(/0\.00000000000000000001 USD/)).toBeVisible();
  provider.facts_state.balances='unsupported';
  provider.facts_state.spend='told';
  rerender(<ProviderUsageCard provider={provider} compact />);
  expect(screen.getByText('Today: 0 USD spent')).toBeVisible();
  provider.facts_state.spend='not_read_yet';
  rerender(<ProviderUsageCard provider={provider} compact />);
  expect(screen.getByText('Not reported')).toBeVisible();
});

it.each(STATES)('models remain %s independently of the manually enabled list', (state) => {
  const provider=planProvider();
  provider.facts_state.models=state;
  provider.facts.models=state === 'told' ? [{model_name:'example',display_name:'Example',parameters:{},listed:true}] : state === 'empty' ? [] : null;
  show(<ModelsSourceState provider={provider} />);
  const key=state === 'unsupported' ? 'providerDetails.models.unsupported' : state === 'not_read_yet' ? 'providerDetails.models.notRead' : state === 'empty' ? 'providerDetails.models.empty' : 'providerDetails.models.told';
  expect(screen.getByText(i18n.t(key))).toBeVisible();
  expect(provider.models).toEqual([]);
});

for (const language of ['en','zh-CN'] as const) it('names the reported program when discovery is off in '+language, async () => {
  await setLocale(language);
  const named={harness:{kind:'research-agent',name:'Research assistant',names:{'zh-CN':'研究助理'}}};
  const provider={...planProvider({harness_kind:'research-agent',machine:detailMachineRef,harnesses:[{agent:detailAgent,machine:detailMachineRef,status:'online',status_detail:null,sessions_open:0,harness_version:'1.2.3',last_active_at:null}]}),...named};
  provider.facts_state.models='unsupported';
  show(<ModelsSourceState provider={provider} />);
  const reason=i18n.t('programVersions.featureOff',{feature:i18n.t('programVersions.features.model_discovery'),name:language==='zh-CN'?'研究助理':'Research assistant',version:'1.2.3'});
  expect(screen.getByText(reason)).toBeVisible();
  expect(document.body).not.toHaveTextContent('research-agent');
});
