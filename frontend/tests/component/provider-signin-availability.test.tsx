/** R2c: a cold program remains named with a reason, a recovery act, and a compact creation flow. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeAll, beforeEach, expect, it, vi } from 'vitest';
import { createProvider } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { AddProviderDialog } from '@/features/resources/providers/AddProvider';
import type { ProviderSetup } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>vi.fn(),Link:({children}:{children:ReactNode})=><a href="#profile">{children}</a>}));
vi.mock('@/features/resources/HomePicker',()=>({
  HomePicker:({value,onChange}:Pick<HomePickerProps,'value'|'onChange'>)=><select aria-label="Home" value={value} onChange={(event)=>onChange(event.target.value)}><option value="">Choose home</option><option value="org">Acme</option></select>,
  useHomeChoices:()=>({choices:[{id:'org',label:'Acme',personal:false,department:false}],pending:false,error:null}),
}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),createProvider:vi.fn(),
  providerSetupQuery:()=>({queryKey:['availability-setup'],queryFn:()=>Promise.resolve(setup)}),
  harnessKindsQuery:{queryKey:['availability-kinds'],queryFn:()=>Promise.resolve([
    {kind:'codex',title:'Codex',builds:[],descriptor:null,program:null,harness:harnessName('codex')},{kind:'claude-code',title:'Claude Code',builds:[],descriptor:null,program:null,harness:harnessName('claude-code')},
  ])},
}));
const profile={id:'scripted',name:'Scripted agent',address:'acme.scripted',harness_kind:'fake',harness:harnessName('fake'),machine:null,disabled:false};
const scriptedHarness={kind:'fake',name:'Scripted subscription',names:{'zh-CN':'脚本订阅'}};
const setup:ProviderSetup={
  machines:[{machine:{id:'box',name:'Team machine',online:true,placement:'connected',platform_os:'linux'},directory_runtime:true,
    roles:['agent','provider'],harnesses:['fake','codex','claude-code'],upstreams:['*']}],key_machines:[],
  subscriptions:[{agent:profile,harness:scriptedHarness,methods:[],options_schema:{},
    arrangements:[{type:'separated',allowed:true,available:true,puts:'',form:null}],template:{id:'template',name:'Provider runtime'}}],
  subscriptions_unavailable:[
    {harness:harnessName('codex'),agent:{...profile,id:'codex',name:'Codex',harness_kind:'codex',harness:harnessName('codex')},reason:'not_reported'},
    {harness:harnessName('claude-code'),agent:null,reason:'profile_needed'},
  ],
  subscriptions_unreported:[{...profile,id:'codex',name:'Codex',harness_kind:'codex',harness:harnessName('codex')}],
  subscriptions_withheld:[],subscriptions_unshared:[],http_allowed:false,runtime_templates:[],api_arrangements:[],
  presets:[{name:'OpenRouter',note:null,docs_url:null,upstreams:[{protocol:'openai-chat',base_url:'https://openrouter.ai/api/v1',options:{}}]}],
  protocols:[{name:'openai-chat',title:'Chat Completions',agents:[],endpoint_options:[]},
    {name:'openai-responses',title:'Responses',agents:[],endpoint_options:[]},
    {name:'anthropic-messages',title:'Messages',agents:[],endpoint_options:[]}],
  sources:[{by:'builtin',name:'adapter',descriptor:{...SOURCE_DESCRIPTOR,title:'Harness adapter'},default_for:[],machine:null,template:null,command:null}],
};
beforeEach(()=>{vi.clearAllMocks();});
// This suite observes availability and drafts; cold code loading is exercised in the browser.
beforeAll(async()=>{await import('@/features/resources/agents/AddAgent');});
afterEach(async()=>{cleanup();useAppearance.getState().set({theme:'system',locale:'en'});await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const){
  async function open(){
    await setLocale(locale);useAppearance.getState().set({locale,theme});
    const user=userEvent.setup();const close=vi.fn();show(<AddProviderDialog onClose={close}/>);
    await user.selectOptions(screen.getByLabelText('Home'),'org');
    await screen.findByLabelText(i18n.t('resources.endpoint.protocol'),{selector:'button'});
    return {user,close,t:i18n.getFixedT(locale)};
  }

  it(`${locale}/${theme} names Codex and Claude Code with distinct reasons and keeps Add profile reachable`,async()=>{
    const {user,close,t}=await open();
    await user.click(screen.getByLabelText(t('providerCreation.reach'),{selector:'button'}));
    await user.click(within(screen.getByRole('listbox')).getByRole('option',{name:t('providerCreation.native')}));
    await user.click(screen.getByLabelText(t('providerCreation.program'),{selector:'button'}));
    const list=within(screen.getByRole('listbox'));
    for(const [name,reason] of [['Codex','not_reported'],['Claude Code','profile_needed']] as const){
      const choices=list.getAllByRole('option',{name});expect(choices).toHaveLength(1);
      expect(choices[0]).toHaveAttribute('aria-disabled','true');
      expect(choices[0]).toHaveAccessibleDescription(t(`providerCreation.subscriptionUnavailable.${reason}`));
    }
    expect(list.getByRole('option',{name:locale==='zh-CN'?'Scripted agent (脚本订阅)':'Scripted agent (Scripted subscription)'})).not.toHaveAttribute('aria-disabled');
    await user.keyboard('{Escape}');
    const next=screen.getByRole('button',{name:t('common.next')});
    expect(next).toHaveAttribute('aria-disabled','true');
    await user.click(next);expect(next).toHaveFocus();await user.keyboard('{Enter}');
    expect(screen.getByLabelText(t('providerCreation.program'),{selector:'button'})).toBeVisible();
    expect(screen.queryByLabelText(t('providerCreation.signInMachine'),{selector:'button'})).not.toBeInTheDocument();
    expect(createProvider).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button',{name:t('resourceCreation.agents')}));
    const add=within(await screen.findByRole('dialog',{name:t('agents.add.title')}));
    expect(add.getByLabelText('Home')).toHaveValue('org');
    expect(await add.findByRole('combobox',{name:t('agents.add.harness')})).toBeVisible();
    await user.click(add.getByRole('button',{name:t('common.cancel')}));
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByLabelText(t('providerCreation.reach'),{selector:'button'})).toHaveTextContent(t('providerCreation.native'));
    expect(createProvider).not.toHaveBeenCalled();
  });

  it(`${locale}/${theme} puts examples first and names no incompatibility until a format is selected`,async()=>{
    const {user,t}=await open();
    const examples=screen.getByLabelText(t('providerCreation.examples'),{selector:'button'});
    const format=screen.getByLabelText(t('resources.endpoint.protocol'),{selector:'button'});
    expect(examples.compareDocumentPosition(format)&Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(screen.queryByText(t('resources.endpoint.noAgent'))).not.toBeInTheDocument();
    await user.click(format);
    const options=within(screen.getByRole('listbox')).getAllByRole('option');expect(options).toHaveLength(3);
    for(const option of options)expect(option).not.toHaveAccessibleDescription();
    await user.click(within(screen.getByRole('listbox')).getByRole('option',{name:'Messages'}));
    expect(screen.getAllByText(t('resources.endpoint.noAgent'))).toHaveLength(1);
    await user.click(examples);
    const unavailable=within(screen.getByRole('listbox')).getByRole('option',{name:'Claude Code'});
    expect(unavailable).toHaveAccessibleDescription(t('providerCreation.subscriptionUnavailable.profile_needed'));
    expect(createProvider).not.toHaveBeenCalled();
  });

  it(`${locale}/${theme} explains step two with credential and connection locations and short native source names`,async()=>{
    const {user,t}=await open();
    await user.click(screen.getByLabelText(t('providerCreation.examples'),{selector:'button'}));
    await user.click(within(screen.getByRole('listbox')).getByRole('option',{name:locale==='zh-CN'?'Scripted agent (脚本订阅)':'Scripted agent (Scripted subscription)'}));
    await user.click(screen.getByRole('button',{name:t('common.next')}));
    expect(screen.getByLabelText(t('providerCreation.signInMachine'),{selector:'button'}).textContent).toBe(t('providerCreation.whereConnectionRuns'));
    expect(screen.getByLabelText(t('providerCreation.runtime'),{selector:'button'}).textContent).toBe(t('providerCreation.newContainer'));
    for(const key of ['detailsFrom','modelsFrom'] as const)expect(screen.getByLabelText(t(`providerSources.${key}`),{selector:'button'}).textContent).toBe(t('providerCreation.signedInProgram'));
    expect(screen.queryByText('Harness adapter',{exact:true})).not.toBeInTheDocument();
    await user.click(screen.getByLabelText(t('providerCreation.runtime'),{selector:'button'}));
    for(const option of within(screen.getByRole('listbox')).getAllByRole('option'))expect(option).not.toHaveAccessibleDescription();
    expect(createProvider).not.toHaveBeenCalled();
  });
}
