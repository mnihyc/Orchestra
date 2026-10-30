/** Enrollment is explicit, scoped to the consented choices, and honest about the installation. */
import { act,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { beforeEach,expect,it,vi } from 'vitest';
import { createHostToken,createMachineToken } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { AddMachineDialog,commandFor } from '@/features/resources/machines/Enrollment';
import { AddHostDialog } from '@/features/resources/hosts/AddHost';
import type { EnrollmentToken,Host,Machine,MachineEnrollmentTokenCreateBody } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';

const info=vi.hoisted(()=>({secure:true,public_url:'https://orch.example',configure:undefined as string|undefined}));
vi.mock('@/features/help/open',()=>({useHelpOpener:()=>vi.fn()}));
vi.mock('@/features/help/Hint',()=>({Hint:()=>null}));
vi.mock('@tanstack/react-router',()=>({useNavigate:()=>vi.fn(),Link:({children}:{children:ReactNode})=><a href="#enrolled">{children}</a>}));
vi.mock('@/features/resources/HomePicker',()=>({useHomeChoices:()=>({choices:[{id:'org',label:'Acme'}]}),HomePicker:({value,onChange}:Pick<HomePickerProps,'value'|'onChange'>)=><select aria-label="Home" value={value} onChange={(event)=>onChange(event.target.value)}><option value="">Personal</option><option value="org">Acme</option></select>}));
vi.mock('@/api/resources',async(load)=>({...await load<typeof ResourcesApi>(),
  createMachineToken:vi.fn(),createHostToken:vi.fn(),
  machineEnrollmentInfoQuery:{queryKey:['enrollment-info-test'],queryFn:()=>Promise.resolve({...info})},
  machinesQuery:{queryKey:['enrollment-machines-test'],queryFn:():Promise<Machine[]>=>Promise.resolve([])},
  hostsQuery:()=>({queryKey:['enrollment-hosts-test'],queryFn:():Promise<Host[]>=>Promise.resolve([])}),
}));
function reply(command:string):EnrollmentToken {return {token:'oenr_component.pin',expires_at:new Date(Date.now()+600_000).toISOString(),commands:{linux:command,macos:command,windows:command}};}
function connector(body:MachineEnrollmentTokenCreateBody):EnrollmentToken {
  return reply(`install.sh --roles '${body.roles?.join(',') ?? ''}' --delegate '${body.delegate?.join(',') ?? ''}'${body.dedicated===true?' --dedicated':''}`);
}
async function showCommand() {const button=screen.getByRole('button',{name:i18n.t('machineInstallation.showCommand')});await waitFor(()=>expect(button).toBeEnabled());await userEvent.click(button);}
/** The field's accessible name includes its visible optional marker in both languages. */
function instanceName(name:string):boolean {
  return name.replace(/\s/gu,'')===(i18n.t('machineInstallation.instance')+i18n.t('ui.field.optional')).replace(/\s/gu,'');
}
beforeEach(()=>{
  vi.clearAllMocks();info.secure=true;info.public_url='https://orch.example';info.configure=undefined;
  vi.mocked(createMachineToken).mockImplementation((_key,body={})=>Promise.resolve(connector(body)));
  vi.mocked(createHostToken).mockImplementation((body)=>Promise.resolve(reply(`install-node.sh${body.delegate_policy===true?' --delegate-policy':''}`)));
});

for(const locale of ['en','zh-CN'] as const) it(`names roles once and mints only on Show command (${locale})`,async()=>{
  await setLocale(locale);
  try {
    const user=userEvent.setup();show(<AddMachineDialog onClose={vi.fn()}/>);
    const help=i18n.t('taskGuides.open',{title:i18n.t('taskGuides.connectMachine')});
    expect(screen.getAllByRole('button',{name:help})).toHaveLength(1);
    await user.click(screen.getByRole('button',{name:help}));
    expect(createMachineToken).not.toHaveBeenCalled();expect(createHostToken).not.toHaveBeenCalled();
    for(const role of ['environment','agent','provider','node'] as const) {
      expect(screen.getAllByRole('checkbox',{name:i18n.t(`machines.add.roleChoices.${role}.name`)})).toHaveLength(1);
      expect(screen.getByText(i18n.t(`machines.add.roleChoices.${role}.description`))).toBeVisible();
    }
    expect(screen.getByText(locale==='en'?'Work in folders on Linux':'在 Linux 的文件夹中工作')).toBeVisible();
    await waitFor(()=>expect(screen.getByRole('button',{name:i18n.t('machineInstallation.showCommand')})).toBeEnabled());
    expect(createMachineToken).not.toHaveBeenCalled();expect(createHostToken).not.toHaveBeenCalled();
    await user.click(screen.getByRole('checkbox',{name:i18n.t('machines.add.roleChoices.provider.name')}));
    expect(createMachineToken).not.toHaveBeenCalled();
    await showCommand();
    expect(await screen.findByText("install.sh --roles 'environment,agent,provider' --delegate ''")).toBeVisible();
    expect(createMachineToken).toHaveBeenCalledTimes(1);
    await user.type(screen.getByRole('textbox',{name:instanceName}),'work');
    expect(screen.getByText(/install.sh.*--data-dir/u)).toHaveTextContent('/instances/work');
    expect(createMachineToken).toHaveBeenCalledTimes(1);
  }finally{await setLocale('en');}
});

for (const locale of ['en', 'zh-CN'] as const) for (const entry of ['machine', 'host'] as const) {
  it(`keeps the ${entry} act named by its entrance when Node changes (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const view = show(entry === 'host' ? <AddHostDialog onClose={vi.fn()} /> : <AddMachineDialog onClose={vi.fn()} />);
    try {
      const name = i18n.t(entry === 'host' ? 'hosts.list.add' : 'machines.list.add');
      const other = i18n.t(entry === 'host' ? 'machines.list.add' : 'hosts.list.add');
      const dialog = screen.getByRole('dialog', { name });
      expect(within(dialog).getAllByRole('heading', { name })).toHaveLength(1);
      const node = within(dialog).getByRole('checkbox', { name: i18n.t('machines.add.roleChoices.node.name') });
      if (entry === 'host') expect(node).toBeChecked(); else expect(node).not.toBeChecked();
      await user.click(node);
      expect(dialog).toHaveAccessibleName(name);
      expect(screen.queryByRole('dialog', { name: other })).not.toBeInTheDocument();
      if (entry === 'host') expect(within(dialog).getByRole('textbox', { name: instanceName })).toBeVisible();
      else expect(within(dialog).queryByRole('textbox', { name: instanceName })).not.toBeInTheDocument();
      expect(createMachineToken).not.toHaveBeenCalled();
      expect(createHostToken).not.toHaveBeenCalled();
      await user.click(node);
      expect(dialog).toHaveAccessibleName(name);
      if (entry === 'host') expect(node).toBeChecked(); else expect(node).not.toBeChecked();
      expect(createMachineToken).not.toHaveBeenCalled();
      expect(createHostToken).not.toHaveBeenCalled();
    } finally { view.unmount(); await setLocale('en'); }
  });
}

it('hands over only selected roles and hides an old command until explicitly issuing new consent',async()=>{
  const user=userEvent.setup();show(<AddMachineDialog onClose={vi.fn()}/>);await showCommand();
  await screen.findByText("install.sh --roles 'environment,agent' --delegate ''");
  await user.click(screen.getByRole('checkbox',{name:'Agent'}));await user.click(screen.getByRole('checkbox',{name:'Provider'}));
  await user.click(within(screen.getByRole('group',{name:'Environment'})).getByRole('switch'));
  await user.click(screen.getByRole('switch',{name:'Full control'}));
  expect(screen.queryByRole('button',{name:'Copy the command'})).not.toBeInTheDocument();
  expect(createMachineToken).toHaveBeenCalledTimes(1);
  await showCommand();
  expect(await screen.findByText("install.sh --roles 'environment,provider' --delegate 'environment' --dedicated")).toBeVisible();
  expect(createMachineToken).toHaveBeenLastCalledWith(expect.any(String),{roles:['environment','provider'],delegate:['environment'],dedicated:true});
});

it('offers Node alone with its root/system paths and mints on the host route after a home is chosen',async()=>{
  const user=userEvent.setup();show(<AddMachineDialog onClose={vi.fn()}/>);
  await user.click(screen.getByRole('checkbox',{name:'Node'}));
  expect(screen.getByText(/\/usr\/local\/bin/u)).toHaveTextContent('/var/lib/orch-node');
  expect(screen.getByText(/\/usr\/local\/bin/u)).toHaveTextContent('/etc/orch');
  expect(screen.queryByRole('textbox',{name:instanceName})).not.toBeInTheDocument();
  for(const role of ['Environment','Agent','Provider']) expect(screen.getByRole('checkbox',{name:role})).not.toBeChecked();
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  expect(createHostToken).not.toHaveBeenCalled();await showCommand();
  expect(await screen.findByText('install-node.sh')).toBeVisible();
  expect(createHostToken).toHaveBeenLastCalledWith({home_id:'org',access:'defaults',delegate_policy:false},expect.any(String));
  expect(createMachineToken).not.toHaveBeenCalled();
});

it('never mints or prints an installer command for a nonloopback HTTP system',async()=>{
  info.secure=false;info.public_url='http://orch.example';
  show(<AddMachineDialog onClose={vi.fn()}/>);
  expect(await screen.findByText('Configure public_url before enrolling a machine')).toBeVisible();
  expect(screen.getByRole('button',{name:'Show command'})).toBeDisabled();
  expect(createMachineToken).not.toHaveBeenCalled();expect(createHostToken).not.toHaveBeenCalled();
  expect(screen.queryByRole('button',{name:'Copy the command'})).not.toBeInTheDocument();
});

it('never displays a late token for a different home and requires another explicit request',async()=>{
  let finish:(token:EnrollmentToken)=>void=()=>{throw new Error('not requested');};
  const pending=new Promise<EnrollmentToken>((resolve)=>{finish=resolve;});
  vi.mocked(createMachineToken).mockImplementationOnce(()=>pending);
  const user=userEvent.setup();show(<AddMachineDialog onClose={vi.fn()}/>);await showCommand();
  await user.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
  await act(async()=>{finish(reply('OLD COMMAND'));await pending;});
  expect(screen.queryByText('OLD COMMAND')).not.toBeInTheDocument();expect(createMachineToken).toHaveBeenCalledTimes(1);
  await showCommand();await waitFor(()=>expect(createMachineToken).toHaveBeenCalledTimes(2));
  expect(createMachineToken).toHaveBeenLastCalledWith(expect.any(String),{home_id:'org',roles:['environment','agent'],delegate:[],dedicated:false});
});

it('writes foreground and named-instance choices without changing token permissions',()=>{
  const token=reply('INSTALL');
  expect(commandFor(token,'linux',{foreground:true,instance:'work',node:false})).toBe('INSTALL --no-service --data-dir "${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector/instances/work" && "$HOME/.local/bin/orch-connector" --data-dir "${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector/instances/work" run');
  expect(commandFor(token,'windows',{foreground:true,instance:'work',node:false})).toContain("-NoService -DataDir (Join-Path $env:LOCALAPPDATA 'Orch/connector/instances/work')");
  expect(commandFor(token,'linux',{foreground:true,instance:'ignored',node:true})).toBe('INSTALL --no-service && /usr/local/bin/orch-node --config /etc/orch/orch.toml run');
});

it('honors loopback HTTP installer eligibility while retaining explicit Show command',async()=>{
  info.secure=true;info.public_url='http://127.0.0.1:3000';
  show(<AddMachineDialog onClose={vi.fn()}/>);
  await waitFor(()=>expect(screen.getByRole('button',{name:'Show command'})).toBeEnabled());
  expect(createMachineToken).not.toHaveBeenCalled();expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  await showCommand();expect(await screen.findByText("install.sh --roles 'environment,agent' --delegate ''")).toBeVisible();
  expect(createMachineToken).toHaveBeenCalledTimes(1);
});


for (const locale of ['en','zh-CN'] as const) for (const entry of ['machine','host'] as const) for (const setting of ['public_url','[[gateway.endpoints]]']) {
  it(`withholds every ${entry} command and token until ${setting} is configured (${locale})`, async()=> {
    await setLocale(locale);
    info.secure=false; info.configure=setting;
    const view=show(entry==='host'?<AddHostDialog onClose={vi.fn()}/>:<AddMachineDialog onClose={vi.fn()}/>);
    try {
      expect(await screen.findByText(i18n.t('machineInstallation.configureAddress',{setting}))).toBeVisible();
      expect(screen.queryByRole('button',{name:i18n.t('machines.add.copy')})).not.toBeInTheDocument();
      expect(screen.getByRole('button',{name:i18n.t('machineInstallation.showCommand')})).toBeDisabled();
      expect(createMachineToken).not.toHaveBeenCalled(); expect(createHostToken).not.toHaveBeenCalled();
    } finally {view.unmount();await setLocale('en');}
  });
}

for(const locale of ['en','zh-CN'] as const) for(const entry of ['machine','host'] as const) {
  it(`explains the expired ${entry} token without exposing its command (${locale})`,async()=>{
    await setLocale(locale);
    const expired={...reply('EXPIRED INSTALLER COMMAND'),expires_at:'2000-01-01T00:00:00Z'};
    if(entry==='host')vi.mocked(createHostToken).mockResolvedValueOnce(expired);
    else vi.mocked(createMachineToken).mockResolvedValueOnce(expired);
    const view=show(entry==='host'?<AddHostDialog onClose={vi.fn()}/>:<AddMachineDialog onClose={vi.fn()}/>);
    try {
      if(entry==='host')await userEvent.selectOptions(screen.getByRole('combobox',{name:'Home'}),'org');
      await showCommand();
      expect(await screen.findByText(i18n.t('machines.add.expired'))).toBeVisible();
      expect(screen.queryByText('EXPIRED INSTALLER COMMAND')).not.toBeInTheDocument();
      expect(screen.queryByRole('button',{name:i18n.t('machines.add.copy')})).not.toBeInTheDocument();
      expect(screen.getByRole('button',{name:i18n.t('machineInstallation.showCommand')})).toBeEnabled();
    }finally{view.unmount();await setLocale('en');}
  });
}
