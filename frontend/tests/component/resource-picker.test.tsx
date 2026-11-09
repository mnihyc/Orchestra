/** One complete picker keeps native choices while grouping, searching and folding their homes. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { useState } from 'react';
import { ResourcePicker, ResourcePickerLookup } from '@/ui/ResourcePicker';
import { ResourcePickerScope } from '@/features/resources/PickerContext';
import type { ResourceGroupMeta } from '@/features/resources/groups';
import type { CardPreview } from '@/ui/DetailCard';
import type { NodeRef } from '@/generated/api-types';
import { client } from '@/api/client';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';
vi.mock('@/features/navigation/places',()=>({PlaceLink:({place,children}:{place:NodeRef;children:React.ReactNode})=><a href={`#${place.id}`}>{children}</a>}));
vi.mock('@/features/resources/machines/MachineLink',()=>({MachineLink:({machine}:{machine:{id:string;name:string}})=><a href={`#${machine.id}`}>{machine.name}</a>}));
vi.mock('@/features/access/AccessDialog',()=>({openAccess:vi.fn()}));
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
 const list=screen.getByRole('listbox',{name:'Template'});
 expect(within(list).getAllByRole('group').map((group)=>group.getAttribute('aria-label'))).toEqual(['Beta home','Alpha home']);
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
  expect(within(screen.getByRole('group', { name: usable })).getByRole('option', { name: 'Usable template' })).toBeEnabled();
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
