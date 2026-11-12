/** Grouping keeps native rows, their selection and their current device state. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Box } from 'lucide-react';
import { afterEach, expect, it, vi } from 'vitest';
import { ResourceListFrame } from '@/features/resources/ResourceList';
import { groupResourceRows, resourceGroupMeta, type ResourceGroupMeta } from '@/features/resources/groups';
import type { NodeRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { client } from '@/api/client';
import { show } from './support';

vi.mock('@/features/resources/trail', () => ({useResourceHeading:vi.fn(),rememberList:vi.fn()}));
vi.mock('@/features/resources/ResourceNav', () => ({ResourceNav:() => null}));
vi.mock('@/features/navigation/places', () => ({PlaceLink:({children,place}:{children:React.ReactNode;place:NodeRef}) => <a href={`#${place.id}`}>{children}</a>}));
const sharing = vi.hoisted(() => vi.fn());
vi.mock('@/features/access/AccessDialog', () => ({openAccess:sharing}));
const appearance = useAppearance.getState();
const session = client.session.getState();
afterEach(async () => {cleanup();localStorage.clear();client.session.setState(session);useAppearance.setState(appearance);sharing.mockReset();await setLocale('en');});
const home = (id:string,name:string):NodeRef => ({id,name,kind:'project',redacted:false});
const first=home('home-a','Alpha'), second=home('home-b','Beta');
const rows=[{id:'one',name:'First resource',owner:null,home:first},{id:'two',name:'Second resource',owner:null,home:second}];
function List() {
  return <ResourceListFrame kind="templates" search={{}} title="Templates" icon={Box} lead={null} label="Resource inventory"
    load={{isPending:false,isError:false,isRefetching:false,error:null,refetch:vi.fn()}} all={rows} rows={rows}
    columns={[{id:'name',header:'Name',cell:(row)=><a href={`#${row.id}`}>{row.name}</a>}]}
    rowKey={(row)=>row.id} text="" onText={vi.fn()} searchLabel="Find resources" searchPlaceholder=""
    empty={{icon:<Box/>,title:'Empty'}} noMatch="No matches" />;
}

it('groups by identity, keeps unavailable rows and puts the current home first', () => {
  const a={id:'a'},b={id:'b'},c={id:'c'};
  const metadata=new Map<string,ResourceGroupMeta>([
    ['a',{home:first,home_path:[home('root-a','Team A'),first]}],
    ['b',{home:second,home_path:[home('root-b','Team B'),second],availability:[{machine_id:'other',available:false,reason:'Unavailable here'}]}],
    ['c',{home:first}],
  ]);
  const groups=groupResourceRows([a,b,c],'home',(row)=>metadata.get(row.id)??{},i18n.t,'en',[second.id]);
  expect(groups.map((group)=>group.home?.id)).toEqual([second.id,first.id]);
  expect(groups[0]?.rows).toEqual([b]);
  expect(groups[1]?.rows).toEqual([a,c]);
  const withheld=resourceGroupMeta({home:null,origin_machine:null},{home:first,origin_machine:{id:'hidden'}});
  expect(withheld.home).toBeNull();expect(withheld.origin_machine).toBeNull();
  expect(groupResourceRows([a,b,c],'none',()=>({}),i18n.t,'en')[0]?.rows).toEqual([a,b,c]);
});

for(const locale of ['en','zh-CN'] as const) for(const theme of ['light','dark'] as const) it(`keeps Home grouping, folding and its sharing entrance in ${locale}/${theme}`,async()=>{
  await setLocale(locale);useAppearance.setState({theme,resolvedTheme:theme});
  client.session.setState({userId:'grouping-reader'});
  const user=userEvent.setup();
  let view=show(<List/>);
  const table=screen.getByRole('table',{name:'Resource inventory'});
  expect(within(table).getByText('First resource')).toBeVisible();
  const heading=table.querySelector('[data-resource-group="home:home-a"]');
  expect(heading).not.toBeNull();
  await user.click(within(heading as HTMLElement).getByRole('button',{name:i18n.t('resourceGroups.sharingOf',{name:first.name})}));
  expect(sharing).toHaveBeenCalledWith({kind:'project',id:first.id,name:first.name},{tab:'defaults',share:false});
  await user.click(screen.getByRole('button',{name:i18n.t('resourceGroups.collapse',{name:first.name})}));
  expect(screen.queryByText('First resource')).not.toBeInTheDocument();
  expect(screen.getByText('Second resource')).toBeVisible();
  view.unmount();view=show(<List/>);
  expect(screen.queryByText('First resource')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:i18n.t('resourceGroups.expand',{name:first.name})}));
  expect(screen.getByText('First resource')).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('resourceGroups.label')}));
  await user.click(screen.getByRole('combobox',{name:i18n.t('resourceGroups.label')}));
  await user.click(screen.getByRole('option',{name:i18n.t('resourceGroups.none')}));
  expect(screen.queryByRole('button',{name:i18n.t('resourceGroups.collapse',{name:first.name})})).not.toBeInTheDocument();
  expect(screen.getByText('First resource')).toBeVisible();
  view.unmount();
});
