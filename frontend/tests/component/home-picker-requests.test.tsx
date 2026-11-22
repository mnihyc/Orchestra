/** Creation loads only the chosen home, reusing live rows instead of fanning out over every org. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import { fetchTreePage } from '@/api/nodes';
import { placesOfTree } from '@/features/settings/PlacePicker';
import { client } from '@/api/client';
import { useAppearance } from '@/lib/appearance';
import type * as Recents from '@/api/pickerRecents';
import type * as Help from '@/help';
import { hintOf } from '@/features/help/hints';
import { HomePicker, type HomePickerProps } from '@/features/resources/HomePicker';
import type { Org } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { choiceCopy } from '@/i18n/choices';
import { NetworkError } from '@/lib/requests';
import { treeWriter, useTreeStore } from '@/realtime/stores/tree';
import { useSelectedOrg } from '@/shell/org';
import { treeNode } from '../doubles/tree-node';
import { ACME, acmeTree, me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const recent=vi.hoisted(()=>({homes:[] as string[],read:vi.fn()}));
vi.mock('@/api/pickerRecents',async(load)=>{const actual=await load<typeof Recents>();return {...actual,
 pickerRecentsQuery:(person:string)=>({...actual.pickerRecentsQuery(person),queryFn:()=>{recent.read(person);return Promise.resolve({count:5,choices:{org:recent.homes}});}}),
 recordPickerChoice:()=>Promise.resolve({count:5,remembered:true}),
};});
const sessionBefore=client.session.getState();const appearanceBefore=useAppearance.getState();
const rootsOf=(length:number)=>Array.from({length},(_,n)=>org(`org-${n}`,`team-${n}`,`Team ${n}`,userRef(me(1,'ada','Ada'))));
let roots=rootsOf(10);
let preloaded=true;
const readRoots=vi.fn(()=>Promise.resolve(roots));
const readNode=vi.fn<(id:string)=>Promise<Org>>();
const treeReply=():Nodes.TreeReply=>({nodes:[],hidden:[],seq:1,since:null,complete:true,next_cursor:null});
// The real field hint/card remain mounted; only the separate guide reader/opener are outside this fixture.
vi.mock('@/features/help/open',async (loadOriginal) => ({ ...await loadOriginal<object>(),useHelpOpener:()=>()=>undefined}));
vi.mock('@/help',async(load)=>({...await load<typeof Help>(),useGuide:()=>undefined}));
vi.mock('@/api/nodes',async(load)=>({...await load<typeof Nodes>(),
  orgsQuery:{queryKey:['home-roots'],staleTime:Infinity,initialData:()=>preloaded?roots:undefined,queryFn:()=>readRoots()},
  nodeQuery:(id:string)=>({queryKey:['home-node',id],queryFn:()=>readNode(id)}),
  fetchTreePage:vi.fn(()=>Promise.resolve(treeReply())),
}));
const selectedBefore=useSelectedOrg.getState();
beforeEach(async()=>{
  recent.homes=[];recent.read.mockClear();
  await setLocale('en');preloaded=true;roots=rootsOf(10);readRoots.mockReset();readRoots.mockImplementation(()=>Promise.resolve(roots));
  readNode.mockReset();readNode.mockRejectedValue(new NetworkError('failed'));
  vi.mocked(fetchTreePage).mockReset();vi.mocked(fetchTreePage).mockImplementation((id,selection)=>{
    if(id!==ACME.org)return Promise.resolve(treeReply());
    const all=acmeTree(),paths=placesOfTree(all,' / '),needle=selection.q?.trim().toLocaleLowerCase()??'';
    const nodes=all.filter((node)=>node.kind==='org'&&paths.find((path)=>path.id===node.id)?.path.toLocaleLowerCase().includes(needle));
    return Promise.resolve({...treeReply(),nodes,ancestors:all.filter((node)=>!nodes.some((row)=>row.id===node.id))});
  });
  useTreeStore.setState({byOrg:new Map()});useSelectedOrg.setState({orgId:'org-0'});
});
afterEach(async()=>{cleanup();client.session.setState(sessionBefore);useAppearance.setState(appearanceBefore);useTreeStore.setState({byOrg:new Map()});useSelectedOrg.setState(selectedBefore);await setLocale('en');});
function Form({initial='org-0',chosen,...props}:Omit<HomePickerProps,'value'|'onChange'>&{initial?:string;chosen?:(id:string)=>void}){
  const [home,setHome]=useState(initial);return <HomePicker {...props} value={home} onChange={(next)=>{setHome(next);chosen?.(next);}}/>;
}

it('keeps a server-selected machine home unpinned until somebody chooses a home', async () => {
  const chosen = vi.fn(); const user = userEvent.setup();
  show(<Form initial="" defaultChoice="Machine home: Guests" chosen={chosen} />);
  const picker = screen.getByRole('combobox', { name: i18n.t('resources.home.label') });
  await waitFor(() => expect(picker).toHaveDisplayValue('Machine home: Guests'));
  expect(chosen).not.toHaveBeenCalled(); expect(fetchTreePage).not.toHaveBeenCalled();
  await user.click(picker); await user.click(screen.getByRole('option', { name: 'Team 7' }));
  expect(chosen).toHaveBeenCalledExactlyOnceWith('org-7');
  await user.click(picker); await user.click(screen.getByRole('option', { name: 'Machine home: Guests' }));
  expect(chosen).toHaveBeenLastCalledWith('');
});

it('loads one selected org among ten, then only the newly chosen one',async()=>{
  show(<Form/>);
  expect(fetchTreePage).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('combobox',{name:i18n.t('resources.home.label')}));
  await waitFor(()=>expect(fetchTreePage).toHaveBeenCalledTimes(1));
  await userEvent.click(screen.getByRole('option',{name:'Team 7'}));
  await userEvent.click(screen.getByRole('combobox',{name:i18n.t('resources.home.label')}));
  await waitFor(()=>expect(fetchTreePage).toHaveBeenCalledTimes(2));
  expect(vi.mocked(fetchTreePage).mock.calls[1]?.[0]).toBe('org-7');
});
it('opens a live home without another tree request',async()=>{
  treeWriter.reset('org-0',[],1);
  show(<Form/>);
  await waitFor(()=>expect(screen.getByRole('combobox',{name:i18n.t('resources.home.label')})).toHaveValue('Team 0'));
  expect(fetchTreePage).not.toHaveBeenCalled();
});

it('uses only org names in a compact creation menu, retaining the same choices and selected-home request',async()=>{
  const user=userEvent.setup();show(<Form compact/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  await waitFor(()=>expect(picker).toHaveValue('Team 0'));
  await user.click(picker);
  const options=screen.getAllByRole('option');expect(options).toHaveLength(10);
  for(const option of options)expect(option).not.toHaveAccessibleDescription();
  await user.click(screen.getByRole('option',{name:'Team 7'}));
  expect(picker).toHaveValue('Team 7');
  await user.click(picker);
  await waitFor(()=>expect(fetchTreePage).toHaveBeenCalledTimes(2));
  expect(vi.mocked(fetchTreePage).mock.calls[1]?.[0]).toBe('org-7');
});

it.each(['en','zh-CN'] as const)('searches all 24 loaded homes and reads only the chosen late home in %s',async(locale)=>{
  await setLocale(locale);roots=rootsOf(24);const chosen=vi.fn();const user=userEvent.setup();show(<Form chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  await waitFor(()=>expect(picker).toHaveValue('Team 0'));
  expect(fetchTreePage).not.toHaveBeenCalled();
  await user.click(picker);await waitFor(()=>expect(fetchTreePage).toHaveBeenCalledTimes(1));expect(screen.getAllByRole('option')).toHaveLength(24);
  expect(picker).toHaveAttribute('placeholder',i18n.t('ui.combobox.search'));
  await user.type(picker,'  tEaM 23  ');
  expect(screen.getAllByRole('option')).toHaveLength(1);
  expect(screen.getByRole('option',{name:'Team 23'})).toHaveAccessibleDescription(choiceCopy(i18n.t,'resourceHome.org',{name:'Team 23'}).description);
  expect(chosen).not.toHaveBeenCalled();expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();
  expect(vi.mocked(fetchTreePage).mock.calls.every(([,selection])=>selection.kind==='org' && typeof selection.q==='string')).toBe(true);
  await user.keyboard('{Enter}');
  expect(chosen).toHaveBeenCalledExactlyOnceWith('org-23');expect(picker).toHaveValue('Team 23');expect(picker).toHaveFocus();
  expect(vi.mocked(fetchTreePage).mock.calls.at(-1)?.[0]).toBe('org-23');
  expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();
});

it.each(['en','zh-CN'] as const)('cancels and reopens a no-match home search without selecting, submitting or reading in %s',async(locale)=>{
  await setLocale(locale);roots=rootsOf(24);const chosen=vi.fn();const submit=vi.fn();const user=userEvent.setup();
  show(<form onSubmit={(event)=>{event.preventDefault();submit();}}><Form chosen={chosen}/><button type="submit">Explicit create</button></form>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  await waitFor(()=>expect(picker).toHaveValue('Team 0'));expect(fetchTreePage).not.toHaveBeenCalled();
  await user.type(picker,'no such home');expect(screen.getByText(i18n.t('ui.combobox.empty'))).toBeVisible();
  expect(screen.queryAllByRole('option')).toHaveLength(0);await user.keyboard('{Enter}');expect(submit).not.toHaveBeenCalled();await user.keyboard('{Escape}');
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();expect(picker).toHaveValue('Team 0');expect(chosen).not.toHaveBeenCalled();
  await user.keyboard('{ArrowDown}');expect(screen.getAllByRole('option')).toHaveLength(24);
  expect(picker).toHaveValue('');await user.keyboard('{Escape}');
  expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();expect(vi.mocked(fetchTreePage).mock.calls.every(([,selection])=>typeof selection.q==='string')).toBe(true);expect(chosen).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:'Explicit create'}));expect(submit).toHaveBeenCalledTimes(1);expect(chosen).not.toHaveBeenCalled();
});

it.each(['en','zh-CN'] as const)('keeps full department paths, distinct place choices, descriptions and the real field help in %s',async(locale)=>{
  await setLocale(locale);roots=[org(ACME.org,'acme','Acme',userRef(me(1,'ada','Ada'))),...rootsOf(24).slice(1)];
  treeWriter.reset(ACME.org,acmeTree(),23);const chosen=vi.fn();const user=userEvent.setup();
  show(<Form initial={ACME.org} chosen={chosen} hint="Existing form guidance" places={[{id:ACME.fixLogin,name:'Current topic'},{id:ACME.org,name:'Duplicate root alias'}]}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await waitFor(()=>expect(picker).toHaveValue('Acme'));
  expect(picker).toHaveAccessibleDescription('Existing form guidance');
  const hint=hintOf('home',i18n.t);const help=screen.getByRole('button',{name:i18n.t('help.hint.label',{term:hint.title})});
  await user.click(help);expect(screen.getByText(hint.what)).toBeVisible();
  if(hint.which!==null)expect(screen.getByText(hint.which)).toBeVisible();await user.keyboard('{Escape}');
  await user.click(picker);await waitFor(()=>expect(screen.getAllByRole('option')).toHaveLength(27));
  expect(screen.getByRole('option',{name:'Current topic'})).not.toHaveAccessibleDescription();
  expect(screen.queryByRole('option',{name:'Duplicate root alias'})).not.toBeInTheDocument();
  const full=['Acme','Research','ML'].join(i18n.t('resources.home.pathJoin'));
  await user.type(picker,'mL');const department=await screen.findByRole('option',{name:full});
  expect(department).toHaveAccessibleDescription(choiceCopy(i18n.t,'resourceHome.department',{name:full}).description);
  await user.click(department);expect(chosen).toHaveBeenCalledExactlyOnceWith(ACME.ml);expect(picker).toHaveValue(full);
  expect(readRoots).not.toHaveBeenCalled();expect(vi.mocked(fetchTreePage).mock.calls.every(([,selection])=>selection.kind==='org')).toBe(true);
});

it('keeps compact late choices name-only without dropping field help',async()=>{
  roots=rootsOf(24);const chosen=vi.fn();const user=userEvent.setup();show(<Form compact chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await waitFor(()=>expect(picker).toHaveValue('Team 0'));
  expect(screen.getByRole('button',{name:i18n.t('help.hint.label',{term:hintOf('home',i18n.t).title})})).toBeInTheDocument();
  await user.click(picker);expect(screen.getAllByRole('option')).toHaveLength(24);
  for(const option of screen.getAllByRole('option'))expect(option).not.toHaveAccessibleDescription();
  await user.keyboard('{End}{Enter}');expect(chosen).toHaveBeenCalledExactlyOnceWith('org-23');expect(picker).toHaveValue('Team 23');
  await user.click(picker);await waitFor(()=>expect(vi.mocked(fetchTreePage).mock.calls.at(-1)?.[0]).toBe('org-23'));
});

it('retains an explicit disabled home and its field error without opening or selecting',async()=>{
  const chosen=vi.fn();const user=userEvent.setup();show(<Form disabled chosen={chosen} hint="Existing hint" error="Choose an allowed home"/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await waitFor(()=>expect(picker).toHaveValue('Team 0'));
  expect(picker).toBeDisabled();expect(picker).toHaveAttribute('aria-invalid','true');expect(picker).toHaveAccessibleDescription('Choose an allowed home');
  expect(screen.queryByText('Existing hint')).not.toBeInTheDocument();await user.click(picker);await user.type(picker,'Team 7');
  expect(screen.queryByRole('listbox')).not.toBeInTheDocument();expect(picker).toHaveValue('Team 0');expect(chosen).not.toHaveBeenCalled();
  expect(fetchTreePage).not.toHaveBeenCalled();expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();
});

it('keeps the pending home disabled until the original root read completes',async()=>{
  preloaded=false;let finish:((value:Org[])=>void)|undefined;readRoots.mockImplementationOnce(()=>new Promise<Org[]>((resolve)=>{finish=resolve;}));
  const chosen=vi.fn();show(<Form initial="" chosen={chosen}/>);const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  expect(picker).toBeDisabled();expect(picker).toHaveAttribute('placeholder',i18n.t('resources.home.loading'));expect(fetchTreePage).not.toHaveBeenCalled();
  await waitFor(()=>expect(readRoots).toHaveBeenCalledTimes(1));
  await act(async()=>{if(finish===undefined)throw new Error('The original roots request did not start');finish(roots);await Promise.resolve();});
  await waitFor(()=>expect(picker).toBeEnabled());expect(picker).toHaveValue('Team 0');expect(chosen).toHaveBeenCalledExactlyOnceWith('org-0');
  expect(fetchTreePage).not.toHaveBeenCalled();expect(readRoots).toHaveBeenCalledTimes(1);
});

it('shows the actual tree failure, keeps the selection, and retries only on the explicit recovery action',async()=>{
  vi.mocked(fetchTreePage).mockRejectedValueOnce(new NetworkError('offline'));const chosen=vi.fn();const user=userEvent.setup();show(<Form chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await user.click(picker);await screen.findByRole('alert');expect(picker).toHaveValue('');
  expect(fetchTreePage).toHaveBeenCalledTimes(1);expect(chosen).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:i18n.t('common.tryAgain')}));
  await waitFor(()=>expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  expect(vi.mocked(fetchTreePage).mock.calls.map(([id])=>id)).toEqual(['org-0','org-0']);expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');expect(picker).toHaveValue('Team 0');expect(chosen).not.toHaveBeenCalled();
});

it.each([false,true])('retains viewed-team defaults and personal preference (preferPersonal=%s)',async(preferPersonal)=>{
  const owner=userRef(me(1,'ada','Ada'));roots=[...rootsOf(24),org('own','ada','Ada',owner,{org_kind:'personal'})];useSelectedOrg.setState({orgId:'org-7'});
  const chosen=vi.fn();show(<Form initial="" preferPersonal={preferPersonal} chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  await waitFor(()=>expect(picker).toHaveValue(preferPersonal?i18n.t('resources.home.ownOrg',{name:'Ada'}):'Team 7'));
  expect(chosen).toHaveBeenCalledExactlyOnceWith(preferPersonal?'own':'org-7');
  expect(fetchTreePage).not.toHaveBeenCalled();
  expect(readRoots).not.toHaveBeenCalled();expect(readNode).not.toHaveBeenCalled();
});

it('resolves an uncached selected department once and reads only its owning tree',async()=>{
  roots=rootsOf(24);readNode.mockResolvedValue(org('remote-department','research','Research',userRef(me(1,'ada','Ada')),
    {org_id:'org-7',parent_id:'org-7',depth:1,path:[{id:'org-7',kind:'org',name:'Team 7',redacted:false},{id:'remote-department',kind:'org',name:'Research',redacted:false}]}));
  const chosen=vi.fn();const user=userEvent.setup();show(<Form initial="remote-department" chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});
  await waitFor(()=>expect(picker).toHaveValue(['Team 7','Research'].join(i18n.t('resources.home.pathJoin'))));
  expect(fetchTreePage).not.toHaveBeenCalled();expect(readNode).toHaveBeenCalledExactlyOnceWith('remote-department');
  await user.type(picker,'Team 23');await user.keyboard('{Escape}');expect(chosen).not.toHaveBeenCalled();expect(readRoots).not.toHaveBeenCalled();expect(readNode).toHaveBeenCalledTimes(1);expect(vi.mocked(fetchTreePage).mock.calls.every(([,selection])=>selection.kind==='org')).toBe(true);
});


it.each(['en','zh-CN'] as const)('keeps an unseen matching department in another root when a root name already matches in %s',async(locale)=>{
  await setLocale(locale);const owner=userRef(me(1,'ada','Ada'));
  roots=[org('lab-root','lab','Lab',owner),org('company-root','company','Company',owner)];
  const lab=treeNode({id:'lab-root',kind:'org',parent_id:null,name:'Lab'});
  const company=treeNode({id:'company-root',kind:'org',parent_id:null,name:'Company'});
  const department=treeNode({id:'unseen-lab',kind:'org',parent_id:company.id,name:'Lab East'});
  vi.mocked(fetchTreePage).mockImplementation((id,selection)=>Promise.resolve({...treeReply(),
    nodes:id===company.id&&selection.q==='Lab'?[department]:id===lab.id?[lab]:[],
    ancestors:id===company.id?[company]:[],
  }));
  const chosen=vi.fn(),user=userEvent.setup();show(<Form compact initial={lab.id} chosen={chosen}/>);
  const picker=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await user.click(picker);
  expect(useTreeStore.getState().byOrg.has(company.id)).toBe(false);
  fireEvent.change(picker,{target:{value:'Lab'}});
  const found=await screen.findByRole('option',{name:'Company / Lab East'});
  expect(screen.getByRole('option',{name:'Lab'})).toBeVisible();
  expect(vi.mocked(fetchTreePage).mock.calls.some(([id,query])=>id===company.id&&query.q==='Lab')).toBe(true);
  await user.click(found);expect(chosen).toHaveBeenCalledWith(department.id);
});


for(const locale of ['en','zh-CN'] as const)for(const theme of ['light','dark'] as const)for(const width of [400,1440]) {
 it(`puts a recently chosen late home first without reading other trees (${locale}/${theme}/${width})`,async()=>{
  await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});client.session.setState({userId:'home-reader'});
  roots=rootsOf(24);recent.homes=['org-23'];const chosen=vi.fn();const user=userEvent.setup();
  show(<div style={{width}}><Form chosen={chosen}/></div>);
  expect(fetchTreePage).not.toHaveBeenCalled();
  const input=screen.getByRole('combobox',{name:i18n.t('resources.home.label')});await user.click(input);
  const group=await screen.findByRole('group',{name:i18n.t('ui.picker.recent')});
  expect(within(group).getAllByRole('option')).toHaveLength(1);expect(within(group).getByRole('option',{name:'Team 23'})).toBeVisible();
  expect(fetchTreePage).toHaveBeenCalledTimes(1);expect(recent.read).toHaveBeenCalledTimes(1);
  await user.keyboard('{Home}{Enter}');expect(chosen).toHaveBeenCalledExactlyOnceWith('org-23');
  expect(fetchTreePage).toHaveBeenCalledTimes(1);
  await user.click(input);
  await waitFor(()=>expect(vi.mocked(fetchTreePage).mock.calls.map(([id])=>id)).toEqual(['org-0','org-23']));
 });
}
