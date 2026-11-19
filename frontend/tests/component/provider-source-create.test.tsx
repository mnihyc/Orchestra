/** Examples fill the same abstract provider interface used by a custom GPU adapter. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createProvider } from '@/api/resources';
import { changeMachinePrograms } from '@/api/machinePrograms';
import { ApiError } from '@/api/client';
import type * as ResourcesApi from '@/api/resources';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import type { MachineProgram, ProviderSetup } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { planProvider, SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';

const guide = vi.hoisted(() => vi.fn());
const navigate = vi.hoisted(() => vi.fn());
const close = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => guide }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@tanstack/react-router', () => ({ useNavigate: () => navigate, Link: ({ children }: { children: ReactNode }) => <a href="#provider">{children}</a> }));
vi.mock('@/api/machinePrograms',()=>({machineProgramsKey:(id:string)=>['programs',id],changeMachinePrograms:vi.fn(),
  machineProgramsQuery:(id:string)=>({queryKey:['programs',id],queryFn:()=>Promise.resolve({entries:[],can_manage:true,config_file:'/etc/orch.toml',
    capabilities:[{kind:'provider_source',supported:true,remote:true,delegated:true,enabled:true,policy_key:'provider.programs',reason:null}]})})}));
vi.mock('@/features/resources/HomePicker', () => ({
  HomePicker: ({ value, onChange }: Pick<HomePickerProps, 'value' | 'onChange'>) => <select aria-label="Home" value={value} onChange={(event) => onChange(event.target.value)}><option value="">Choose home</option><option value="org">Acme</option></select>,
  useHomeChoices: () => ({ choices: [{ id: 'org', label: 'Acme', personal: false, department: false }], pending: false, error: null }),
}));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), createProvider: vi.fn(), providerSetupQuery: (home: string) => ({ queryKey: ['provider-setup', home], queryFn: () => Promise.resolve(setup) }) }));

const machine = { id: 'keeper', name: 'GPU machine', online: true, placement: 'connected' as const, platform_os: 'linux' as const };
const other = { ...machine, id: 'other', name: 'Other machine' };
const setup: ProviderSetup = {
  machines: [machine, other].map((entry) => ({ machine: entry, directory_runtime: true, roles: ['provider'], harnesses: [], upstreams: ['*'] })),
  subscriptions: [], subscriptions_withheld: [], subscriptions_unshared: [], http_allowed: false,
  protocols: [
    { name: 'openai-chat', title: 'Chat Completions', endpoint_options: [], agents: [{id:'gpu-agent',name:'GPU assistant',address:'Acme',harness_kind:'local',harness:{kind:'local',name:'GPU assistant',names:{}},machine:null,disabled:false}] },
    { name: 'openai-responses', title: 'Responses', endpoint_options: [], agents: [] },
  ],
  presets: [{ name: 'OpenRouter', note: null, docs_url: null, upstreams: [{ protocol: 'openai-chat', base_url: 'https://openrouter.ai/api/v1', options: {} }] }],
  api_arrangements: [{ type: 'separated', allowed: true, available: true, puts: 'The key stays at home.', form: null }], key_machines: [machine, other],
  runtime_templates: [{ id: 'template', name: 'Python provider', driver: 'docker', owner: null }],
  sources: [
    { by: 'builtin', name: 'endpoints', descriptor: { ...SOURCE_DESCRIPTOR, name: 'endpoints', title: 'Endpoints', facts: null }, default_for: [], machine: null, template: null, command: null },
    { by: 'builtin', name: 'openrouter', descriptor: { ...SOURCE_DESCRIPTOR, name: 'openrouter', title: 'OpenRouter' }, default_for: ['openrouter.ai'], machine: null, template: null, command: null },
    { by: 'program', name: 'acme', descriptor: null, default_for: [], machine: null, template: { id: 'template', name: 'Python provider' }, command: ['python', '/opt/plan.py'] },
    { by: 'program', name: 'mine', descriptor: { ...SOURCE_DESCRIPTOR, name: 'mine', title: 'GPU reports' }, default_for: [], machine, template: null, command: null },
    { by: 'program', name: 'theirs', descriptor: { ...SOURCE_DESCRIPTOR, name: 'theirs', title: 'Other reports' }, default_for: [], machine: other, template: null, command: null },
  ],
};

// The labelled trigger identifies the field directly; scope option accessibility to its open list.
// Repeated whole-dialog role walks dominate cold jsdom runs of this multi-step form.
async function pick(label: string, option: string, user:ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByLabelText(label, { selector: 'button' }));
  await user.click(within(screen.getByRole('listbox')).getByRole('option', { name: option }));
}

// Match this native button directly and still require its full accessible name. Avoid computing
// every other button's name/style for each action in a form containing two independent source editors.
function button(name:string,root:HTMLElement=document.body){
  const found=within(root).getByText(name,{selector:'button'});
  expect(found).toHaveAccessibleName(name);
  return found;
}

afterEach(async () => { cleanup();useAppearance.getState().set({ theme: 'system', locale: 'en' }); await setLocale('en'); });

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) describe(`Provider interface in ${language}, ${theme}`, { timeout: 30_000 }, () => {
  const t = i18n.getFixedT(language);
  beforeEach(async () => {
    await setLocale(language);
    useAppearance.getState().set({ theme, locale: language });
    vi.clearAllMocks();
    setup.subscriptions_unreported=[];setup.subscriptions_withheld=[];
    for(const choice of setup.machines ?? [])choice.upstreams=['*'];
    vi.mocked(createProvider).mockResolvedValue({...planProvider({ kind: 'api', key_set: false, status: 'needs_setup' }),warning:null});
  });

  async function address(url = 'https://gpu.example/v1') {
    const user = userEvent.setup();
    show(<AddProviderDialog onClose={close} />);
    await user.selectOptions(screen.getByLabelText('Home'), 'org');
    await screen.findByLabelText(t('resources.endpoint.protocol'), { selector: 'button' });
    await pick(t('resources.endpoint.protocol'), 'Chat Completions', user);
    expect(screen.getAllByText(t('resources.endpoint.oneAgent',{agents:'GPU assistant'}))).toHaveLength(1);
    expect(screen.queryByText(/Codex\s+(needs|需要)/u)).not.toBeInTheDocument();
    await user.click(screen.getByLabelText(t('resources.endpoint.address')));
    await user.paste(url);
    await pick(t('providerCreation.credential'), t('providerCreation.none'), user);
    await user.click(button(t('common.next')));
    return user;
  }

  it('keeps unavailable subscription examples visible with their actual reason',async()=>{
    setup.subscriptions_unreported=[{id:'cold',name:'Cold agent',address:'acme.cold',harness_kind:'custom',harness:{kind:'custom',name:'Cold agent',names:{}},machine:null,disabled:false}];
    setup.subscriptions_withheld=[{harness:{kind:'restricted',name:'Restricted plan',names:{}},scope:null}];
    show(<AddProviderDialog onClose={close}/>);
    await userEvent.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
    const picker=await screen.findByRole('combobox',{name:t('providerCreation.examples')});
    await userEvent.click(picker);
    for(const [name,reason] of [['Cold agent','subscriptionUnreported'],['Restricted plan','subscriptionWithheld']] as const){
      const option=screen.getByRole('option',{name});
      expect(option).toHaveAttribute('aria-disabled','true');
      expect(option).toHaveAccessibleDescription(t(`providerCreation.${reason}`));
    }
    expect(createProvider).not.toHaveBeenCalled();
  });

  async function finish(user: ReturnType<typeof userEvent.setup>, accepted = true) {
    await user.click(screen.getByLabelText(t('providers.add.name')));
    await user.paste('GPU model (custom adapter)');
    await user.click(button(t('providerCreation.create')));
    await waitFor(() => expect(createProvider).toHaveBeenCalledTimes(1));
    if(accepted){
      await waitFor(()=>expect(navigate).toHaveBeenCalledExactlyOnceWith({to:'/resources/providers/$id',params:{id:planProvider().id},search:{tab:'key'}}));
      expect(close).toHaveBeenCalledOnce();
    }else{
      expect(navigate).not.toHaveBeenCalled();expect(close).not.toHaveBeenCalled();
    }
    return vi.mocked(createProvider).mock.calls[0]![0];
  }

  it('creates a custom Chat Completions address with no credential, without a vendor choice or invented key', async () => {
    const user = await address();
    expect(screen.getByRole('group', { name: t('providerSources.detailsFrom') })).toBeVisible();
    expect(screen.getByRole('group', { name: t('providerSources.modelsFrom') })).toBeVisible();
    expect(screen.queryByLabelText(t('providerCreation.key'))).not.toBeInTheDocument();
    const body = await finish(user);
    expect(body).toMatchObject({ kind: 'api', home_id: 'org', upstreams: [{ protocol: 'openai-chat', base_url: 'https://gpu.example/v1', options: { auth: 'none' } }] });
    expect(body).not.toHaveProperty('api_key');
    expect(body).not.toHaveProperty('sources');
  });

  it('selects a real source program on the selected machine for both models and limits', async () => {
    const user = await address();
    await pick(t('providerCreation.machine'), 'GPU machine', user);
    for (const element of ['facts', 'models'] as const) {
      const label = t(element === 'facts' ? 'providerSources.detailsFrom' : 'providerSources.modelsFrom');
      const group = within(screen.getByRole('group', { name: label }));
      await user.click(group.getByLabelText(label, { selector: 'button' }));
      await user.click(within(screen.getByRole('listbox')).getByRole('option', { name: t('choiceCatalog.sourceKind.program.name') }));
      await user.click(group.getByLabelText(t('providerSources.registeredProgram'), { selector: 'button' }));
      const choices = within(screen.getByRole('listbox'));
      expect(choices.queryByRole('option', { name: 'Other reports' })).not.toBeInTheDocument();
      expect(choices.queryByRole('option', { name: /Python provider/ })).not.toBeInTheDocument();
      await user.click(choices.getByRole('option', { name: 'GPU reports' }));
    }
    const body = await finish(user);
    expect(body).toMatchObject({ keeper_machine_id: 'keeper', sources: { facts: { by: 'program', name: 'mine' }, models: { by: 'program', name: 'mine' } } });
    expect(body).not.toHaveProperty('api_key');
    expect(body).not.toHaveProperty('runtime_config');
  });

  it('uses the same fields for a preset, with the key added only afterwards', async () => {
    const user=userEvent.setup();
    show(<AddProviderDialog onClose={vi.fn()} />);
    await user.selectOptions(screen.getByLabelText('Home'), 'org');
    await screen.findByLabelText(t('resources.endpoint.protocol'),{selector:'button'});
    await pick(t('providerCreation.examples'), 'OpenRouter',user);
    expect(screen.getByLabelText(t('resources.endpoint.address'))).toHaveValue('https://openrouter.ai/api/v1');
    expect(screen.getByRole('combobox', { name: t('resources.endpoint.protocol') })).toHaveTextContent('Chat Completions');
    expect(screen.getByRole('combobox', { name: t('providerCreation.credential') })).toHaveTextContent(t('providerCreation.key'));
    await user.click(button(t('common.next')));
    expect(screen.getByRole('group', { name: t('providerSources.detailsFrom') })).toBeVisible();
    await user.click(button(t('providerCreation.create')));
    await waitFor(() => expect(createProvider).toHaveBeenCalledTimes(1));
    expect(vi.mocked(createProvider).mock.calls[0]![0]).toMatchObject({ kind: 'api', name: 'OpenRouter', upstreams: [{ protocol: 'openai-chat', base_url: 'https://openrouter.ai/api/v1' }] });
    expect(vi.mocked(createProvider).mock.calls[0]![0]).not.toHaveProperty('api_key');
    await waitFor(()=>expect(navigate).toHaveBeenCalledExactlyOnceWith({to:'/resources/providers/$id',params:{id:planProvider().id},search:{tab:'key'}}));
  });

  it('keeps template programs visible and preserves their exact registered command and template', async () => {
    const user = await address();
    for (const element of ['facts', 'models'] as const) {
      const label = t(element === 'facts' ? 'providerSources.detailsFrom' : 'providerSources.modelsFrom');
      const group = within(screen.getByRole('group', { name: label }));
      await user.click(group.getByLabelText(label, { selector: 'button' }));
      await user.click(within(screen.getByRole('listbox')).getByRole('option', { name: t('choiceCatalog.sourceKind.program.name') }));
      await user.click(group.getByLabelText(t('providerSources.registeredProgram'), { selector: 'button' }));
      await user.click(within(screen.getByRole('listbox')).getByRole('option', { name: t('providerSources.inTemplate', { program: 'acme', template: 'Python provider' }) }));
      expect(group.getByText('python /opt/plan.py')).toBeVisible();
      expect(group.getByText(t('providerSources.reportsUnread'))).toBeVisible();
    }
    const body = await finish(user);
    expect(body).toMatchObject({ runtime_config: { 'runtime.template_id': 'template' }, sources: { facts: { by: 'program', name: 'acme' }, models: { by: 'program', name: 'acme' } } });
  });

  it('opens the complete provider task from its one question mark without starting creation', async () => {
    const user = await address();
    const label = t('taskGuides.open', { title: t('taskGuides.addProvider') });
    expect(screen.getAllByRole('button', { name: label })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: label }));
    expect(guide).toHaveBeenCalledExactlyOnceWith('add-a-provider');
    expect(createProvider).not.toHaveBeenCalled();
  });
  it('registers a source inline through the machine registry and selects its returned declaration without another setup read',async()=>{
    const program:MachineProgram={kind:'provider_source',name:'gpu',command:['python','/opt/source.py'],origin:'orch',ready:true,version:'1',
      descriptor:{...SOURCE_DESCRIPTOR,name:'gpu',title:'GPU source'},models:null,detail:null};
    vi.mocked(changeMachinePrograms).mockResolvedValue({entries:[program],can_manage:true,capabilities:[],config_file:null});
    const user=await address();await pick(t('providerCreation.machine'),'GPU machine',user);
    await user.click(button(t('providerCreation.registerSource')));
    const dialog=await screen.findByRole('dialog',{name:t('machinePrograms.addTitle')});
    await user.click(within(dialog).getByLabelText(t('machinePrograms.name')));await user.paste('gpu');
    await user.click(within(dialog).getByLabelText(t('machinePrograms.command')));await user.paste('python /opt/source.py');
    await user.click(button(t('machinePrograms.register'),dialog));
    await waitFor(()=>expect(screen.queryByRole('dialog',{name:t('machinePrograms.addTitle')})).not.toBeInTheDocument());
    expect(changeMachinePrograms).toHaveBeenCalledExactlyOnceWith('keeper',{action:'register',kind:'provider_source',name:'gpu',command:['python','/opt/source.py']});
    const body=await finish(user);
    expect(body).toMatchObject({keeper_machine_id:'keeper',sources:{facts:{by:'program',name:'gpu'},models:{by:'program',name:'gpu'}}});
  });
  it('puts an endpoint refusal back on its field in the first step using the server words',async()=>{
    const user=await address();
    const words='This machine only forwards to the local GPU address.';
    vi.mocked(createProvider).mockRejectedValueOnce(new ApiError({type:'orch:error:request.invalid',code:'request.invalid',status:400,title:'Invalid request',detail:words,request_id:'request',
      errors:[{path:'upstreams[0].base_url',message:words}]},null));
    await finish(user,false);
    const endpoint=await screen.findByLabelText(t('resources.endpoint.address'));
    expect(endpoint).toHaveAttribute('aria-invalid','true');expect(endpoint).toHaveAccessibleDescription(words);
    await waitFor(()=>expect(endpoint).toHaveFocus());
    expect(button(t('common.cancel'))).toBeEnabled();
  });
  it('refuses a disallowed machine address locally and leaves a successful creation closed and non-repeatable',async()=>{
    const user=await address();
    const choice=setup.machines?.[0];if(choice===undefined)throw new Error('the keeping machine fixture is required');
    choice.upstreams=['https://allowed.example/v1'];
    await pick(t('providerCreation.machine'),'GPU machine',user);
    expect(screen.getByText(t('providerCreation.machineAddressRefused',{address:'https://gpu.example/v1'}))).toBeVisible();
    await user.click(button(t('providerCreation.create')));expect(createProvider).not.toHaveBeenCalled();
    await pick(t('providerCreation.machine'),t('providerCreation.whereConnectionRuns'),user);
    await finish(user);
    await waitFor(()=>expect(navigate).toHaveBeenCalledWith({to:'/resources/providers/$id',params:{id:'plan-provider'},search:{tab:'key'}}));
    expect(close).toHaveBeenCalledTimes(1);
    await user.click(button(t('providerCreation.create')));
    expect(createProvider).toHaveBeenCalledTimes(1);
  });
});
