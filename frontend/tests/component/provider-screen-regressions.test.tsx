/** QS-009/QS-029: visible provider names, the next setup act and credential facts agree with one another. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ComponentPropsWithRef } from 'react';
import { I18nextProvider } from 'react-i18next';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { ProviderList } from '@/features/resources/providers/ProviderList';
import type * as ResourcesApi from '@/api/resources';
import { ProviderSetupNotice } from '@/features/resources/providers/SetupNotice';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { FACT_MEMBERS } from '@/features/resources/providers/details';
import type { ResourceListProps } from '@/features/resources/ResourceList';
import type { Provider } from '@/generated/api-types';
import { i18n, loadSearchCatalogues } from '@/i18n';
import { knownPlaces } from '@/lib/placeLabel';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { MODEL_ESTIMATE } from '../doubles/fixtures';

const listed=vi.hoisted((): {rows:Provider[]} => ({rows:[]}));
beforeAll(async () => { await loadSearchCatalogues(); });
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),providersQuery:()=>({queryKey:['qs-provider-list'],queryFn:()=>Promise.resolve(listed.rows),initialData:listed.rows})}));
vi.mock('@/features/resources/providers/AddProvider',()=>({AddProviderDialog:()=>null}));
vi.mock('@/features/resources/providers/acts',()=>({useProviderActs:()=>({list:{bulk:[],dialogs:null}})}));
vi.mock('@/features/resources/harnesses',()=>({useHarnessTitle:()=>(kind:string)=>kind}));
vi.mock('@/features/resources/HomePicker',()=>({useViewedOrg:()=>null}));
vi.mock('@/features/navigation/placeLabels',()=>({useKnownPlaces:()=>knownPlaces([
  {id:'acme',name:'Acme',above:[]},{id:'beta',name:'Beta',above:[]},
])}));
// The help question beside a column reads the router when it renders (useHelpOpener).
vi.mock('@tanstack/react-router',()=>({Link:({children,params,to:_to,...props}:ComponentPropsWithRef<'a'> & {params?:{id:string};to?:string})=>
  <a {...props} href={'/resources/providers/' + (params?.id ?? '')}>{children}</a>,
  useRouter:()=>({state:{location:{pathname:'/resources/providers'}},history:{push:()=>undefined}})}));
// The name column is the behavior under review; geometry, row menus and the list frame have their own proofs.
vi.mock('@/features/resources/ResourceList',()=>({
  useSearchText:(text:string)=>[text,vi.fn()],
  ResourceList:({columns}:Pick<ResourceListProps<Provider>,'columns'>)=>{
    const rows=listed.rows;
    const name=(typeof columns==='function'?columns(rows):columns).find((column)=>column.id==='name');
    const shown=new Set(['name','owner','status']);
    return <div>{rows.map((row)=><div key={row.id}>{name?.cell(row,shown)}</div>)}</div>;
  },
}));

it('labels colliding origins in the provider name column and drops the suffix for a unique result', async () => {
  const user = userEvent.setup();
  listed.rows=[
    planProvider({id:'first',name:'Scripted model',org_id:'acme',home:{id:'acme',kind:'org',name:'Acme',redacted:false}}),
    planProvider({id:'second',name:'Scripted model',org_id:'beta',home:{id:'beta',kind:'org',name:'Beta',redacted:false}}),
  ];
  const {rerender,container}=show(<ProviderList search={{}} onSearch={vi.fn()} />);
  expect(screen.getByRole('link',{name:'Scripted model (Acme)'})).toHaveAttribute('href','/resources/providers/first');
  const beta = screen.getByRole('link',{name:'Scripted model (Beta)'});
  expect(beta).toHaveAttribute('href','/resources/providers/second');
  await user.hover(beta);
  const card = await screen.findByLabelText('Scripted model', { selector: '[data-inspection-card]' });
  expect(await within(card).findByRole('link', { name: 'Open provider' })).toHaveAttribute('href', '/resources/providers/second');
  expect(card).toHaveTextContent('Scripted model');
  expect(card).toHaveTextContent('Beta');
  await user.unhover(beta);
  await waitFor(() => expect(document.querySelector('[data-resource-card="provider"]')).toBeNull());
  // The shared server pager now filters before LIMIT; this fixture supplies its returned page.
  listed.rows=listed.rows.filter((row)=>row.org_id==='beta');
  rerender(<ProviderList search={{q:'Beta'}} onSearch={vi.fn()} />);
  expect(within(container).queryByRole('link',{name:'Scripted model (Acme)'})).not.toBeInTheDocument();
  expect(within(container).getByRole('link',{name:'Scripted model'})).toBeVisible();
  expect(within(container).queryByRole('link',{name:'Scripted model (Beta)'})).not.toBeInTheDocument();
});

const endpoint={protocol:'openai-responses',base_url:'https://example.invalid/v1',options:{}};
function keeperOnly(provider:Provider) {
  provider.sources.facts={...provider.sources.facts,by:'none',name:null,descriptor:null};
  for (const member of FACT_MEMBERS) provider.facts_state[member]=member==='credential' ? 'told' : 'unsupported';
  provider.facts.credential={method:null,yields:'none',issued_at:null,expires_at:null};
  return provider;
}

for (const language of ['en','zh-CN'] as const) describe('QS-029 in ' + language, () => {
  const t=i18n.getFixedT(language);
  it('shows the missing model with its act, without claiming that an unauthenticated endpoint needs a key', async () => {
    const words=i18n.cloneInstance({lng:language});
    const provider=planProvider({kind:'api',status:'needs_setup',upstreams:[{...endpoint,options:{auth:'none'}}]});
    const onTab=vi.fn();
    show(<I18nextProvider i18n={words}><ProviderSetupNotice provider={provider} manage onTab={onTab} /></I18nextProvider>);
    expect(screen.getByText(t('providers.phrase.needsModel'))).toBeVisible();
    expect(screen.getByText(t('providers.overview.needsModelLead'))).toBeVisible();
    expect(screen.queryByText(t('providers.phrase.needsSetup'))).not.toBeInTheDocument();
    expect(screen.queryByText(t('providers.phrase.needsKey'))).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button',{name:t('resources.act.nextModels')}));
    expect(onTab).toHaveBeenCalledWith('models');
  });
  it('keeps the setup reason visible to an observer without offering a manager act', () => {
    const words=i18n.cloneInstance({lng:language});
    const provider=planProvider({kind:'api',status:'needs_setup',upstreams:[endpoint]});
    show(<I18nextProvider i18n={words}><ProviderSetupNotice provider={provider} manage={false} onTab={vi.fn()} /></I18nextProvider>);
    expect(screen.getByText(t('providers.overview.needsKeyLead'))).toBeVisible();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
  it('offers a setup review with the returned reason when the key and model are already ready', async () => {
    const words=i18n.cloneInstance({lng:language});
    const provider=planProvider({kind:'api',status:'needs_setup',placement:'connected',key_set:true,upstreams:[endpoint],
      status_detail:'The keeping machine needs its endpoint configuration reviewed.',
      models:[{ ...MODEL_ESTIMATE,id:'model',model_name:'example',display_name:'Example',parameters:{},groups:[],enabled:true,source:'manual'}]});
    const onTab=vi.fn();
    show(<I18nextProvider i18n={words}><ProviderSetupNotice provider={provider} manage onTab={onTab} /></I18nextProvider>);
    expect(screen.getAllByText(t('providers.phrase.needsSetup'))).toHaveLength(1);
    expect(screen.getByText(provider.status_detail ?? '')).toBeVisible();
    await userEvent.click(screen.getByRole('button',{name:t('providers.overview.nextSettings')}));
    expect(onTab).toHaveBeenCalledWith('settings');
  });
  it('names the keeper and a saved API key even when an older credential fact says none', () => {
    const words=i18n.cloneInstance({lng:language});
    const provider=keeperOnly(planProvider({kind:'api',key_set:true,upstreams:[endpoint]}));
    show(<I18nextProvider i18n={words}><ProviderUsageCard provider={provider} /></I18nextProvider>);
    expect(screen.getByRole('heading',{name:t('providerDetails.title',{source:t('providerDetails.keeperSource')})})).toBeVisible();
    const credential=within(screen.getByRole('group',{name:t('providerDetails.members.credential')}));
    expect(credential.getByText(t('providerDetails.key'))).toBeVisible();
    expect(credential.queryByText(t('providerDetails.noCredentialNeeded'))).not.toBeInTheDocument();
  });
  it('distinguishes a missing native credential from endpoints explicitly needing no key', () => {
    const words=i18n.cloneInstance({lng:language});
    const provider=keeperOnly(planProvider());
    const {rerender}=show(<I18nextProvider i18n={words}><ProviderUsageCard provider={provider} /></I18nextProvider>);
    expect(screen.getByText(t('providerDetails.noCredentialHeld'))).toBeVisible();
    expect(screen.queryByText(t('providerDetails.noCredentialNeeded'))).not.toBeInTheDocument();
    provider.kind='api';provider.upstreams=[{...endpoint,options:{auth:'none'}}];
    rerender(<I18nextProvider i18n={words}><ProviderUsageCard provider={provider} /></I18nextProvider>);
    expect(screen.getByText(t('providerDetails.noCredentialNeeded'))).toBeVisible();
  });
});
