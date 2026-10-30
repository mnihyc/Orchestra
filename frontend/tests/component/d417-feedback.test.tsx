/** Actual refusal facts, catalogued recovery words and two equal provider names are usable on screen. */
import {screen,waitFor,within} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {afterEach,beforeEach,describe,expect,it,vi} from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import {ApiError} from '@/api/client';
import {MissingDefault} from '@/features/start/MissingDefault';
import {NO_PICKS,resolve} from '@/features/start/choices';
import {sourceLine} from '@/features/source/Preparation';
import {blockedDetail} from '@/features/timeline/waiting';
import {placeOfNode} from '@/features/navigation/places';
import type {Project,Provider,SourceInfo} from '@/generated/api-types';
import {i18n,setLocale} from '@/i18n';
import {ErrorAlert} from '@/ui/ErrorState';
import {org,me,userRef} from '../doubles/fixtures';
import {agent,options,provider} from '../unit/start-fixtures';
import {planProvider} from '../unit/provider-plan-fixtures';
import {show} from './support';

vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
const providerRead=vi.hoisted(()=>vi.fn<()=>Promise<Provider[]>>());
vi.mock('@/api/resources',async(original)=>({...await original<typeof ResourcesApi>(),providersQuery:()=>({queryKey:['d438-provider-origins'],queryFn:providerRead})}));
vi.mock('@/features/navigation/placeLabels',()=>({useKnownPlaces:()=>new Map()}));
const first='2026-10-15T10:00:00.000Z',second='2026-10-15T10:01:00.000Z';
beforeEach(()=>{providerRead.mockReset();providerRead.mockResolvedValue([]);});
afterEach(async()=>{await setLocale('en');});

for (const language of ['en','zh-CN'] as const) describe('resource feedback in '+language,()=>{
  // Selecting a language loads its lazy catalogue; cloning an unloaded language does not.
  beforeEach(async()=>{await setLocale(language);});
  const t=i18n.getFixedT(language);
  it('lists both offline hosts with their observed times in the refusal recovery',async()=>{
    const error=new ApiError({type:'orch:error:plan.blocked',code:'plan.blocked',status:409,title:'Blocked',detail:'No host',request_id:'request',host_blockers:[
      {host:{id:'a',name:'docker-1',address:'a'},online:false,offline_since:first,message:'docker-1 is offline'},
      {host:{id:'b',name:'docker-2',address:'b'},online:false,offline_since:second,message:'docker-2 is offline'},
    ]},null);
    show(<ErrorAlert error={error} />);
    await userEvent.click(screen.getByRole('button',{name:t('ui.error.review')}));
    const list=screen.getByRole('list',{name:t('planHosts.title')});
    expect(within(list).getByText('docker-1')).toBeVisible();expect(within(list).getByText('docker-2')).toBeVisible();
    expect(list.querySelector('time[datetime="'+first+'"]')).not.toBeNull();
    expect(list.querySelector('time[datetime="'+second+'"]')).not.toBeNull();
    expect(within(list).getAllByText(t('planHosts.offline'))).toHaveLength(2);
  });
  it('translates only fixed engine explanations and names the workspace naturally',()=>{
    for (const key of ['approvalNotAllowed','approvalUnsupported','sandboxUnsupported'] as const) {
      const message=i18n.getFixedT('en')(`threadConfinement.${key}`);
      expect(blockedDetail(message,t)).toBe(t(`threadConfinement.${key}`));
    }
    expect(blockedDetail('the vendor supplied this exact detail',t)).toBe('the vendor supplied this exact detail');
    expect(blockedDetail("no sandbox mode at or below Workspace can be enforced here: change the setting, the environment's limit or the environment",t)).toBe(t('threadConfinement.sandboxUnsupported'));
    const source:SourceInfo={directory:'',kind:'directory',state:'ready',repository_url:null,branch:null,head:null,job:null,reason:null};
    expect(sourceLine(t,source)).toBe(t('source.prepare.existing',{directory:t('source.prepare.workspace')}));
    expect(sourceLine(t,source)).not.toContain('/');
  });
  it('keeps the agent startup cause in a localized recovery and translates retained approval enum names',()=>{
    const cause='llm-pi-ai: provider "orch" has no compatible protocol';
    const shown=blockedDetail(`harness_unavailable: ${cause}`,t);
    expect(shown).toBe(t('threadConfinement.agentRefused',{detail:cause}));
    expect(shown).toContain(cause);
    expect(shown).not.toContain('harness_unavailable');
    for (const mode of ['Ask','OnRequest','Auto','Bypass']) {
      const unsupported=blockedDetail(`the agent does not have the approval mode ${mode}`,t);
      expect(unsupported).toBe(t('threadConfinement.approvalUnsupported'));
      expect(unsupported).not.toContain(mode);
      expect(blockedDetail(`the approval mode ${mode} is not among those allowed here`,t)).toBe(t('threadConfinement.approvalNotAllowed'));
    }
  });
  it('says when the offline time is unknown and ignores malformed host records',async()=>{
    const error=new ApiError({type:'orch:error:plan.blocked',code:'plan.blocked',status:409,title:'Blocked',detail:'No host',request_id:'request',host_blockers:[
      {host:{id:'a',name:'docker-1'},online:false,offline_since:'not-a-time',message:'offline'},
      {host:null,online:false,offline_since:first,message:'not an authorized host record'},
    ]},null);
    show(<ErrorAlert error={error} />);
    await userEvent.click(screen.getByRole('button',{name:t('ui.error.review')}));
    const list=screen.getByRole('list',{name:t('planHosts.title')});
    const rows=within(list).getAllByRole('listitem');
    expect(rows).toHaveLength(1);
    expect(rows[0]).toBeVisible();
    expect(rows[0]).toHaveTextContent(t('planHosts.unknownSince'));
    expect(list.querySelector('time')).toBeNull();
    expect(screen.queryByText('not an authorized host record')).not.toBeInTheDocument();
  });
});

it('tells equal provider names apart in the actual inline start picker and sends the chosen id',async()=>{
  const root=org('org','acme','Acme',userRef(me(1,'ada','Ada')));
  const project:Project={...root,kind:'project',id:'project',name:'Web',parent_id:root.id,topic_count:0,project_count:0};
  const a=provider('a'),b=provider('b');
  a.provider.name=b.provider.name='Scripted model';a.provider.address='acme.ada.scripted';b.provider.address='beta.grace.scripted';
  const answer=options({providers:[a,b],defaults:{provider_id:null,agent_id:null,model_name:null,effort:null,locked:false}});
  providerRead.mockResolvedValue([
    planProvider({id:'a',name:'Scripted model',home:{id:'acme',kind:'org',name:'Acme',redacted:false},org_id:'acme'}),
    planProvider({id:'b',name:'Scripted model',home:{id:'beta',kind:'org',name:'Beta',redacted:false},org_id:'beta'}),
  ]);
  const onPick=vi.fn();
  show(<MissingDefault place={placeOfNode(project)} node={project} options={answer} resolved={resolve(answer,NO_PICKS,{suggest:false})} picks={NO_PICKS}
    onPick={onPick} scopes={[]} keepAt={null} onKeepAt={vi.fn()} onChange={vi.fn()} />);
  await userEvent.click(await screen.findByRole('combobox',{name:i18n.t('start.provider.title')}));
  expect(screen.getByRole('option',{name:'Scripted model (Acme)'})).toBeVisible();
  await userEvent.click(screen.getByRole('option',{name:'Scripted model (Beta)'}));
  expect(onPick).toHaveBeenCalledWith({provider:'b',agent:null,model:null,effort:null,arrangement:null});
  expect(providerRead).toHaveBeenCalledTimes(1);
  expect(document.body).not.toHaveTextContent('acme.ada.scripted');
  expect(document.body).not.toHaveTextContent('beta.grace.scripted');
});

it('does not read provider origins when the offered names are unique',async()=>{
  const root=org('org','acme','Acme',userRef(me(1,'ada','Ada')));
  const project:Project={...root,kind:'project',id:'project',name:'Web',parent_id:root.id,topic_count:0,project_count:0};
  const answer=options({defaults:{provider_id:null,agent_id:null,model_name:null,effort:null,locked:false}});
  show(<MissingDefault place={placeOfNode(project)} node={project} options={answer} resolved={resolve(answer,NO_PICKS,{suggest:false})} picks={NO_PICKS}
    onPick={vi.fn()} scopes={[]} keepAt={null} onKeepAt={vi.fn()} onChange={vi.fn()} />);
  await userEvent.click(screen.getByRole('combobox',{name:i18n.t('start.provider.title')}));
  expect(screen.getByRole('option',{name:'Provider p1'})).toBeVisible();
  expect(providerRead).not.toHaveBeenCalled();
});

for (const language of ['en','zh-CN'] as const) it('names the offered profile’s reported program in '+language,async()=>{
  await setLocale(language);
  const root=org('org','acme','Acme',userRef(me(1,'ada','Ada')));
  const project:Project={...root,kind:'project',id:'project',name:'Web',parent_id:root.id,topic_count:0,project_count:0};
  const first=agent('a1');
  const named={harness:{kind:'research-agent',name:'Research assistant',names:{'zh-CN':'研究助理'}}};
  const offered={...first,agent:{...first.agent,...named,name:'Reviewer',harness_kind:'research-agent'}};
  const answer=options({providers:[provider('p1',{agents:[offered,agent('a2')]})],defaults:{provider_id:'p1',agent_id:null,model_name:'m1',effort:null,locked:false}});
  show(<MissingDefault place={placeOfNode(project)} node={project} options={answer} resolved={resolve(answer,NO_PICKS,{suggest:false})} picks={NO_PICKS}
    onPick={vi.fn()} scopes={[]} keepAt={null} onKeepAt={vi.fn()} onChange={vi.fn()} />);
  await userEvent.click(screen.getByRole('combobox',{name:i18n.t('start.agent.title')}));
  const option=screen.getByRole('option',{name:/Reviewer/});
  expect(option).toHaveTextContent(language==='zh-CN'?'研究助理':'Research assistant');
  expect(option).not.toHaveTextContent('research-agent');
});

it('keeps a colliding-name picker unavailable until readable origins arrive and offers Retry after refusal',async()=>{
  const root=org('org','acme','Acme',userRef(me(1,'ada','Ada')));
  const project:Project={...root,kind:'project',id:'project',name:'Web',parent_id:root.id,topic_count:0,project_count:0};
  const a=provider('a'),b=provider('b');a.provider.name=b.provider.name='Shared';
  const answer=options({providers:[a,b],defaults:{provider_id:null,agent_id:null,model_name:null,effort:null,locked:false}});
  providerRead.mockRejectedValueOnce(new Error('Origin read refused'));
  const onPick=vi.fn();
  show(<MissingDefault place={placeOfNode(project)} node={project} options={answer} resolved={resolve(answer,NO_PICKS,{suggest:false})} picks={NO_PICKS}
    onPick={onPick} scopes={[]} keepAt={null} onKeepAt={vi.fn()} onChange={vi.fn()} />);
  expect(await screen.findByRole('combobox',{name:i18n.t('startFlow.providerLocationsFailure')})).toBeDisabled();
  providerRead.mockResolvedValue([
    planProvider({id:'a',name:'Shared',home:{id:'acme',kind:'org',name:'Acme',redacted:false}}),
    planProvider({id:'b',name:'Shared',home:{id:'beta',kind:'org',name:'Beta',redacted:false}}),
  ]);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('common.tryAgain')}));
  await waitFor(()=>expect(screen.getByRole('combobox',{name:i18n.t('start.provider.title')})).toBeEnabled());
  expect(providerRead).toHaveBeenCalledTimes(2);expect(onPick).not.toHaveBeenCalled();
});
