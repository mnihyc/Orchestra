/** Nested library views stay compact, widen empty results and restore the permitted state for each resource page. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Box } from 'lucide-react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { ResourceList } from '@/features/resources/ResourceList';
import type { ResourceKind } from '@/features/resources/trail';
import type { EverythingFilter } from '@/api/everything';
import { show } from './support';

const original=client.session.getState();
const calls:EverythingFilter[]=[];
let admin=false;
let orgReady=true;
vi.mock('@/api/auth',()=>({meQuery:{queryKey:['library-reader'],initialData:{id:'library-reader'},queryFn:()=>Promise.resolve({id:'library-reader'})}}));
vi.mock('@/api/everything',()=>({everythingQuery:(_reader:string,_parent:string,filter:EverythingFilter)=>({queryKey:['library-inventory',filter],initialPageParam:null,
 queryFn:()=>{calls.push(filter);return Promise.resolve({items:[],next_cursor:null});},getNextPageParam:()=>null})}));
vi.mock('@/features/resources/HomePicker',()=>({useViewedOrgState:()=>({org:orgReady?{id:'team',name:'Workshop',access:{administers:admin}}:null,load:{isSuccess:orgReady,isPending:!orgReady,isError:false}})}));
vi.mock('@/features/resources/trail',()=>({rememberList:vi.fn(),useResourceHeading:vi.fn()}));
vi.mock('@/features/resources/ResourceNav',()=>({ResourceNav:()=>null}));
beforeEach(()=>{localStorage.clear();client.session.setState({userId:'library-reader'});calls.length=0;admin=false;orgReady=true;});
afterEach(()=>{cleanup();localStorage.clear();client.session.setState(original);});

function Library({kind='skills'}:{kind?:ResourceKind}) {return <ResourceList kind={kind} search={{}} title="Library" icon={Box} lead={null} label="Library" readRow={()=>null}
 columns={[]} rowKey={(row)=>row.id} text="" onText={vi.fn()} searchLabel="Find resources" searchPlaceholder=""
 empty={{icon:<Box/>,title:'Empty'}} noMatch="No matches"/>;}

it('starts at use, puts Show before search, and widens an empty view with one act',async()=>{
 const user=userEvent.setup();show(<Library/>);
 expect(await screen.findByText('Nothing you can use here')).toBeVisible();
 const view=screen.getByRole('combobox',{name:'Show'});expect(view).toHaveTextContent('I can use');
 const bar=view.closest('[data-filter-bar]');expect(bar?.firstElementChild).toContainElement(view);
 await user.click(screen.getByRole('button',{name:'Show what you can view'}));
 expect(view).toHaveTextContent('I can view');expect(await screen.findByText('Nothing you can view here')).toBeVisible();
 expect(calls.at(-1)?.resource?.view).toBe('viewable');
});

it('offers the nested labels in order, separates the admin view and saves this list independently',async()=>{
 const user=userEvent.setup();admin=true;const opened=show(<Library/>);
 const view=screen.getByRole('combobox',{name:'Show'});await user.click(view);
 const list=screen.getByRole('listbox');expect(within(list).getAllByRole('option').map((option)=>option.textContent)).toEqual(['Mine','I manage','I can use','I can view','All in Workshop']);
 expect(within(list).getByRole('separator',{hidden:true})).toBeVisible();
 await user.click(within(list).getByRole('option',{name:'I manage'}));expect(view).toHaveTextContent('I manage');opened.unmount();
 const other=show(<Library kind="templates"/>);expect(screen.getByRole('combobox',{name:'Show'})).toHaveTextContent('I can use');other.unmount();
 show(<Library/>);expect(screen.getByRole('combobox',{name:'Show'})).toHaveTextContent('I manage');
});

it('leaves out Use for environments and replaces an unavailable saved admin view with the default',async()=>{
 const user=userEvent.setup();const environment=show(<Library kind="environments"/>);
 const view=screen.getByRole('combobox',{name:'Show'});expect(view).toHaveTextContent('I can view');await user.click(view);
 expect(screen.queryByRole('option',{name:'I can use'})).not.toBeInTheDocument();await user.keyboard('{Escape}');environment.unmount();
 admin=true;const first=show(<Library/>);await user.click(screen.getByRole('combobox',{name:'Show'}));await user.click(screen.getByRole('option',{name:'All in Workshop'}));first.unmount();
 admin=false;show(<Library/>);expect(screen.getByRole('combobox',{name:'Show'})).toHaveTextContent('I can use');
});

it('keeps a saved All view while the org’s authority metadata is still loading',async()=>{
 const user=userEvent.setup();admin=true;const first=show(<Library/>);await user.click(screen.getByRole('combobox',{name:'Show'}));await user.click(screen.getByRole('option',{name:'All in Workshop'}));first.unmount();
 orgReady=false;const loading=show(<Library/>);const before=calls.length;
 expect(screen.getByRole('combobox',{name:'Show'})).toHaveTextContent('I can use');expect(calls).toHaveLength(before);
 orgReady=true;loading.rerender(<Library/>);
 expect(screen.getByRole('combobox',{name:'Show'})).toHaveTextContent('All in Workshop');
});
