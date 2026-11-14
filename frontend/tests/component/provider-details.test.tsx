/** Provider facts retain their distinct states while unsupported names share one quiet source summary. */
import { screen, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { ModelsSourceState } from '@/features/resources/providers/Models';
import type { DetailMember } from '@/features/resources/providers/details';
import type { PlanProvider } from '@/features/resources/providers/plan';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { OBSERVED, planProvider } from '../unit/provider-plan-fixtures';
import { detailAgent, detailMachineRef } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', () => ({ Link:({children}:{children:ReactNode}) => <a href="#provider-usage">{children}</a> }));
vi.mock('@/features/help/open', () => ({useHelpOpener:() => vi.fn()}));
const MEMBERS: readonly DetailMember[] = ['identity','credential','credential_verified','plan','windows','limit_reached','ordinary_usage_allowed','balances','spend','extra_usage','available_resets'];
const STATES = ['unsupported','not_read_yet','empty','told'] as const;
afterEach(async () => { useAppearance.setState({locale:'en'}); await setLocale('en'); });
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
  provider.facts.extra_usage={enabled:true,used:'1234',limit:'2000',unit:'GBP',decimal_places:2,used_percent:61.7,disabled_reason:null,limit_reached:false};
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
      expect(screen.queryByRole('group',{name:label})).not.toBeInTheDocument();
      const summary=drawn.container.querySelector('[data-unsupported-members]');
      expect(summary).toBeVisible();
      expect(summary).toHaveTextContent(label);
      expect(summary).toHaveTextContent(i18n.t('providerDetails.notSuppliedMembers',{members:''}));
      expect(drawn.container.querySelectorAll('[data-unsupported-members]')).toHaveLength(1);
      expect(screen.getByRole('heading',{name:i18n.t('providerDetails.title',{source:'Example source'})})).toBeVisible();
      expect(screen.queryByRole('meter')).not.toBeInTheDocument();
      expect(screen.queryByText(i18n.t('providerPlan.notReported'), {exact:true})).not.toBeInTheDocument();
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

/** Source declarations and returned member states agree; omitted fields are unsupported, not unread. */
function declaredOnly(provider:PlanProvider, members:readonly DetailMember[]) {
  const source=provider.sources.facts.descriptor?.facts;
  if (source === null || source === undefined) throw new Error('the fixture needs a declared facts source');
  source.reports=[...members];
  source.spends_resets=members.includes('available_resets');
  for (const member of MEMBERS) {
    provider.facts_state[member]=members.includes(member) ? 'told' : 'unsupported';
    if (!members.includes(member)) clear(provider,member);
  }
}

for (const language of ['en','zh-CN'] as const) it('shows the keeper once with one credential row and one missing-members line in '+language, async()=>{
  useAppearance.setState({locale:language});
  await setLocale(language);
  const provider=planProvider();
  provider.sources.facts={...provider.sources.facts,by:'none',name:null,descriptor:null};
  for (const member of MEMBERS) provider.facts_state[member]=member==='credential'?'told':'unsupported';
  const view=show(<ProviderUsageCard provider={provider}/>);
  expect(screen.getByRole('heading',{name:i18n.t('providerDetails.title',{source:i18n.t('providerDetails.keeperSource')})})).toBeVisible();
  expect(screen.getAllByRole('group')).toHaveLength(1);
  expect(screen.getByRole('group',{name:i18n.t('providerDetails.members.credential')})).toHaveTextContent(i18n.t('providerDetails.tokenShort'));
  const missing=MEMBERS.filter((member)=>member!=='credential').map((member)=>i18n.t('providerDetails.members.'+member));
  const summary=view.container.querySelector('[data-unsupported-members]');
  expect(summary).toHaveTextContent(i18n.t('providerDetails.notSuppliedMembers',{members:new Intl.ListFormat(language,{type:'unit'}).format(missing)}));
  expect(view.container.querySelectorAll('[data-unsupported-members]')).toHaveLength(1);
  expect(summary).not.toHaveTextContent(i18n.t('providerDetails.keeperSource'));
  expect(screen.queryByText('team@example.org')).not.toBeInTheDocument();
  expect(screen.queryByText('Pro',{exact:true})).not.toBeInTheDocument();
  expect(screen.queryByRole('meter')).not.toBeInTheDocument();
});

for (const language of ['en','zh-CN'] as const) it.each(['api','subscription'] as const)(
  language+' explains unsupported members without an unknown footer beside reported %s windows', async(kind)=>{
    await setLocale(language);
    const provider=planProvider({kind});
    declaredOnly(provider,['credential','windows']);
    const drawn=show(<ProviderUsageCard provider={provider}/>);
    expect(screen.getByRole('meter',{name:'5-hour window'})).toHaveAttribute('aria-valuenow','0');
    expect(screen.getByRole('meter',{name:'Weekly window (Fable)'})).toHaveAttribute('aria-valuenow','81');
    const summary=drawn.container.querySelector('[data-unsupported-members]');
    expect(summary).toBeVisible();
    for (const member of ['extra_usage','balances'] as const) {
      const label=i18n.t('providerDetails.members.'+member);
      expect(screen.queryByRole('group',{name:label})).not.toBeInTheDocument();
      expect(summary).toHaveTextContent(label);
    }
    expect(drawn.container.querySelectorAll('[data-unsupported-members]')).toHaveLength(1);
    expect(screen.getAllByRole('group')).toHaveLength(2);
    expect(screen.queryByText(i18n.t('providerPlan.notReported'),{exact:true})).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('providerDetails.notReadOnce'),{exact:false})).not.toBeInTheDocument();
  },
);

it('retains supported unread and unknown balances instead of treating missing values as unsupported',()=>{
  const provider=planProvider();
  declaredOnly(provider,['credential','balances']);
  provider.facts_state.balances='not_read_yet';
  provider.facts.balances=[];
  const drawn=show(<ProviderUsageCard provider={provider} refreshing/>);
  const unread=screen.getByRole('status');
  expect(unread).toHaveTextContent(i18n.t('providerDetails.notReadOnce'));
  expect(unread).toHaveTextContent(i18n.t('providerDetails.members.balances'));
  expect(unread).toHaveAttribute('aria-busy','true');
  expect(drawn.container.querySelector('[data-unsupported-members]')).not.toHaveTextContent(i18n.t('providerDetails.members.balances'));
  provider.facts_state.balances='told';
  provider.facts.balances=[{id:'plan',label:'Reported plan',unit:'vendor credits',remaining:null,total:null,used:null,unlimited:false,parts:[],expires_at:null}];
  provider.facts.field_times['balances.plan']=OBSERVED;
  drawn.rerender(<ProviderUsageCard provider={provider}/>);
  const balance=within(screen.getByRole('group',{name:i18n.t('providerDetails.members.balances')}));
  expect(balance.getByText(i18n.t('providerDetails.balanceRemaining',{amount:i18n.t('providerPlan.notReported')}))).toBeVisible();
  expect(balance.queryByText('0 vendor credits remaining')).not.toBeInTheDocument();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
});

for (const language of ['en','zh-CN'] as const) it('groups unsupported names while retaining unread and admin-key causes without old values in '+language, async()=>{
  await setLocale(language);
  const provider=planProvider();
  provider.facts_state.plan='not_read_yet';
  provider.facts.plan='Earlier private plan';
  provider.facts_state.balances='unsupported';
  provider.facts_state.spend={unsupported_because:{reason:'needs_admin_key'}};
  const view=show(<ProviderUsageCard provider={provider} refreshing />);
  const plan=screen.getByRole('group',{name:i18n.t('providerDetails.members.plan')});
  expect(plan).toHaveTextContent(i18n.t('providerDetails.notReadOnce'));
  expect(plan).not.toHaveTextContent('Example source');
  expect(screen.getByRole('heading',{name:i18n.t('providerDetails.title',{source:'Example source'})})).toBeVisible();
  expect(view.container.querySelector('[data-fact-source]')).not.toBeInTheDocument();
  expect(plan).toHaveAttribute('aria-busy','true');
  expect(plan.querySelector('time')).not.toBeInTheDocument();
  expect(screen.queryByText('Earlier private plan')).not.toBeInTheDocument();
  const summary=view.container.querySelector('[data-unsupported-members]');
  expect(summary).toHaveTextContent(i18n.t('providerDetails.notSuppliedMembers',{members:i18n.t('providerDetails.members.balances')}));
  expect(summary).not.toHaveTextContent('12.4');
  expect(summary?.querySelector('time')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-needs-admin-members]')).toHaveTextContent(i18n.t('providerDetails.needsAdminKeyMembers',{members:i18n.t('providerDetails.members.spend')}));
  expect(screen.queryByRole('group',{name:i18n.t('providerDetails.members.balances')})).not.toBeInTheDocument();
  expect(screen.queryByRole('group',{name:i18n.t('providerDetails.members.spend')})).not.toBeInTheDocument();
  expect(screen.getByRole('meter',{name:'5-hour window'})).toHaveAttribute('aria-valuenow','0');
  provider.access.permissions=['provider.manage'];
  view.rerender(<ProviderUsageCard provider={structuredClone(provider)} />);
  expect(screen.queryByRole('group')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-unsupported-members]')).not.toBeInTheDocument();
  expect(screen.queryByText('Example source')).not.toBeInTheDocument();
  expect(screen.queryByText('Earlier private plan')).not.toBeInTheDocument();
});

it('preserves reported zero and unlimited vendor balances after unsupported fields disappear',()=>{
  const provider=planProvider();
  declaredOnly(provider,['credential','balances']);
  const base={unit:'vendor credits',total:null,used:null,parts:[],expires_at:null};
  provider.facts.balances=[
    {...base,id:'zero',label:'Zero balance',remaining:'0',unlimited:false},
    {...base,id:'unlimited',label:'Unlimited balance',remaining:null,unlimited:true},
  ];
  provider.facts.field_times['balances.zero']=OBSERVED;
  provider.facts.field_times['balances.unlimited']=OBSERVED;
  const drawn=show(<ProviderUsageCard provider={provider}/>);
  const balance=within(screen.getByRole('group',{name:i18n.t('providerDetails.members.balances')}));
  expect(balance.getByText('0 vendor credits remaining')).toBeVisible();
  expect(balance.getByText(i18n.t('providerPlan.unlimited'))).toBeVisible();
  expect(balance.queryByRole('meter')).not.toBeInTheDocument();
  expect(drawn.container.querySelector('[data-unsupported-members]')).not.toHaveTextContent(i18n.t('providerDetails.members.balances'));
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
  provider.facts.models_count=state === 'told' ? 1 : state === 'empty' ? 0 : null;
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
