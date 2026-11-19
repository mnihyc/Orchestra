/** The served provider context scopes program offers and keeps address warnings and drafts with their provider. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { providerSetupQuery, setProviderSources } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import type * as SettingsApi from '@/api/settings';
import { ProviderSources } from '@/features/resources/providers/Sources';
import type { MachinePolicy, ProviderSetup } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { planProvider, SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/help/open', () => ({useHelpOpener:() => vi.fn()}));
vi.mock('@tanstack/react-router', () => ({Link:({children}:{children:ReactNode}) => <a href="#machine-policy">{children}</a>}));
vi.mock('@/api/settings', async (load) => ({...await load<typeof SettingsApi>(),settingsQuery:(id:string) => ({queryKey:['settings',id],queryFn:() => Promise.resolve([])})}));
vi.mock('@/api/resources', async (load) => ({...await load<typeof ResourcesApi>(),
  providerSetupQuery:vi.fn((org:string,provider?:string) => ({queryKey:['provider-setup',org,provider],queryFn:() => Promise.resolve(servedSetup)})),
  providerSettingsQuery:(id:string) => ({queryKey:['provider-settings',id],queryFn:() => Promise.resolve([])}),
  machinePolicyQuery:(id:string) => ({queryKey:['machine-policy',id],queryFn:() => Promise.resolve(policy)}),
  setProviderSources:vi.fn(),
}));

const keeper={id:'foreign-keeper',name:'Provider keeper',online:true,placement:'connected' as const,platform_os:'linux' as const};
const setup:ProviderSetup={subscriptions:[],subscriptions_withheld:[],subscriptions_unshared:[],protocols:[],presets:[],api_arrangements:[],key_machines:[keeper],http_allowed:false,runtime_templates:[],sources:[
  {by:'program',name:'keeper-reports',descriptor:{...SOURCE_DESCRIPTOR,name:'keeper-reports',title:'Keeper reports'},default_for:[],machine:keeper,template:null,command:null},
  {by:'program',name:'another',descriptor:{...SOURCE_DESCRIPTOR,name:'another',title:'Another machine reports'},default_for:[],machine:{...keeper,id:'different'},template:null,command:null},
]};
const policy:MachinePolicy={machine:keeper,reported_at:'2026-10-14T12:00:00Z',keys:[{key:'provider.upstreams',definition:{key:'provider.upstreams',title:'Provider upstreams',meaning:'Addresses the keeping machine permits providers to reach.',sections:['provider'],cautious:[],managed:['*'],delegable:true},value:[],bounds:'provider',writable:false,delegated:false}]};
let servedSetup=setup;
const connected=() => planProvider({id:'managed-by-reader',kind:'api',placement:'connected',machine:keeper,credential_keeper:keeper});
beforeEach(() => {vi.clearAllMocks();servedSetup=setup;});

it('uses provider.manage context to choose its actual keeper’s program and writes the canonical choice at the reviewed revision', async () => {
  const provider=connected();
  const fresh=structuredClone(provider);fresh.revision+=1;
  fresh.sources.facts={by:'program',name:'keeper-reports',address:null,config:{},secrets:[],descriptor:SOURCE_DESCRIPTOR,described_at:null,last_error:null};
  vi.mocked(setProviderSources).mockResolvedValue(fresh);
  show(<ProviderSources provider={provider} />);
  const group=within(await screen.findByRole('group',{name:'Details from'}));
  expect(providerSetupQuery).toHaveBeenCalledWith(provider.home.id,provider.id);
  await userEvent.click(group.getByRole('combobox',{name:'Details from'}));
  await userEvent.click(screen.getByRole('option',{name:'Program'}));
  await userEvent.click(group.getByRole('combobox',{name:'Registered program'}));
  expect(screen.getByRole('option',{name:'Keeper reports'})).toBeVisible();
  expect(screen.queryByRole('option',{name:'Another machine reports'})).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('option',{name:'Keeper reports'}));
  await userEvent.click(screen.getByRole('button',{name:'Save sources'}));
  await waitFor(() => expect(setProviderSources).toHaveBeenCalledWith(provider.id,{expected_revision:provider.revision,facts:{by:'program',name:'keeper-reports',config:{},secrets:{}}}));
});

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
it('marks a locally blocked address and HTTP refusal, then drops that provider’s secret draft on a route change', { timeout: 30_000 }, async () => {
  const provider=connected();
  const view=show(<ProviderSources provider={provider} />);
  let group=within(await screen.findByRole('group',{name:'Details from'}));
  await userEvent.click(group.getByRole('combobox',{name:'Details from'}));
  await userEvent.click(screen.getByRole('option',{name:'Address'}));
  await userEvent.type(group.getByRole('textbox',{name:'Address'}),'http://reports.example/read');
  expect(group.getByText('Unencrypted')).toBeVisible();
  expect(await group.findByText(i18n.t('providerSources.upstreamWarning',{address:'http://reports.example/read'}))).toBeVisible();
  expect(group.getByRole('link',{name:'Provider keeper policy'})).toBeVisible();
  expect(screen.getByRole('button',{name:'Save sources'})).toBeDisabled();
  await userEvent.click(group.getByRole('combobox',{name:'Authorization'}));
  await userEvent.click(screen.getByRole('option',{name:'Bearer token'}));
  await userEvent.type(group.getByLabelText('Authorization value'),'old-provider-draft');
  const other=connected();other.id='another-provider';
  other.sources.facts={by:'address',name:null,address:{url:'https://other.example/read',auth:'bearer',send_credential:false},config:{},secrets:[],descriptor:null,described_at:null,last_error:null};
  other.secrets_set=['source:facts:authorization'];
  view.rerender(<ProviderSources provider={other} />);
  group=within(await screen.findByRole('group',{name:'Details from'}));
  expect(group.getByRole('textbox',{name:'Address'})).toHaveValue('https://other.example/read');
  expect(group.getByLabelText('Authorization value')).toHaveValue('');
  expect(setProviderSources).not.toHaveBeenCalled();
  expect(providerSetupQuery).toHaveBeenCalledWith(other.home.id,other.id);
});

it('shows what a template program reported after it ran, retaining its registered command', async () => {
  const provider=planProvider({kind:'api',placement:'managed',machine:{...keeper,id:'runtime-machine',placement:'managed'},runtime_support:{lifecycle:'unit',commands_after_start:null,after_up:null}});
  servedSetup=structuredClone(setup);
  servedSetup.runtime_templates=[{id:'selected-template',name:'Installed provider',driver:'docker',owner:null}];
  servedSetup.sources=[
    {by:'program',name:'reports',descriptor:null,default_for:[],machine:null,template:{id:'selected-template',name:'Installed provider'},command:['python','/opt/reports.py']},
    {by:'program',name:'reports',descriptor:{...SOURCE_DESCRIPTOR,title:'Running reports',fields:[{name:'token',label:'Program secret',type:'secret',required:false,default:null,help:null,choices:null,max_bytes:null}]},default_for:[],machine:provider.machine,template:null,command:null},
  ];
  show(<ProviderSources provider={provider} />);
  const group=within(await screen.findByRole('group',{name:'Details from'}));
  await userEvent.click(group.getByRole('combobox',{name:'Details from'}));
  await userEvent.click(screen.getByRole('option',{name:'Program'}));
  await userEvent.click(group.getByRole('combobox',{name:'Registered program'}));
  await userEvent.click(screen.getByRole('option',{name:'Running reports · Installed provider'}));
  const secret = group.getByLabelText(/^Program secret/);
  expect(secret).toHaveAttribute('type', 'password');
  expect(secret).toHaveValue('');
  expect(group.getByText('python /opt/reports.py')).toBeVisible();
  expect(group.queryByText('What it reports will be known after its runtime starts and describes it.')).not.toBeInTheDocument();
});
