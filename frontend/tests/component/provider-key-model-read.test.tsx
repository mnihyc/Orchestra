/** P7: one key save installs the reported model list; opening Models asks no second source. */
import { useQuery } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { providerKey, providerSuggestionsQuery, setProviderKey } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { ProviderKey } from '@/features/resources/providers/Key';
import { ProviderModels } from '@/features/resources/providers/Models';
import type { Provider, ProviderSetup } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
const setup:ProviderSetup={machines:[],key_machines:[],subscriptions:[],subscriptions_withheld:[],subscriptions_unshared:[],
  http_allowed:false,presets:[],api_arrangements:[],runtime_templates:[],sources:[],
  protocols:[{name:'openai-chat',title:'Chat Completions',agents:[],endpoint_options:[]}],};
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),setProviderKey:vi.fn(),
  providerSetupQuery:()=>({queryKey:['key-model-setup'],queryFn:()=>Promise.resolve(setup)}),
  providerSuggestionsQuery:vi.fn((id:string)=>({queryKey:['extra-model-read',id],queryFn:()=>Promise.resolve([])})),
}));
const readProvider=vi.fn<()=>Promise<Provider>>();
function Page({initial}:{initial:Provider}){
  const query=useQuery({queryKey:providerKey(initial.id),queryFn:readProvider,initialData:initial,staleTime:Infinity});
  return <><ProviderKey provider={query.data}/><ProviderModels provider={query.data} manage/></>;
}
afterEach(async()=>{cleanup();vi.clearAllMocks();await setLocale('en');});
it.each(['en','zh-CN'] as const)('shows the pending read and then the returned suggestions without Refresh in %s',async(locale)=>{
  await setLocale(locale);const user=userEvent.setup();
  const initial=planProvider({kind:'api',key_set:false,models:[],upstreams:[{protocol:'openai-chat',base_url:'https://openrouter.ai/api/v1',options:{}}]});
  initial.facts.models=null;initial.facts_state.models='not_read_yet';initial.facts.field_times={};
  const saved=structuredClone(initial);saved.key_set=true;saved.facts_state.models='told';
  saved.facts.models=[{model_name:'offered-1',display_name:'Model offered for this key',listed:true,parameters:{}}];
  saved.facts.field_times.models='2026-10-16T00:00:00Z';
  let acknowledge:((provider:Provider)=>void)|undefined;
  vi.mocked(setProviderKey).mockImplementation(()=>new Promise<Provider>((resolve)=>{acknowledge=resolve;}));
  show(<Page initial={initial}/>);
  const key=await screen.findByLabelText(i18n.t('providers.key.value'));
  await user.click(key);await user.paste('sk-only-for-the-component-test');
  await user.click(screen.getByRole('button',{name:i18n.t('providers.key.save')}));
  await screen.findByRole('button',{name:i18n.t('providerNativeLogin.readingModels')});
  expect(key).toBeDisabled();expect(readProvider).not.toHaveBeenCalled();
  const accepted=acknowledge;if(accepted===undefined)throw new Error('key write did not start');
  await act(async()=>{accepted(saved);await Promise.resolve();});
  expect(await screen.findByText('Model offered for this key',{exact:true})).toBeVisible();
  await waitFor(()=>expect(key).toHaveValue(''));
  expect(setProviderKey).toHaveBeenCalledExactlyOnceWith(initial.id,'sk-only-for-the-component-test');
  expect(providerSuggestionsQuery).not.toHaveBeenCalled();expect(readProvider).not.toHaveBeenCalled();
  expect(screen.getByText(i18n.t('providers.models.none'))).toBeVisible();
});
