/** The source's actual proposals become saved models and native readiness in the same act. */
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { QueryClient,QueryClientProvider,useQuery } from '@tanstack/react-query';
import { act,cleanup,screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,expect,it,vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { acceptSuggestions,providerKey } from '@/api/resources';
import { answerChanges } from '@/api/actAnswers';
import { applyChanges } from '@/api/acts';
import { ProviderModels } from '@/features/resources/providers/Models';
import { ProviderSetupNotice } from '@/features/resources/providers/SetupNotice';
import type { Provider,ProviderModel,ProviderModelSuggestionAcceptReply } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { planProvider,SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),acceptSuggestions:vi.fn()}));
const read=vi.fn<()=>Promise<Provider>>();
function Page({initial}:{initial:Provider}) {
  const query=useQuery({queryKey:providerKey(initial.id),queryFn:read,initialData:initial,staleTime:Infinity});
  return <><ProviderSetupNotice provider={query.data} manage currentTab="models" onTab={vi.fn()}/><ProviderModels provider={query.data} manage/></>;
}
function sourceProvider():Provider {
  const provider=planProvider({kind:'api',status:'needs_setup',status_detail:'it needs an enabled model',models:[],
    upstreams:[{protocol:'openai-chat',base_url:'https://gpu.example/v1',options:{auth:'none'}}]});
  provider.sources.models={...provider.sources.models,by:'program',name:'gpu-box',descriptor:{...SOURCE_DESCRIPTOR,name:'gpu-box',title:'GPU box'}};
  provider.facts_state.models='told';
  provider.facts.models=[{model_name:'gpu-small',display_name:'GPU Small',listed:true,parameters:{context_window:32768}},
    {model_name:'gpu-large',display_name:'GPU Large',listed:true,parameters:{context_window:131072}}];
  return provider;
}
afterEach(async()=>{cleanup();vi.clearAllMocks();delete document.documentElement.dataset.theme;await setLocale('en');});

for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'])it(`accepts GPU Small and GPU Large without a false setup error or another read (${locale}, ${theme})`,async()=>{
  await setLocale(locale);document.documentElement.dataset.theme=theme;
  const initial=sourceProvider();const client=new QueryClient({defaultOptions:{queries:{retry:false},mutations:{retry:false}}});
  const added:ProviderModel[]=(initial.facts.models ?? []).map((proposal)=>({...MODEL_ESTIMATE,id:proposal.model_name,model_name:proposal.model_name,
    display_name:proposal.display_name,parameters:proposal.parameters,groups:[],enabled:true,source:'suggested'}));
  const ready={...initial,status:'ready' as const,status_detail:null,models:added};
  const reply:ProviderModelSuggestionAcceptReply={added,provider:ready};
  let finish:(()=>Promise<void>)|undefined;
  vi.mocked(acceptSuggestions).mockImplementation((id,names)=>new Promise<ProviderModel[]>((resolve)=>{
    finish=async()=>{
      await applyChanges(answerChanges({method:'POST',path:'/providers/{id}/models/suggestions/accept',input:{model_names:names},answer:reply},
        (part)=>part==='id'?id:'',[{key:['provider'],id}]),client);
      resolve(reply.added);
    };
  }));
  const user=userEvent.setup();show(<QueryClientProvider client={client}><Page initial={initial}/></QueryClientProvider>);
  expect(await screen.findByText('GPU Small',{exact:true})).toBeVisible();expect(screen.getByText('GPU Large',{exact:true})).toBeVisible();
  expect(screen.queryByText(i18n.t('providers.phrase.needsModel'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:i18n.t('providers.models.addAll')}));
  expect(acceptSuggestions).toHaveBeenCalledExactlyOnceWith(initial.id,['gpu-small','gpu-large']);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  const acknowledge=finish;if(acknowledge===undefined)throw new Error('model acceptance was not issued');
  await act(async()=>{await acknowledge();});
  await waitFor(()=>expect(screen.queryByRole('button',{name:i18n.t('providers.models.addAll')})).not.toBeInTheDocument());
  for(const name of ['GPU Small','GPU Large'])expect(screen.getByRole('switch',{name:i18n.t('providers.models.enabledLabel',{name})})).toBeChecked();
  expect(client.getQueryData(providerKey(initial.id))).toEqual(ready);
  expect(screen.queryByText(i18n.t('providers.phrase.needsModel'))).not.toBeInTheDocument();expect(read).not.toHaveBeenCalled();
});

it('keeps an actual models-source failure visible on its recovery panel',()=>{
  const provider=sourceProvider();provider.sources.models.last_error={code:'unavailable',message:'GPU box could not be read',at:'2026-10-16T00:00:00Z'};
  show(<Page initial={provider}/>);
  expect(screen.getByRole('alert')).toBeVisible();
  expect(screen.queryByText(i18n.t('providers.phrase.needsModel'))).not.toBeInTheDocument();
});
