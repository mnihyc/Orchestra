/** Creating a template asks for essentials, retains the complete example and opens the returned page. */
import { fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Resources from '@/api/resources';
import { NewTemplateDialog } from '@/features/resources/templates/NewTemplate';
import { QuickTemplateFields } from '@/features/resources/templates/QuickTemplateFields';
import { setLocale } from '@/i18n';
import { useSelectedOrg } from '@/shell/org';
import { hostTemplateSchema, templateSchema } from '../doubles/template-specs';
import { show } from './support';
import { detailTemplate } from './resource-detail-fixtures';

const calls=vi.hoisted(()=>({create:vi.fn<typeof Resources.createTemplate>(),check:vi.fn(),navigate:vi.fn(),close:vi.fn()}));
vi.mock('@tanstack/react-router',async(original)=>({...await original<object>(),useNavigate:()=>calls.navigate}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
vi.mock('@/api/nodes',async(original)=>({...await original<object>(),orgsQuery:{queryKey:['short-template-orgs'],queryFn:()=>Promise.resolve([{id:'acme',name:'Acme',org_kind:'team'},{id:'beta',name:'Beta',org_kind:'team'}])},fetchTree:()=>Promise.resolve({nodes:[]}),nodeQuery:()=>({queryKey:['short-template-node'],queryFn:()=>Promise.resolve({id:'beta',kind:'org',org_id:'beta',path:[]})})}));
vi.mock('@/api/resources',async(original)=>({...await original<object>(),createTemplate:calls.create,checkTemplate:calls.check,
  driversQuery:{queryKey:['short-template-drivers'],queryFn:()=>Promise.resolve([
    {name:'docker',title:'Docker',hosts:1,descriptor:{spec_schemas:{environment:templateSchema('environment',{}),provider:templateSchema('provider',{})}}},
    {name:'host',title:'Host',hosts:1,descriptor:{spec_schemas:{environment:hostTemplateSchema('environment'),provider:hostTemplateSchema('provider')}}},
  ])},hostsQuery:()=>({queryKey:['short-template-hosts'],queryFn:()=>Promise.resolve([{id:'host',name:'Host',machine:{online:true},offline_since:null,drivers:[{name:'docker',creates:['environment','provider']},{name:'host',creates:['environment','provider']}]}])})}));
beforeEach(async()=>{await setLocale('en');vi.clearAllMocks();calls.create.mockReset().mockResolvedValue({...detailTemplate,id:'made',name:'First web'});useSelectedOrg.setState({orgId:'beta'});});
afterEach(async()=>{useSelectedOrg.setState({orgId:null});await setLocale('en');});

it('creates a full web starter from a name and image without mounting the long settings form or checking it per keystroke',async()=>{
  const user=userEvent.setup();const view=show(<NewTemplateDialog onClose={calls.close}/>);
  const image=await screen.findByRole('textbox',{name:'Image reference'});
  await waitFor(()=>expect(screen.getByRole('combobox',{name:'Belongs to'})).toHaveValue('Beta'));
  expect(view.container.querySelector('[data-template-settings]')).toBeNull();
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  expect(screen.queryByRole('switch',{name:'JSON'})).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox',{name:'Name'}),'First web');
  fireEvent.change(image,{target:{value:'node:24-bookworm'}});
  await user.click(screen.getByRole('button',{name:'Add template'}));
  await waitFor(()=>expect(calls.create).toHaveBeenCalledOnce());
  const created = calls.create.mock.calls[0];
  if (created === undefined) throw new Error('The template needs its one creation request');
  const [body, key] = created;
  expect(body).toMatchObject({name:'First web',home_id:'beta',driver:'docker',access:'only_me',spec:{image:'node:24-bookworm',resources:{cpu_millis:4000,memory_mb:4096}}});
  expect(key).toEqual(expect.any(String));
  expect(calls.create).toHaveBeenCalledOnce();expect(calls.check).not.toHaveBeenCalled();
  expect(calls.close).toHaveBeenCalledOnce();expect(calls.navigate).toHaveBeenCalledWith({to:'/resources/templates/$id',params:{id:'made'}});
});

it('choosing Host fills its account and retains its program, leaving advanced commands for the page',async()=>{
  const user=userEvent.setup();show(<NewTemplateDialog onClose={calls.close}/>);
  await user.click(await screen.findByRole('button',{name:'Start from an example'}));await user.click(screen.getByRole('menuitem',{name:'Host'}));
  expect(screen.getByRole('textbox',{name:'Run as user'})).toHaveValue('agent');
  expect(screen.queryByRole('textbox',{name:'Image reference'})).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox',{name:'Program and arguments · one per line'})).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox',{name:'Name'}),'Host tools');await user.click(screen.getByRole('button',{name:'Add template'}));
  await waitFor(()=>expect(calls.create).toHaveBeenCalledWith(expect.objectContaining({driver:'host',spec:{run_as:'agent',after_up:[{name:'10-ready',command:['/usr/bin/printf','Host tools are ready.']}]}}),expect.any(String)));
});

it('shows a save-time image fault at that field and keeps the name and selected example',async()=>{
  const user=userEvent.setup();calls.create.mockRejectedValueOnce(new ApiError({type:'orch:error:request.invalid',code:'request.invalid',title:'Choose an image',detail:'Choose an image',status:400,request_id:'template-error',errors:[{path:'spec/image',message:'Choose an image reference.'}]},null));
  show(<NewTemplateDialog onClose={calls.close}/>);await screen.findByRole('textbox',{name:'Image reference'});
  await user.type(screen.getByRole('textbox',{name:'Name'}),'First web');await user.click(screen.getByRole('button',{name:'Add template'}));
  const image=screen.getByRole('textbox',{name:'Image reference'});
  await waitFor(()=>expect(image).toHaveAttribute('aria-invalid','true'));
  expect(image).toHaveAccessibleDescription('Choose an image reference.');expect(image).toHaveFocus();
  expect(screen.getAllByText('Choose an image reference.')).toHaveLength(1);
  expect(screen.queryByText('Choose an image',{exact:true})).not.toBeInTheDocument();
  expect(screen.getByRole('textbox',{name:'Name'})).toHaveValue('First web');expect(calls.close).not.toHaveBeenCalled();
});

it('keeps a plugin required input available even when its parent begins as an empty object',()=>{
  const changed=vi.fn();
  const schema={type:'object',required:['connection'],properties:{connection:{type:'object',required:['endpoint'],properties:{endpoint:{type:'string',title:'Service address'},optional:{type:'string',title:'Advanced option'}}},extras:{type:'object'}}};
  show(<QuickTemplateFields schema={schema} spec={{connection:{},extras:{keep:true}}} driver="plugin" onChange={changed} onReady={vi.fn()}/>);
  const endpoint=screen.getByRole('textbox',{name:'Service address'});
  expect(screen.queryByRole('textbox',{name:'Advanced option'})).not.toBeInTheDocument();
  fireEvent.change(endpoint,{target:{value:'https://service.example.test'}});
  expect(changed).toHaveBeenLastCalledWith({connection:{endpoint:'https://service.example.test'},extras:{keep:true}});
});
