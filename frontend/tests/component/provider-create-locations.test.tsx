/** The one provider form keeps native/key credentials and runtime location independent. */
import { cleanup,screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach,beforeEach,expect,it,vi } from 'vitest';
import { createProvider, createTemplate, createModel } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import type { ArrangementType,ProviderSetup,Template,ProviderModel } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { planProvider,SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
const navigate=vi.hoisted(()=>vi.fn());
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>navigate,Link:({children}:{children:ReactNode})=><a href="#provider">{children}</a>}));
vi.mock('@/features/resources/HomePicker',()=>({
  HomePicker:({value,onChange}:Pick<HomePickerProps,'value'|'onChange'>)=><select aria-label="Home" value={value} onChange={(event)=>onChange(event.target.value)}><option value="">Choose home</option><option value="org">Acme</option></select>,
  useHomeChoices:()=>({choices:[{id:'org',label:'Acme',personal:false,department:false}],pending:false,error:null}),
}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),createProvider:vi.fn(),createTemplate:vi.fn(),createModel:vi.fn(),providerQuery:(id:string)=>({queryKey:['created-provider',id],queryFn:()=>Promise.resolve(planProvider({id}))}),templatesQuery:()=>({queryKey:['local-templates'],queryFn:()=>Promise.resolve(localTemplates)}),providerSetupQuery:()=>({queryKey:['location-setup'],queryFn:()=>Promise.resolve(setup)})}));
let localTemplates: Template[] = [];
const keeper={id:'keeper',name:'Keeper',online:true,placement:'connected' as const,platform_os:'linux' as const};
const runner={...keeper,id:'runner',name:'Runner'};
const setup:ProviderSetup={
  machines:[keeper,runner].map((machine)=>({machine,directory_runtime:true,roles:['agent','provider'],harnesses:['fake'],upstreams:['*']})),
  key_machines:[keeper,runner],http_allowed:false,subscriptions_unshared:[],subscriptions_withheld:[],runtime_templates:[],
  subscriptions:[{agent:{id:'native-profile',name:'Native profile',harness_kind:'fake', harness: harnessName('fake'),address:'acme.native',machine:null,disabled:false},harness:{kind:'fake',name:'Fake'},
    methods:[],options_schema:{},arrangements:[{type:'separated',allowed:true,available:true,puts:'The sign-in stays in its keeper.',form:null}],template:{id:'unit-template',name:'Native unit'}}],
  protocols:[{name:'openai-responses',title:'Responses',agents:[],endpoint_options:[]}],
  presets:[{name:'Example API',note:null,docs_url:null,upstreams:[{protocol:'openai-responses',base_url:'https://models.example/v1',options:{}}]}],
  api_arrangements:[{type:'separated',allowed:true,available:true,puts:'The key stays in its keeper.',form:null}],
  sources:[{by:'builtin',name:'adapter',descriptor:SOURCE_DESCRIPTOR,default_for:[],machine:null,template:null,command:null}],
};
beforeEach(()=>{
  vi.clearAllMocks();const profile=setup.subscriptions[0];
  if(profile!==undefined){profile.agent.machine=null;profile.arrangements=[{type:'separated',allowed:true,available:true,puts:'',form:null}];}
  for(const choice of setup.machines ?? [])choice.arrangements=['separated','key_separated'];
  vi.mocked(createProvider).mockResolvedValue({...planProvider(),warning:null});
});
afterEach(async()=>{cleanup();await setLocale('en');});

async function chooseLocation(user:ReturnType<typeof userEvent.setup>,label:string,value:string) {
  await user.click(screen.getByRole('combobox',{name:label}));
  await user.click(screen.getByRole('option',{name:value}));
}

it.each(['en', 'zh-CN'] as const)('keeps Add provider named the same through both steps and Back in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<AddProviderDialog onClose={vi.fn()} />);
  const name = i18n.t('providers.list.add');
  const dialog = screen.getByRole('dialog', { name });
  expect(screen.getAllByRole('heading', { name })).toHaveLength(1);
  await user.selectOptions(screen.getByRole('combobox', { name: 'Home' }), 'org');
  await screen.findByRole('combobox', { name: i18n.t('providerCreation.examples') });
  await chooseLocation(user, i18n.t('providerCreation.examples'), 'Example API');
  await user.click(screen.getByRole('button', { name: i18n.t('common.next') }));
  expect(dialog).toHaveAccessibleName(name);
  expect(screen.getAllByRole('button', { name })).toHaveLength(1);
  expect(createProvider).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.back') }));
  expect(dialog).toHaveAccessibleName(name);
  expect(screen.getByRole('button', { name: i18n.t('common.next') })).toBeVisible();
  expect(createProvider).not.toHaveBeenCalled();
});

it.each([false,true])('creates a provider with independent locations (native=%s)',async(native)=>{
  const user=userEvent.setup();
  show(<AddProviderDialog onClose={vi.fn()} />);
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await screen.findByRole('combobox',{name:i18n.t('providerCreation.examples')});
  await chooseLocation(user,i18n.t('providerCreation.examples'),native?'Native profile (Fake)':'Example API');
  await user.click(screen.getByRole('button',{name:i18n.t('common.next')}));
  await chooseLocation(user,i18n.t(native?'providerCreation.signInMachine':'providerCreation.keyMachine'),'Keeper');
  await chooseLocation(user,i18n.t('providerCreation.runtime'),'Runner');
  const name=screen.getByLabelText(i18n.t('providers.add.name'));
  await user.clear(name);await user.paste('Independent account');
  await user.click(screen.getByRole('button',{name:i18n.t('providerCreation.create')}));
  await waitFor(()=>expect(createProvider).toHaveBeenCalled());
  const body=vi.mocked(createProvider).mock.calls[0]?.[0];
  expect(body).toMatchObject({kind:native?'subscription':'api',runtime_machine_id:'runner',keeper_machine_id:'keeper'});
  expect(body).not.toHaveProperty('placement');
  expect(body).not.toHaveProperty('runtime_config');
  if (native) {expect(body).toHaveProperty('agent_id','native-profile');expect(body).not.toHaveProperty('api_key');}
  // Adding a provider and editing its key are separate acts. A missing key is not a no-auth endpoint.
  else {expect(body).not.toHaveProperty('api_key');expect(body).toHaveProperty('upstreams',setup.presets[0]?.upstreams);}
});

it('keeps a registered native profile on its declared keeper while choosing another runtime',async()=>{
  const user=userEvent.setup();
  const profile=setup.subscriptions[0];if(profile===undefined)throw new Error('the native setup fixture is required');
  profile.agent.machine=keeper;
  show(<AddProviderDialog onClose={vi.fn()} />);
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await screen.findByRole('combobox',{name:i18n.t('providerCreation.examples')});
  await chooseLocation(user,i18n.t('providerCreation.examples'),'Native profile (Fake)');
  await user.click(screen.getByRole('button',{name:i18n.t('common.next')}));
  expect(screen.getByRole('combobox',{name:i18n.t('providerCreation.signInMachine')})).toBeDisabled();
  expect(screen.getByText(/This native profile is registered on Keeper/)).toBeVisible();
  await chooseLocation(user,i18n.t('providerCreation.runtime'),'Runner');
  await user.click(screen.getByLabelText(i18n.t('providers.add.name')));
  await user.paste('Registered native');
  await user.click(screen.getByRole('button',{name:i18n.t('providerCreation.create')}));
  await waitFor(()=>expect(createProvider).toHaveBeenCalled());
  expect(vi.mocked(createProvider).mock.calls[0]?.[0]).toMatchObject({kind:'subscription',agent_id:'native-profile',keeper_machine_id:'keeper',runtime_machine_id:'runner'});
});

it.each(['en','zh-CN'] as const)('creates a native subscription under the keeper policy and opens its acknowledged provider in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();const close=vi.fn();
  const profile=setup.subscriptions[0];if(profile===undefined)throw new Error('native profile fixture is required');
  const types:ArrangementType[]=['separated','key_separated','credential_shared','handed_over'];
  profile.arrangements=types.map((type)=>({type,allowed:true,available:true,puts:'',form:null}));
  show(<AddProviderDialog onClose={close}/>);
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await screen.findByRole('combobox',{name:i18n.t('providerCreation.examples')});
  await chooseLocation(user,i18n.t('providerCreation.examples'),'Native profile (Fake)');
  await user.click(screen.getByRole('button',{name:i18n.t('common.next')}));
  await chooseLocation(user,i18n.t('providerCreation.signInMachine'),'Keeper');
  await chooseLocation(user,i18n.t('providerCreation.runtime'),'Runner');
  await user.click(screen.getByText(i18n.t('providers.add.arrangements'),{exact:true}));
  for(const kind of ['credential_shared','handed_over'] as const){
    const checkbox=screen.getByRole('checkbox',{name:i18n.t(`providerCreation.arrangements.${kind}`)});
    expect(checkbox).toBeDisabled();expect(checkbox).not.toBeChecked();await user.click(checkbox);
  }
  expect(createProvider).not.toHaveBeenCalled();
  await user.click(screen.getByLabelText(i18n.t('providers.add.name')));await user.paste('Native subscription');
  await user.click(screen.getByRole('button',{name:i18n.t('providerCreation.create')}));
  await waitFor(()=>expect(navigate).toHaveBeenCalledExactlyOnceWith({to:'/resources/providers/$id',params:{id:planProvider().id},search:{tab:'key'}}));
  expect(close).toHaveBeenCalledOnce();
  expect(createProvider).toHaveBeenCalledTimes(1);
  expect(vi.mocked(createProvider).mock.calls[0]?.[0]).toMatchObject({kind:'subscription',agent_id:'native-profile',keeper_machine_id:'keeper',runtime_machine_id:'runner',arrangements:['separated','key_separated']});
});

it.each(['en','zh-CN'] as const)('explains when the selected keeper permits none of the native profile’s ways in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  const choice=setup.machines?.find((entry)=>entry.machine.id==='keeper');
  if(choice===undefined)throw new Error('keeper fixture is required');
  choice.arrangements=[];
  show(<AddProviderDialog onClose={vi.fn()}/>);
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await screen.findByRole('combobox',{name:i18n.t('providerCreation.examples')});
  await chooseLocation(user,i18n.t('providerCreation.examples'),'Native profile (Fake)');
  await user.click(screen.getByRole('button',{name:i18n.t('common.next')}));
  await chooseLocation(user,i18n.t('providerCreation.signInMachine'),'Keeper');
  expect(screen.getByText(i18n.t('providerCreation.noAllowedArrangement'))).toBeVisible();
  await user.click(screen.getByLabelText(i18n.t('providers.add.name')));await user.paste('No admitted way');
  await user.click(screen.getByRole('button',{name:i18n.t('providerCreation.create')}));
  expect(createProvider).not.toHaveBeenCalled();expect(navigate).not.toHaveBeenCalled();
});

for(const locale of ['en','zh-CN'] as const)it(`the machine provider entrance keeps its runtime and opens both navigation steps (${locale})`,async()=>{
  await setLocale(locale);const t=i18n.getFixedT(locale);const user=userEvent.setup();
  show(<AddProviderDialog initialRuntimeMachine={keeper.id} onClose={vi.fn()}/>);
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await screen.findByRole('combobox',{name:t('providerCreation.examples')});
  await chooseLocation(user,t('providerCreation.examples'),'Example API');
  expect(screen.getAllByRole('button',{name:t('common.next')})).toHaveLength(1);
  await user.click(screen.getByRole('button',{name:t('common.next')}));
  expect(await screen.findByRole('combobox',{name:t('providerCreation.runtime')})).toHaveTextContent('Keeper');
  expect(screen.getByRole('button',{name:t('common.back')})).toBeVisible();
  await user.click(screen.getByRole('button',{name:t('common.back')}));
  expect(screen.getByRole('button',{name:t('common.next')})).toBeVisible();expect(createProvider).not.toHaveBeenCalled();
});

it.each([true, false])('Local embeddings uses a live ordinary template or creates a fresh one (live=%s)', async (live) => {
  const before = setup.presets; const http = setup.http_allowed; const protocols = setup.protocols;
  const recipe = {name: 'Local embeddings', home_id: 'org', kind: 'provider' as const, driver: 'docker', spec: {build: {dockerfile: 'FROM llama\nRUN fetch-model', context: null, args: {}, target: null}}};
  const template = {id: live ? 'edited-template' : 'fresh-template', name: 'Local embeddings', home: {id: 'org'}, spec: recipe.spec} as Template;
  try {
    setup.http_allowed = false; setup.protocols = [...protocols, {name: 'openai-embeddings', title: 'Embeddings', agents: [], endpoint_options: []}];
    setup.presets = [...before, {name: 'Local embeddings', note: null, docs_url: null, upstreams: [{protocol: 'openai-embeddings', base_url: 'http://127.0.0.1:8090/v1', options: {auth: 'none'}}], template: recipe, model: {model_name: 'embeddinggemma-2', display_name: 'EmbeddingGemma 2'}, dimensions: 256}];
    localTemplates = live ? [template] : [];
    vi.mocked(createTemplate).mockResolvedValue(template); vi.mocked(createModel).mockResolvedValue({} as ProviderModel);
    const done = vi.fn(); const user = userEvent.setup();
    show(<AddProviderDialog initialHome="org" initialPreset="Local embeddings" onClose={vi.fn()} onCreated={done} />);
    expect(await screen.findByRole('textbox', {name: i18n.t('admin.semantic.fields.model')})).toHaveValue('embeddinggemma-2');
    expect(screen.getByRole('textbox', {name: i18n.t('admin.semantic.fields.dimensions')})).toHaveValue('256');
    expect(screen.getByRole('button', {name: i18n.t('taskGuides.open', {title: i18n.t('taskGuides.addProvider')})})).toHaveAttribute('data-help-heading', 'local-embeddings');
    await user.click(screen.getByRole('button', {name: i18n.t('common.next')}));
    await user.click(screen.getByRole('button', {name: i18n.t('providerCreation.create')}));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(createProvider).toHaveBeenCalledWith(expect.anything(),expect.any(String));
    const given=vi.mocked(createProvider).mock.calls[0]?.[0];
    expect(given).toMatchObject({kind:'api',runtime_config:{'runtime.template_id':template.id}});
    if(given?.kind!=='api')throw new Error('the ordinary API provider is created');
    expect(given.upstreams).toMatchObject([{protocol:'openai-embeddings',base_url:'http://127.0.0.1:8090/v1',options:{auth:'none'}}]);
    expect(createModel).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({model_name: 'embeddinggemma-2'}));
    if (live) expect(createTemplate).not.toHaveBeenCalled();
    else expect(createTemplate).toHaveBeenCalledWith(recipe, expect.any(String));
  } finally { setup.presets = before; setup.protocols = protocols; setup.http_allowed = http; }
});
