/** The source's actual proposals become saved models and native readiness in the same act. */
import { MODEL_ESTIMATE } from '../doubles/fixtures';
import { QueryClient,QueryClientProvider,useQuery } from '@tanstack/react-query';
import { act,cleanup,screen,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,expect,it,vi } from 'vitest';
import type * as Resources from '@/api/resources';
import { acceptSuggestions,createModel,updateModel,providerKey } from '@/api/resources';
import { answerChanges } from '@/api/actAnswers';
import { applyChanges } from '@/api/acts';
import { AddModelDialog, ModelEditorDialog } from '@/features/resources/providers/ModelEditor';
import { ProviderModels } from '@/features/resources/providers/Models';
import { ProviderSetupNotice } from '@/features/resources/providers/SetupNotice';
import type { Provider,ProviderModel,ProviderModelSuggestionAcceptReply } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { formatNumber } from '@/lib/format';
import { planProvider,SOURCE_DESCRIPTOR } from '../unit/provider-plan-fixtures';
import { show } from './support';

vi.mock('@/features/resources/providers/Caps', () => ({ ModelLimits: () => null }));
vi.mock('@/api/resources',async(load)=>({...await load<typeof Resources>(),acceptSuggestions:vi.fn(),createModel:vi.fn(),updateModel:vi.fn()}));
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

it('says that adding switches on what a model offers, and offers the suggestions as what they offer', async () => {
  const provider=sourceProvider();
  provider.facts.models=[{model_name:'gpu-small',display_name:'GPU Small',listed:true,parameters:{context_window:32768,offered:{service_tiers:[{id:'fast',label:'Fast'}]}}}];
  show(<Page initial={provider}/>);
  expect(await screen.findByText(i18n.t('models.list.suggestionsOffer'))).toBeVisible();
  expect(screen.getByText(i18n.t('models.list.offers',{list:'Fast'}))).toBeVisible();
  expect(screen.queryByText(/Offered, not switched on/)).not.toBeInTheDocument();
});

it('shows what an added model has switched on in its row, with nothing left offered', () => {
  const provider=sourceProvider();
  const model:ProviderModel={...MODEL_ESTIMATE,id:'gpu-small',model_name:'gpu-small',display_name:'GPU Small',groups:[],enabled:true,source:'suggested',
    parameters:{context_window:272000,context_max:1000000,default_context:272000,service_tiers:[{id:'fast',label:'Fast'}],access_programs:['early'],efforts:['low','high'],
      offered:{context_max:1000000,service_tiers:[{id:'fast',label:'Fast'}],access_programs:['early']}}};
  show(<Page initial={{...provider,models:[model]}}/>);
  const row=screen.getAllByRole('row').find((one)=>one.textContent.includes('GPU Small'));
  if(row===undefined)throw new Error('The added model has a row');
  expect(row).toHaveTextContent('Fast');expect(row).toHaveTextContent('early');
  expect(row).not.toHaveTextContent('Offered, not switched on');
});

for(const locale of ['en','zh-CN'] as const)it(`keeps an unknown window optional and shows the catalogue fallback when one is known (${locale})`,async()=>{
  await setLocale(locale);
  const user=userEvent.setup();const provider=sourceProvider();const onClose=vi.fn();
  vi.mocked(createModel).mockResolvedValue({...MODEL_ESTIMATE,id:'gpu-small',model_name:'gpu-small',display_name:'GPU Small',groups:[],enabled:true,source:'manual',parameters:{}});
  const view=show(<AddModelDialog provider={provider} onClose={onClose}/>);
  const optional=i18n.t('ui.field.optional'),window=i18n.t('models.editor.context.window'),routing=screen.getByLabelText(i18n.t('providers.models.routingName'));
  // An unknown window remains unknown; adding does not require a guessed fact.
  await user.type(routing,'unlisted');
  expect(screen.getByLabelText(`${window} ${optional}`)).toBeInTheDocument();
  expect(screen.getByLabelText(`${i18n.t('providers.models.effortsLabel')} ${optional}`)).toHaveAttribute('placeholder',i18n.t('models.editor.none'));
  await user.click(screen.getByRole('button',{name:i18n.t('providers.models.addSubmit')}));
  await waitFor(()=>expect(createModel).toHaveBeenCalledExactlyOnceWith(provider.id,{model_name:'unlisted'}));
  view.unmount();vi.mocked(createModel).mockClear();
  show(<AddModelDialog provider={provider} onClose={onClose}/>);
  // A name the catalogue lists reports its window, which fills what is left empty.
  await user.type(screen.getByLabelText(i18n.t('providers.models.routingName')),'gpu-small');
  const field=screen.getByLabelText(`${window} ${optional}`);
  expect(field).toHaveAttribute('placeholder',i18n.t('models.editor.fromCatalogue',{value:formatNumber(32768,locale)}));
  await user.click(screen.getByRole('button',{name:i18n.t('providers.models.addSubmit')}));
  await waitFor(()=>expect(createModel).toHaveBeenCalledExactlyOnceWith(provider.id,{model_name:'gpu-small'}));
  await waitFor(()=>expect(onClose).toHaveBeenCalled());
});

for (const locale of ['en', 'zh-CN'] as const) it(`saves efforts and rates without inventing an unknown window (${locale})`, async () => {
  await setLocale(locale);
  const provider = sourceProvider();
  const model: ProviderModel = { ...MODEL_ESTIMATE, id: 'unknown', model_name: 'unknown', display_name: 'Unknown window', enabled: true, source: 'suggested', groups: [], parameters: {} };
  vi.mocked(updateModel).mockResolvedValue(model);
  const user = userEvent.setup();
  show(<ModelEditorDialog provider={provider} model={model} onClose={vi.fn()} />);
  await user.type(screen.getByLabelText(new RegExp(`^${i18n.t('providers.models.effortsLabel')}`)), 'low, high');
  for (const key of ['rateInput', 'rateCached', 'rateOutput']) await user.type(screen.getByLabelText(new RegExp(`^${i18n.t(`providers.models.${key}`)}`)), '1.25');
  await user.click(screen.getByRole('button', { name: i18n.t('providers.models.save') }));
  await waitFor(() => expect(updateModel).toHaveBeenCalledExactlyOnceWith(provider.id, model.id, { groups: [], parameters: {
    efforts: ['low', 'high'], rate: { input_per_mtok: '1.25', cached_input_per_mtok: '1.25', output_per_mtok: '1.25' },
  } }));
});

it('uses the catalogue window for an inherited context start after clearing model facts', async () => {
  const provider = sourceProvider();
  provider.facts.models = [{ model_name: 'gpu-small', display_name: 'GPU Small', listed: true,
    parameters: { context_window: 272000, efforts: ['low', 'high'], offered: { context_max: 1000000 } } }];
  const model: ProviderModel = { ...MODEL_ESTIMATE, id: 'small', model_name: 'gpu-small', display_name: 'GPU Small', enabled: true, source: 'manual', groups: [],
    parameters: { context_window: 400000, context_max: 1000000, default_context: 400000, efforts: ['low'], offered: { context_max: 1000000 } } };
  vi.mocked(updateModel).mockResolvedValue(model);
  const user = userEvent.setup();
  show(<ModelEditorDialog provider={provider} model={model} onClose={vi.fn()} />);
  const window = screen.getByLabelText(new RegExp(`^${i18n.t('models.editor.context.window')}`));
  await user.clear(window);
  await user.clear(screen.getByLabelText(new RegExp(`^${i18n.t('providers.models.effortsLabel')}`)));
  expect(window).toHaveAttribute('placeholder', i18n.t('models.editor.fromCatalogue', { value: formatNumber(272000, 'en') }));
  await user.click(screen.getByRole('button', { name: i18n.t('providers.models.save') }));
  await waitFor(() => expect(updateModel).toHaveBeenCalledExactlyOnceWith(provider.id, model.id, { groups: [], parameters: {
    context_max: 1000000, default_context: 272000, offered: { context_max: 1000000 },
  } }));
});
