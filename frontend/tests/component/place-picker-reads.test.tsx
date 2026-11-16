/** A closed place picker reads nothing; native names supply one scoped page without walking org trees. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Search from '@/api/search';
import type * as Nodes from '@/api/nodes';
import { PlacePicker } from '@/features/settings/PlacePicker';
import type { NameHit, NameSearchPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { treeWriter } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';
import { show } from './support';

const reads=vi.hoisted(()=>({tree:vi.fn(),names:vi.fn<(search:Search.NameSearch,cursor:string|null)=>Promise<NameSearchPage>>()}));
vi.mock('@/api/nodes',async(load)=>({...await load<typeof Nodes>(),fetchTree:reads.tree}));
vi.mock('@/api/search',async(load)=>{const actual=await load<typeof Search>();return {...actual,
 pickerNamesQuery:(search:Search.NameSearch)=>({...actual.pickerNamesQuery(search),queryFn:({pageParam}:{pageParam:string|null})=>reads.names(search,pageParam)}),
};});
const root={id:ACME.org,kind:'org' as const,name:'Acme',redacted:false};
const hit=(index:number):NameHit=>({home_path:[],id:`place-${index}`,kind:'topic',name:`Destination ${index}`,address:null,archived:false,avatar:null,color:null,custom_color:null,owner:null,recent:false,path:[root,{id:`place-${index}`,kind:'topic',name:`Destination ${index}`,redacted:false}],tag:null,thread_attention:null,username:null});
beforeEach(()=>{reads.names.mockReset().mockImplementation((search,cursor)=>{
 const found=Array.from({length:51},(_,index)=>hit(index)).filter((row)=>search.q===''||row.name.toLowerCase().includes(search.q.toLowerCase()));
 const from=Number(cursor??0);return Promise.resolve({items:found.slice(from,from+20),partial:from+20<found.length,next_cursor:from+20<found.length?String(from+20):null});
});});
afterEach(async()=>{cleanup();treeWriter.clear();reads.tree.mockClear();await setLocale('en');});

for(const locale of ['en','zh-CN'] as const) {
 it(`reads one native page only on opening and refreshes recent order on reopening in ${locale}`,async()=>{
  await setLocale(locale);const user=userEvent.setup();const changed=vi.fn();show(<PlacePicker label="Place" value={null} onChange={changed} includeThreads/>);
  expect(reads.names).not.toHaveBeenCalled();expect(reads.tree).not.toHaveBeenCalled();
  const picker=screen.getByRole('combobox',{name:'Place'});await user.click(picker);
  expect(await screen.findByRole('option',{name:'Acme / Destination 0'})).toBeVisible();expect(reads.names).toHaveBeenCalledTimes(1);
  await user.keyboard('{Escape}');await user.click(picker);await waitFor(()=>expect(reads.names).toHaveBeenCalledTimes(2));
  await user.click(await screen.findByRole('option',{name:'Acme / Destination 0'}));expect(changed).toHaveBeenCalledWith(expect.objectContaining({id:'place-0',kind:'topic',rootOrgId:ACME.org}));
  expect(reads.tree).not.toHaveBeenCalled();expect(reads.names).toHaveBeenCalledTimes(2);
 });
 it(`keeps root and kind restrictions on native search and pages while a richer tree is loaded in ${locale}`,async()=>{
  await setLocale(locale);treeWriter.reset(ACME.org,acmeTree(),1);const user=userEvent.setup();const changed=vi.fn();
  show(<PlacePicker label="Place" value={null} onChange={changed} rootOrgId={ACME.org} only="topic"/>);
  const input=screen.getByRole('combobox',{name:'Place'});await user.click(input);await screen.findByRole('option',{name:'Acme / Destination 0'});
  expect(screen.getAllByRole('option')).toHaveLength(20);expect(screen.queryByRole('option',{name:'Acme / Destination 50'})).toBeNull();
  await user.click(screen.getByRole('button',{name:i18n.t('resourceDetails.layout.next')}));
  expect(await screen.findByRole('option',{name:'Acme / Destination 20'})).toBeVisible();expect(screen.queryByRole('option',{name:'Acme / Destination 0'})).toBeNull();
  await user.click(input);await user.type(input,'50');expect(await screen.findByRole('option',{name:'Acme / Destination 50'})).toBeVisible();
  expect(reads.names.mock.calls.every(([search])=>search.orgId===ACME.org&&search.kinds.join(',')==='topic')).toBe(true);
  expect(changed).not.toHaveBeenCalled();expect(reads.tree).not.toHaveBeenCalled();
  expect(screen.queryByRole('option',{name:'Acme / Research / Web / Fix login'})).toBeNull();
 });
}
