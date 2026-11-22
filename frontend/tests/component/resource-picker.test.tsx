/** One complete picker keeps native choices while grouping, searching and folding their homes. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { ResourcePicker, ResourcePickerLookup } from '@/ui/ResourcePicker';
import { PickerDiagnostics } from '@/features/inspector/PickerDiagnostics';
import { useListChoice } from '@/lib/listState';
import { ResourcePickerScope } from '@/features/resources/PickerContext';
import type { ResourceGroupMeta } from '@/features/resources/groups';
import type { CardPreview } from '@/ui/DetailCard';
import type { NodeRef } from '@/generated/api-types';
import { client } from '@/api/client';
import { pickerRecentsKey } from '@/api/pickerRecents';
import type * as Recents from '@/api/pickerRecents';
import type { MePickerRecents } from '@/generated/api-types';
import { QueryClient } from '@tanstack/react-query';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show, skipOverflowFallback } from './support';

const recentServer=vi.hoisted(()=>({read:vi.fn<(person:string)=>Promise<MePickerRecents>>(),record:vi.fn<typeof Recents.recordPickerChoice>()}));
vi.mock('@/api/pickerRecents',async(load)=>{const actual=await load<typeof Recents>();return {...actual,
 pickerRecentsQuery:(person:string)=>({...actual.pickerRecentsQuery(person),queryFn:()=>recentServer.read(person)}),
 recordPickerChoice:recentServer.record,
};});
beforeEach(()=>{recentServer.read.mockReset().mockResolvedValue({count:5,choices:{}});recentServer.record.mockReset().mockResolvedValue({count:5,remembered:true});});
skipOverflowFallback();
vi.mock('@/features/navigation/places',async (loadOriginal) => ({ ...await loadOriginal<object>(),PlaceLink:({place,children}:{place:NodeRef;children:React.ReactNode})=><a href={`#${place.id}`}>{children}</a>}));
vi.mock('@/features/resources/machines/MachineLink',()=>({MachineLink:({machine}:{machine:{id:string;name:string}})=><a href={`#${machine.id}`}>{machine.name}</a>}));
vi.mock('@/features/access/AccessDialog',async (loadOriginal) => ({ ...await loadOriginal<object>(),openAccess:vi.fn()}));
const appearance=useAppearance.getState();const session=client.session.getState();
afterEach(async()=>{cleanup();localStorage.clear();client.session.setState(session);useAppearance.setState(appearance);await setLocale('en');});
const alpha:NodeRef={id:'alpha-home',name:'Alpha home',kind:'project',redacted:false};
const beta:NodeRef={id:'beta-home',name:'Beta home',kind:'project',redacted:false};
const preview=(id:string,name:string,home:NodeRef):CardPreview=>({label:name,content:()=>null,resource:{kind:'template',id,data:{home}}});
function Picker({screenName='picker-test',selected}:{screenName?:string;selected:(value:string)=>void}){
 const [value,setValue]=useState('alpha');
 return <ResourcePickerScope screen={screenName} currentHomes={[beta.id]} entries={new Map<string,ResourceGroupMeta>([
   ['template:alpha',{home:alpha,home_path:[alpha]}],
   ['template:beta',{home:beta,home_path:[beta]}],
   ['template:blocked',{home:beta,home_path:[beta]}],
 ])}>
  <ResourcePicker pickerKey="template" label="Template" value={value} onValueChange={(next)=>{setValue(next);selected(next);}}
   options={[
    {value:'alpha',name:'Alpha template',preview:preview('alpha','Alpha template',alpha)},
    {value:'beta',name:'Beta template',preview:preview('beta','Beta template',beta)},
    {value:'blocked',name:'Blocked template',description:'Needs permission',disabled:true,preview:preview('blocked','Blocked template',beta)},
   ]}/>
 </ResourcePickerScope>;
}
for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)it(`uses one keyboard row sequence and keeps unavailable rows in ${locale}/${theme}`,async()=>{
 await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});
 const user=userEvent.setup();const selected=vi.fn();show(<Picker selected={selected}/>);
 const input=screen.getByRole('combobox',{name:'Template'});
 await user.click(input);
 // Group actions live in the popup; each named listbox owns only that home's choices.
 const popup=screen.getByRole('dialog',{name:'Template'});
 expect(within(popup).getAllByRole('group').map((group)=>group.getAttribute('aria-label'))).toEqual(['Beta home','Alpha home']);
 expect(within(popup).getAllByRole('listbox').map((list)=>list.getAttribute('aria-label'))).toEqual(['Template · Beta home','Template · Alpha home']);
 const list=within(popup).getByRole('listbox',{name:'Template · Beta home'});
 expect(within(list).getByRole('option',{name:'Blocked template'})).toHaveAttribute('aria-disabled','true');
 await user.keyboard('{Home}{Enter}');
 expect(selected).toHaveBeenLastCalledWith('beta');
 await user.click(input);await user.clear(input);await user.type(input,'alpha');
 expect(screen.getByRole('option',{name:'Alpha template'})).toBeVisible();
 expect(screen.queryByRole('option',{name:'Beta template'})).not.toBeInTheDocument();
 await user.keyboard('{Enter}');expect(selected).toHaveBeenLastCalledWith('alpha');
 await user.click(input);await user.clear(input);await user.keyboard('{Home}{ArrowDown}{Enter}');
 expect(selected).toHaveBeenCalledTimes(2);
 expect(screen.getByText('Needs permission')).toBeVisible();
});
it('remembers collapsed groups per screen, reveals search matches and never selects a heading',async()=>{
 client.session.setState({userId:'picker-reader'});const user=userEvent.setup();const selected=vi.fn();
 let view=show(<Picker selected={selected}/>);let input=screen.getByRole('combobox',{name:'Template'});await user.click(input);
 await user.click(screen.getByRole('button',{name:i18n.t('resourceGroups.collapse',{name:beta.name})}));
 expect(selected).not.toHaveBeenCalled();expect(screen.queryByRole('option',{name:'Beta template'})).not.toBeInTheDocument();
 view.unmount();view=show(<Picker selected={selected}/>);input=screen.getByRole('combobox',{name:'Template'});await user.click(input);
 expect(screen.queryByRole('option',{name:'Beta template'})).not.toBeInTheDocument();
 await user.type(input,'Beta');expect(screen.getByRole('option',{name:'Beta template'})).toBeVisible();
 expect(selected).not.toHaveBeenCalled();view.unmount();
 show(<Picker screenName="another-picker-screen" selected={selected}/>);await user.click(screen.getByRole('combobox',{name:'Template'}));
 expect(screen.getByRole('option',{name:'Beta template'})).toBeVisible();
});
it('does not fill protected metadata from a richer detail preview of another operation',async()=>{
 const user=userEvent.setup();const selected=vi.fn();
 show(<ResourcePickerScope screen="protected" entries={new Map([['template:alpha',{home:{...alpha,name:null,redacted:true},origin_machine:null}]])}>
  <ResourcePicker label="Protected template" value="alpha" onValueChange={selected} options={[{value:'alpha',name:'Safe template name',preview:preview('alpha','Safe template name',{...alpha,name:'SECRET-HOME-NAME'})}]}/>
 </ResourcePickerScope>);
 await user.click(screen.getByRole('combobox',{name:'Protected template'}));
 expect(screen.getByRole('option',{name:'Safe template name'})).toBeVisible();
 expect(screen.queryByText('SECRET-HOME-NAME')).not.toBeInTheDocument();
 expect(selected).not.toHaveBeenCalled();
});

it('keeps origin links and related chips separate from selecting a resource', async () => {
  const user = userEvent.setup(); const selected = vi.fn<(value: string) => void>(); const inspect = vi.fn();
  show(<ResourcePicker pickerKey="related-chips" label="Related template" value="alpha" onValueChange={selected} options={[{
    value:'alpha',name:'Alpha template',resource:{kind:'template',id:'alpha',data:{home:alpha,origin_machine:{id:'origin',name:'Origin machine'}}},
    accessory:<button type="button" onClick={inspect}>Inspect owner</button>,
  }]} />);
  await user.click(screen.getByRole('combobox',{name:'Related template'}));
  const row = screen.getByRole('option',{name:'Alpha template'});
  await user.click(within(row).getByRole('link',{name:'Origin machine'}));
  expect(selected).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:'Inspect owner'}));
  expect(inspect).toHaveBeenCalledTimes(1); expect(selected).not.toHaveBeenCalled();
  await user.click(row); expect(selected).toHaveBeenCalledExactlyOnceWith('alpha');
});

it('keeps an explicit protected compound entry ahead of richer cached full content', async () => {
  const user=userEvent.setup();
  show(<ResourcePickerScope screen="protected-full" entries={new Map([['template:alpha',{home:null,origin_machine:null}]])}>
    <ResourcePicker pickerKey="full" label="Full template" value="alpha" onValueChange={vi.fn()} options={[{
      value:'alpha',name:'Safe template',resource:{kind:'template',id:'alpha',data:{home:{...alpha,name:'OTHER-OPERATION-HOME'}}},
    }]} />
  </ResourcePickerScope>);
  await user.click(screen.getByRole('combobox',{name:'Full template'}));
  expect(screen.queryByText('OTHER-OPERATION-HOME')).not.toBeInTheDocument();
  expect(screen.getByRole('group',{name:i18n.t('resourceDetails.system')})).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) for (const lookup of [false, true]) it(`keeps native access groups inside a foldable home (${locale}, lookup=${lookup})`, async () => {
  await setLocale(locale);
  const user = userEvent.setup(); const selected = vi.fn<(value: string) => void>();
  client.session.setState({ userId: 'access-picker-reader' });
  const usable = i18n.t('start.where.canUse'); const askable = i18n.t('start.where.askAccess');
  const rows = [
    { value: 'use', name: 'Usable template', group: usable, resource: { kind: 'template' as const, id: 'use', data: { home: alpha } } },
    { value: 'ask', name: 'Requestable template', group: askable, resource: { kind: 'template' as const, id: 'ask', data: { home: alpha } } },
    { value: 'blocked', name: 'Blocked template', disabled: true, resource: { kind: 'template' as const, id: 'blocked', data: { home: alpha } } },
  ];
  show(lookup
    ? <ResourcePickerLookup label="Access template" query="" onQueryChange={() => undefined} onSelect={(option) => selected(option.value)} groups={rows.map((row) => ({ id: row.value, label: row.group ?? '', options: [{ ...row, label: row.name, text: row.name }] }))} />
    : <ResourcePicker label="Access template" value="use" onValueChange={selected} options={rows} />);
  await user.click(screen.getByRole('combobox', { name: 'Access template' }));
  expect(within(await screen.findByRole('group', { name: usable })).getByRole('option', { name: 'Usable template' })).toBeEnabled();
  const popup = within(screen.getByRole('dialog', { name: 'Access template' }));
  expect(popup.queryByText(i18n.t('clientFacts.device.collapsedGroups'))).toBeNull();
  expect(popup.queryByText(i18n.t('clientFacts.device.save'))).toBeNull();
  const requestGroup = screen.getByRole('group', { name: askable });
  expect(within(requestGroup).getByRole('option', { name: 'Requestable template' })).toBeEnabled();
  expect(within(requestGroup).queryByRole('option', { name: 'Blocked template' })).toBeNull();
  expect(screen.getByRole('option', { name: 'Blocked template' })).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('button', { name: i18n.t('resourceGroups.collapse', { name: alpha.name }) }));
  expect(screen.queryAllByRole('option')).toHaveLength(0);
  await user.click(screen.getByRole('button', { name: i18n.t('resourceGroups.expand', { name: alpha.name }) }));
  await user.click(within(screen.getByRole('group', { name: askable })).getByRole('option', { name: 'Requestable template' }));
  expect(selected).toHaveBeenCalledExactlyOnceWith('ask');
});


for (const kind of ['agent','provider','environment','machine','host','user','document','topic']) {
  it(`offers this person's recent ${kind} first, only once, without losing search or keyboard selection`, async () => {
    client.session.setState({userId:'recent-reader'});
    const cache=new QueryClient({defaultOptions:{queries:{retry:false,staleTime:Infinity}}});
    const reply={count:5,choices:{[kind]:['late','missing','blocked']}};
    recentServer.read.mockResolvedValue(reply);cache.setQueryData(pickerRecentsKey('recent-reader'),reply);
    const user=userEvent.setup();const selected=vi.fn();
    show(<ResourcePicker pickerKey="recent-kind" label="Resource" value="first" onValueChange={selected}
      options={[
        {value:'first',name:'First entry',resource:{kind,id:'first'}},
        {value:'middle',name:'Middle entry',resource:{kind,id:'middle'}},
        {value:'late',name:'Recent entry',resource:{kind,id:'late'}},
        {value:'blocked',name:'Unavailable entry',disabled:true,description:'Needs permission',resource:{kind,id:'blocked'}},
      ]}/>,cache);
    const input=screen.getByRole('combobox',{name:'Resource'});await user.click(input);
    const recent=await screen.findByRole('group',{name:i18n.t('ui.picker.recent')});
    expect(within(recent).getAllByRole('option').map((row)=>row.textContent)).toEqual(['Recent entry']);
    expect(screen.getAllByRole('option',{name:'Recent entry'})).toHaveLength(1);
    expect(screen.getByRole('option',{name:'Unavailable entry'})).toHaveAttribute('aria-disabled','true');
    expect(recentServer.read).toHaveBeenCalledTimes(1);
    await user.keyboard('{Home}{Enter}');expect(selected).toHaveBeenLastCalledWith('late');
    expect(recentServer.record).toHaveBeenCalledWith({kind,key:'late'});
    await user.click(input);await user.type(input,'m');
    expect(screen.getByRole('option',{name:'Middle entry'})).toBeVisible();
    expect(screen.queryByRole('option',{name:'Recent entry'})).not.toBeInTheDocument();
    await user.keyboard('{Enter}');expect(selected).toHaveBeenLastCalledWith('middle');
    await user.click(input);await user.clear(input);await user.type(input,'no such entry');
    expect(screen.getByText(i18n.t('resourceGroups.noMatches'))).toBeVisible();
    await user.keyboard('{Escape}');expect(input).toHaveFocus();
  });
}

it('does not show another person’s recent choices after a reader changes',async()=>{
  client.session.setState({userId:'other-recent-reader'});
  const cache=new QueryClient({defaultOptions:{queries:{retry:false,staleTime:Infinity}}});
  cache.setQueryData(pickerRecentsKey('recent-reader'),{count:5,choices:{agent:['late']}});
  cache.setQueryData(pickerRecentsKey('other-recent-reader'),{count:5,choices:{}});
  show(<ResourcePicker label="Agent" value="first" onValueChange={vi.fn()} options={[
    {value:'first',name:'First agent',resource:{kind:'agent',id:'first'}},
    {value:'late',name:'Last agent',resource:{kind:'agent',id:'late'}},
  ]}/>,cache);
  await userEvent.click(screen.getByRole('combobox',{name:'Agent'}));
  expect(screen.queryByRole('group',{name:i18n.t('ui.picker.recent')})).not.toBeInTheDocument();
  expect(screen.getAllByRole('option').map((row)=>row.textContent)).toEqual(['First agent','Last agent']);
});


it('remembers the chosen program rather than another program from its same template preview',async()=>{
 client.session.setState({userId:'program-reader'});
 recentServer.read.mockResolvedValue({count:5,choices:{template:[JSON.stringify(['source-template','first-program'])]}});
 const user=userEvent.setup();const chosen=vi.fn();
 show(<ResourcePicker label="Program" value="second-program" onValueChange={chosen} options={[
  {value:'second-program',name:'Second program',resource:{kind:'template',id:'source-template'},recent:{kind:'template',key:JSON.stringify(['source-template','second-program'])}},
  {value:'first-program',name:'First program',resource:{kind:'template',id:'source-template'},recent:{kind:'template',key:JSON.stringify(['source-template','first-program'])}},
 ]}/>);
 await user.click(screen.getByRole('combobox',{name:'Program'}));
 const recent=await screen.findByRole('group',{name:i18n.t('ui.picker.recent')});
 expect(within(recent).getAllByRole('option')).toHaveLength(1);expect(within(recent).getByRole('option',{name:'First program'})).toBeVisible();
 await user.keyboard('{Home}{Enter}');expect(chosen).toHaveBeenCalledExactlyOnceWith('first-program');
 expect(recentServer.record).toHaveBeenCalledWith({kind:'template',key:JSON.stringify(['source-template','first-program'])});
});


it('keeps the inherited choice first while Recent leads the actual resource choices',async()=>{
 client.session.setState({userId:'inherited-reader'});
 recentServer.read.mockResolvedValue({count:5,choices:{provider:['late']}});
 show(<ResourcePicker label="Provider" value="inherit" onValueChange={vi.fn()} options={[
  {value:'inherit',name:'As Acme decides'},
  {value:'first',name:'First provider',resource:{kind:'provider',id:'first'}},
  {value:'late',name:'Recent provider',resource:{kind:'provider',id:'late'}},
 ]}/>);
 await userEvent.click(screen.getByRole('combobox',{name:'Provider'}));
 await screen.findByRole('group',{name:i18n.t('ui.picker.recent')});
 expect(screen.getAllByRole('option').map((row)=>row.textContent)).toEqual(['As Acme decides','Recent provider','First provider']);
 expect(recentServer.record).not.toHaveBeenCalled();
});


it('keeps a native username or address match whose visible name differs from the search',async()=>{
 const selected=vi.fn();
 show(<ResourcePickerLookup label="Person" groupHomes={false} filterLocal={false} query="ada.handle" onQueryChange={vi.fn()} onSelect={selected}
  groups={[{id:'people',label:'',options:[{value:'ada',label:'Ada Lovelace',text:'Ada Lovelace',description:'@ada.handle',resource:{kind:'user',id:'ada'}}]}]}/>);
 await userEvent.click(screen.getByRole('combobox',{name:'Person'}));
 await userEvent.click(screen.getByRole('option',{name:'Ada Lovelace'}));
 expect(selected).toHaveBeenCalledWith(expect.objectContaining({value:'ada'}));
});


it('shares a resource recent across screens whose local option values differ',async()=>{
 client.session.setState({userId:'workspace-reader'});recentServer.read.mockResolvedValue({count:5,choices:{environment:['workspace']}});
 const selected=vi.fn();show(<ResourcePicker label="Workspace" value="other" onValueChange={selected} options={[
  {value:'other',name:'Other workspace',resource:{kind:'environment',id:'other'}},
  {value:'use:workspace',name:'Recent workspace',resource:{kind:'environment',id:'workspace'}},
 ]}/>);
 const user=userEvent.setup();await user.click(screen.getByRole('combobox',{name:'Workspace'}));
 const recent=await screen.findByRole('group',{name:i18n.t('ui.picker.recent')});
 expect(within(recent).getByRole('option',{name:'Recent workspace'})).toBeVisible();
 await user.keyboard('{Home}{Enter}');expect(selected).toHaveBeenCalledExactlyOnceWith('use:workspace');
 expect(recentServer.record).toHaveBeenCalledWith({kind:'environment',key:'workspace'});
});


it('keeps device receipts out of the choices and reports the last fold in Diagnostics only to its reader', async () => {
  function SignInProgram() {
    const [value, choose] = useListChoice('provider-sign-in-test', 'program', 'one', 'device');
    return <ResourcePicker label="Sign-in program" value={value} onValueChange={choose} options={[
      { value: 'one', name: 'Program one', resource: { kind: 'template', id: 'one', data: { home: alpha } } },
    ]} />;
  }
  client.session.setState({ userId: 'program-picker-reader' });
  const user = userEvent.setup();
  const picker = show(<SignInProgram />);
  await user.click(screen.getByRole('combobox', { name: 'Sign-in program' }));
  const popup = await screen.findByRole('dialog', { name: 'Sign-in program' });
  const fold = await within(popup).findByRole('button', { name: i18n.t('resourceGroups.collapse', { name: alpha.name }) });
  expect(within(popup).getByRole('option', { name: 'Program one' })).toBeVisible();
  expect(popup.querySelector('[data-fact-disclosure-body]')).toBeNull();
  await user.click(fold);
  expect(within(popup).queryByRole('option', { name: 'Program one' })).toBeNull();
  expect(popup.querySelector('[data-fact-disclosure-body]')).toBeNull();
  await user.keyboard('{Escape}');
  picker.unmount();

  show(<PickerDiagnostics />);
  await user.click(screen.getByText(i18n.t('clientFacts.device.lastPicker')));
  const facts = within(screen.getByLabelText(i18n.t('clientFacts.device.collapsedGroups')));
  expect(facts.getByText(i18n.t('clientFacts.device.loadedCollapsed', { count: 1, number: '1' }))).toBeVisible();
  expect(facts.getByText(i18n.t('clientFacts.device.savedCollapsed', { count: 1, number: '1' }))).toBeVisible();
  expect(facts.getByText(i18n.t('clientFacts.device.saves.saved'))).toBeVisible();
  act(() => { client.session.setState({ userId: 'another-program-reader' }); });
  expect(screen.queryByText(i18n.t('clientFacts.device.lastPicker'))).toBeNull();
  act(() => { client.session.setState({ userId: 'program-picker-reader' }); });
  expect(screen.queryByText(i18n.t('clientFacts.device.lastPicker'))).toBeNull();
});

it.each(['template', 'provider'] as const)('names each home of a %s picker in a plain group header, with none of the lists’ acts', async (kind) => {
 const user = userEvent.setup();
 const option = (id: string, name: string, home: NodeRef) => ({ value: id, name, preview: { label: name, content: () => null, resource: { kind, id, data: { home } } } });
 show(<ResourcePickerScope screen="picker-plain" currentHomes={[beta.id]} entries={new Map<string, ResourceGroupMeta>([
   [`${kind}:alpha`, { home: alpha, home_path: [alpha] }], [`${kind}:beta`, { home: beta, home_path: [beta] }]])}>
  <ResourcePicker pickerKey={kind} label="Choice" value="alpha" onValueChange={vi.fn()} options={[option('alpha', 'Alpha choice', alpha), option('beta', 'Beta choice', beta)]} />
 </ResourcePickerScope>);
 await user.click(screen.getByRole('combobox', { name: 'Choice' }));
 const popup = screen.getByRole('dialog', { name: 'Choice' });
 for (const home of [alpha, beta]) {
  const header = popup.querySelector<HTMLElement>(`[data-resource-group="home:${home.id}"]`);
  if (header === null) throw new Error(`${home.name ?? ''} has no group header`);
  // The header names the home and folds its group: making a pool and the home's sharing are acts of the lists, and its name opens nothing.
  expect(header).toHaveTextContent(home.name ?? '');
  expect(within(header).getAllByRole('button').map((button) => button.getAttribute('aria-label'))).toEqual([i18n.t('resourceGroups.collapse', { name: home.name })]);
  expect(within(header).queryByRole('link')).toBeNull();
  expect(header.querySelector('[data-resource-sharing]')).toBeNull();
  expect(within(header).queryByText(i18n.t('resourceGroups.sharingPolicy'))).toBeNull();
  expect(within(header).queryByText(i18n.t('pools.make'))).toBeNull();
 }
});
