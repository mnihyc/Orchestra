/** Location facts distinguish unconfigured, concealed and offline data on the mount and machine pages. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MountsHere } from '@/features/resources/mounts/Here';
import { Locations } from '@/features/resources/mounts/Locations';
import { MountWhere } from '@/features/resources/mounts/Where';
import type { Machine, MachineMounts, Mount, MountLocationSetBody } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const { read, readMachines, add }=vi.hoisted(()=>({
  read:vi.fn<(machine:string)=>Promise<MachineMounts>>(),
  readMachines:vi.fn<()=>Promise<Machine[]>>(),
  add:vi.fn<(mount:string,machine:string,body:MountLocationSetBody)=>Promise<Mount>>(),
}));
vi.mock('@/api/resources',()=>({machinesQuery:{queryKey:['machines'],queryFn:readMachines}}));
vi.mock('@/api/resource-tools',()=>({machineMountsQuery:(id:string)=>({queryKey:['mounts','machine',id],queryFn:()=>read(id)}),removeMountLocation:vi.fn(),setMountLocation:add}));
vi.mock('@tanstack/react-router',()=>({useRouter:()=>({state:{location:{pathname:'/resources/mounts/notes'}},history:{push:vi.fn()}}),Link:({to,params,children}:{to:string;params:{id:string};children:ReactNode})=><a href={to.replace('$id',params.id)}>{children}</a>}));
vi.mock('@/ui/UserChip',()=>({UserChip:({user}:{user:{display_name:string}})=><span>{user.display_name}</span>}));
const mount=():Mount=>({id:'notes',name:'shared-notes',address:'team.shared-notes',description:'A host directory, read-only',org_id:'org',home:null,owner:null,
  target:'/mnt/notes',read_only:true,enabled:true,default_fate:'as_is',locations:[],locations_hidden:0,kept:[],revision:1,
  access:{permissions:['mount.view'],administers:false,participating:true,admin_only:[],through:[]}});
const machine=(id:string,name:string,permissions:string[]):Machine=>({id,name,placement:'connected',online:true,platform_os:'linux',owner:null,
  platform_arch:'x86_64',connector_version:'1.0.0',protocol_version:1,roles:['environment'],status:'active',last_seen_at:null,last_ip:null,
  cert_not_after:'2027-01-01T00:00:00Z',facts:{environment_descriptor:{data_storage:{made_by_orch:'directory',existing_directory:true,reason:null}}},providers:[],agents:[],environments:[],environment_sandboxes:[],
  access:{permissions,administers:false,participating:true,admin_only:[],through:[]}});
beforeEach(()=>{vi.clearAllMocks();read.mockResolvedValue({locations:[],available:[]});readMachines.mockResolvedValue([]);add.mockResolvedValue(mount());});
afterEach(async()=>{await setLocale('en');});

for(const language of ['en','zh-CN'] as const) {
  it(`explains no configured location despite a host-directory description, scoped to world-box (${language})`,async()=>{
    await setLocale(language);const row=mount();
    show(<><p>{row.description}</p><Locations mount={row}/><MountsHere machine="world-box" name="world-box"/></>);
    expect(screen.getByText(row.description)).toBeVisible();
    expect(screen.getByText(language==='en'?/No location is configured for this mount yet/:/此挂载尚未配置位置/)).toBeVisible();
    expect(await screen.findByText(language==='en'?/No mount locations are visible on world-box/:/world-box 上没有可见的挂载位置/)).toBeVisible();
    expect(screen.getByText(language==='en'?/locations on other machines are shown on its page/:/在其他机器上的位置可在其页面查看/)).toBeVisible();
  });

  it(`explains a hidden location without inventing a host or claiming none exists (${language})`,async()=>{
    await setLocale(language);const row=mount();row.locations_hidden=1;
    show(<Locations mount={row}/>);
    expect(screen.getByText(language==='en'?/One location is hidden/:/有 1 个位置被隐藏/)).toBeVisible();
    expect(screen.queryByText(language==='en'?/No location is configured/:/尚未配置位置/)).not.toBeInTheDocument();
    expect(screen.queryByRole('link',{name:'docker-1'})).not.toBeInTheDocument();
  });

  it(`keeps an offline host named on both the mount and Where column (${language})`,async()=>{
    await setLocale(language);const row=mount();
    row.locations=[{machine:{id:'node',name:'Docker node',online:false,platform_os:'linux',placement:'managed'},host:{id:'docker-1',name:'docker-1',address:'team.docker-1'},
      kind:'path',source:null,storage:null,data_kind:null,can_remove:false,created_by:{id:'person',username:'person',display_name:'Owner',kind:'human',avatar:{text:'O',color:1}},created_at:'2026-10-15T10:00:00Z'}];
    show(<><Locations mount={row}/><section aria-label={i18n.t('resourceDetails.mount.where')}><MountWhere locations={row.locations} hidden={row.locations_hidden}/></section></>);
    const links=screen.getAllByRole('link',{name:'docker-1'});expect(links).toHaveLength(2);
    for(const link of links) expect(link).toHaveAttribute('href','/resources/hosts/docker-1');
    expect(within(screen.getByRole('region',{name:i18n.t('resourceDetails.mount.locations')})).getByText(i18n.t('resourceDetails.mount.locationOfflineHelp'))).toBeVisible();
    expect(within(screen.getByRole('region',{name:i18n.t('resourceDetails.mount.where')})).getByRole('link',{name:'docker-1'}).closest('li')).toHaveTextContent(i18n.t('resourceDetails.mount.locationOffline'));
    expect(screen.queryByText(language==='en'?/No location is configured/:/尚未配置位置/)).not.toBeInTheDocument();
    expect(screen.queryByText(language==='en'?/location is hidden/:/位置被隐藏/)).not.toBeInTheDocument();
  });

  it(`offers a managed directory machine's declared data without requiring personal ownership (${language})`,async()=>{
    await setLocale(language);const user=userEvent.setup();const row=mount();row.access.permissions.push('mount.use');
    readMachines.mockResolvedValue([machine('shared','Shared directory machine',['machine.manage']),machine('read-only','Use-only machine',['machine.view','host.use'])]);
    read.mockResolvedValue({locations:[],available:[{mount:{id:row.id,name:row.name,address:row.address},path:true,default_storage:true,storages:[]}]});
    show(<Locations mount={row}/>);
    await user.click(screen.getByRole('button',{name:i18n.t('resourceDetails.mount.addLocation')}));
    const dialog=screen.getByRole('dialog',{name:i18n.t('resourceDetails.mount.addLocation')});
    await user.click(within(dialog).getByRole('combobox',{name:i18n.t('resourceDetails.mount.machine')}));
    const option=await screen.findByRole('option',{name:/Shared directory machine/});
    expect(screen.queryByRole('option',{name:/Use-only machine/})).not.toBeInTheDocument();
    await user.click(option);
    const kind=await within(dialog).findByRole('combobox',{name:i18n.t('resourceDetails.mount.locationKind')});
    await user.click(kind);
    await user.click(screen.getByRole('option',{name:i18n.t('choiceCatalog.mountLocation.volume.name')}));
    await user.click(within(dialog).getByRole('button',{name:i18n.t('resources.edit.save')}));
    await waitFor(()=>expect(add).toHaveBeenCalledWith('notes','shared',{kind:'volume'}));
    expect(read).toHaveBeenCalledWith('shared');
  });
}

it('offers an existing directory without inventing an owned-data capability',async()=>{
  const user=userEvent.setup();const row=mount();row.access.permissions.push('mount.use');
  readMachines.mockResolvedValue([machine('shared','Shared directory machine',['machine.manage'])]);
  read.mockResolvedValue({locations:[],available:[{mount:{id:row.id,name:row.name,address:row.address},path:true,default_storage:false,storages:[]}]});
  show(<Locations mount={row}/>);
  await user.click(screen.getByRole('button',{name:i18n.t('resourceDetails.mount.addLocation')}));
  const dialog=screen.getByRole('dialog',{name:i18n.t('resourceDetails.mount.addLocation')});
  await user.click(within(dialog).getByRole('combobox',{name:i18n.t('resourceDetails.mount.machine')}));
  await user.click(await screen.findByRole('option',{name:/Shared directory machine/}));
  await user.click(await within(dialog).findByRole('combobox',{name:i18n.t('resourceDetails.mount.locationKind')}));
  expect(screen.queryByRole('option',{name:i18n.t('choiceCatalog.mountLocation.volume.name')})).not.toBeInTheDocument();
  await user.click(screen.getByRole('option',{name:i18n.t('choiceCatalog.mountLocation.path.name')}));
  await user.type(within(dialog).getByRole('textbox',{name:i18n.t('resourceDetails.mount.source')}),'/srv/notes');
  await user.click(within(dialog).getByRole('button',{name:i18n.t('resources.edit.save')}));
  await waitFor(()=>expect(add).toHaveBeenCalledWith('notes','shared',{kind:'path',source:'/srv/notes'}));
});

it('treats an unreported location count as unknown, rather than zero',()=>{
  show(<MountWhere locations={[]}/>);
  expect(screen.getByText('Location details have not been reported.')).toBeVisible();
  expect(screen.queryByText(/No location is configured/)).not.toBeInTheDocument();
});
