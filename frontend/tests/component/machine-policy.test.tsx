/** Policy rows are compact; one native invocation, not a key's role, identifies the enforcing program. */
import { fireEvent,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach,expect,it,vi } from 'vitest';
import { ApiError } from '@/api/client';
import { MachineLimits,MachinePolicyTab,policyCommand } from '@/features/resources/machines/Policy';
import type { MachinePolicy,MachinePolicyKey } from '@/generated/api-types';
import { i18n,setLocale } from '@/i18n';
import { show } from './support';
const {read,write,openGuide}=vi.hoisted(()=>({read:vi.fn<()=>Promise<MachinePolicy>>(),write:vi.fn<()=>Promise<MachinePolicy>>(),openGuide:vi.fn()}));
vi.mock('@/features/help/open',async (loadOriginal) => ({ ...await loadOriginal<object>(),useHelpOpener:()=>openGuide}));
vi.mock('@/api/resources',async (loadOriginal) => ({ ...await loadOriginal<object>(),
  machinePolicyQuery:()=>({queryKey:['machine-policy','machine'],queryFn:read}),
  machineQuery:()=>({queryKey:['machine','machine']}),updateMachinePolicy:write,
}));
function entry(key:string,sections:MachinePolicyKey['definition']['sections'],value:unknown):MachinePolicyKey {
  return {key,value,bounds:'environment',delegated:false,writable:false,definition:{key,title:'Future setting',meaning:'A meaning reported by the machine.',sections,managed:true,cautious:false,delegable:true}};
}
function policy(command="'/home/ada/.local/bin/orch-connector' --data-dir '/data/work instance' policy"):MachinePolicy {
  return {machine:{id:'machine',name:'Laptop',online:true,placement:'connected',platform_os:'linux'},command,reported_at:null,keys:[
    entry('environment.terminal',['environment'],false),entry('environment.roots',['environment','node'],['/work']),entry('future.setting',['provider'],true),
  ]};
}
beforeEach(()=>{vi.clearAllMocks();read.mockResolvedValue(policy());});
for (const locale of ['en','zh-CN'] as const) it(`uses the native applicable keys and puts their actual authority in the existing help opening (${locale})`, async () => {
  await setLocale(locale);
  try {
    const current = { ...policy(), limits: { unit_context: true, applicable_keys: [
      { key: 'environment.terminal', governing_resource: 'environment' as const, authority_source: 'physical_operator' as const },
    ] } };
    read.mockResolvedValue(current);
    const view = show(<MachineLimits machineId="machine" bounds="environment" id="limits" title="Machine limits" lead="" />);
    await screen.findByRole('region', { name: 'Machine limits' });
    expect(view.container.querySelector('[data-setting="environment.roots"]')).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('machines.policy.keys.environment_terminal.title') }) }));
    const body = document.querySelector('[data-fact-disclosure-body]');
    expect(body).not.toBeNull();
    expect(body).toHaveTextContent(locale === 'en' ? 'Applies to this unit' : '适用于此单元');
    expect(body).toHaveTextContent(locale === 'en' ? 'Physical machine operator' : '物理机器操作员');
    expect(screen.queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
    expect(read).toHaveBeenCalledOnce(); expect(write).not.toHaveBeenCalled();
  } finally { await setLocale('en'); }
});

it('keeps an unfinished policy choice while its long collection is filtered away and restored', async () => {
  const current = policy();
  current.keys = [{ ...entry('environment.terminal',['environment'],false), delegated:true, writable:true },
    ...Array.from({length:29},(_,index)=>entry(`future.${index}`,['environment'],false))];
  read.mockResolvedValue(current);
  const view = show(<MachineLimits machineId="machine" bounds="environment" id="limits" title="Machine limits" lead="" />);
  const search = await screen.findByRole('searchbox');
  await userEvent.click(screen.getByRole('button',{name:i18n.t('machines.policy.editTitle',{key:i18n.t('machines.policy.keys.environment_terminal.title')})}));
  await userEvent.click(screen.getByRole('switch'));
  expect(screen.getByRole('switch')).toBeChecked();
  fireEvent.change(search,{target:{value:'no matching key'}});
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  fireEvent.change(search,{target:{value:'environment.terminal'}});
  expect(screen.getByRole('switch')).toBeChecked();
  expect(view.container.querySelector('[data-policy-keys]')).toHaveClass('max-h-(--list-max-height)');
  expect(write).not.toHaveBeenCalled();
});

it('finds a reported directory value without expanding an unbounded policy list', async () => {
  const current = policy();
  current.keys = [entry('environment.roots',['environment','node'],Array.from({length:30},(_,index)=>`/workspace/${index}`))];
  read.mockResolvedValue(current);
  show(<MachinePolicyTab machine={{id:'machine'}} />);
  await userEvent.click(await screen.findByRole('button',{name:i18n.t('machines.policy.viewTitle',{key:i18n.t('machines.policy.keys.environment_roots.title')})}));
  const search = await screen.findByRole('searchbox',{name:i18n.t('machines.policy.findValue')});
  fireEvent.change(search,{target:{value:'/workspace/29'}});
  expect(screen.getByText('/workspace/29',{exact:true})).toBeVisible();
  expect(screen.queryByText('/workspace/0',{exact:true})).not.toBeInTheDocument();
  expect(write).not.toHaveBeenCalled();
});
for(const locale of ['en','zh-CN'] as const)it(`names the physical operator in a unit's existing locally kept limit source (${locale})`,async()=>{
  await setLocale(locale);
  try {
    const view=show(<MachineLimits machineId="machine" bounds="environment" id="limits" title="Machine limits" lead=""/>);
    await screen.findByRole('region',{name:'Machine limits'});
    const row=view.container.querySelector('[data-setting="environment.terminal"]');
    expect(row).toHaveTextContent(locale==='en'?'Physical machine operator':'物理机器操作员');
    expect(within(row as HTMLElement).queryByRole('button',{name:i18n.t('machines.policy.editTitle',{key:i18n.t('machines.policy.keys.environment_terminal.title')})})).not.toBeInTheDocument();
  }finally{await setLocale('en');}
});
for(const locale of ['en','zh-CN'] as const)it(`keeps descriptions and exact kept commands behind Help, with shared keys shown once (${locale})`,async()=>{
  await setLocale(locale);
  try {
    const user=userEvent.setup();const view=show(<MachinePolicyTab machine={{id:'machine'}}/>);
    const environment=await screen.findByRole('region',{name:i18n.t('machines.policy.sections.environment.name')});
    expect(within(environment).queryByText(i18n.t('machines.policy.sections.environment.description'))).not.toBeInTheDocument();
    expect(view.container.querySelectorAll('[data-setting="environment.roots"]')).toHaveLength(1);
    const shells=i18n.t('machines.policy.keys.environment_terminal.title');
    expect(within(environment).getAllByText(shells)).toHaveLength(1);
    await user.click(within(environment).getByRole('button',{name:i18n.t('help.hint.label',{term:shells})}));
    expect(screen.getByText(i18n.t('machines.policy.defaults.managed'))).toBeVisible();
    expect(screen.getByText(i18n.t('machines.policy.defaults.cautious'))).toBeVisible();
    expect(screen.getByText("'/home/ada/.local/bin/orch-connector' --data-dir '/data/work instance' policy set environment.terminal false")).toBeVisible();
  }finally{await setLocale('en');}
});
it('uses the physical node command for its role handover and keeps explicit local values in force',async()=>{
  read.mockResolvedValue(policy("'/usr/local/bin/orch-node' --data-dir '/var/lib/orch-node' --config '/etc/orch/orch.toml' policy"));
  const user=userEvent.setup();show(<MachinePolicyTab machine={{id:'machine'}}/>);
  const region=await screen.findByRole('region',{name:'Environment'});await user.click(within(region).getByRole('button',{name:'Full Access…'}));
  const dialog=screen.getByRole('dialog',{name:'Full Access for Environment'});
  expect(within(dialog).getByText("'/usr/local/bin/orch-node' --data-dir '/var/lib/orch-node' --config '/etc/orch/orch.toml' policy delegate 'environment.*'")).toBeVisible();
  expect(within(dialog).getByText(/Values explicitly kept on the machine stay in force/u)).toBeVisible();
  expect(within(dialog).queryByText(/delegate 'provider/u)).not.toBeInTheDocument();
});
it('edits in the row and applies the returned policy without another read',async()=>{
  const initial=policy();initial.keys[0]={...entry('environment.terminal',['environment'],false),delegated:true,writable:true};
  read.mockResolvedValue(initial);write.mockResolvedValue({...initial,keys:[{...initial.keys[0],value:true}]});
  const user=userEvent.setup();show(<MachinePolicyTab machine={{id:'machine'}}/>);
  await user.click(await screen.findByRole('button',{name:i18n.t('machines.policy.editTitle',{key:i18n.t('machines.policy.keys.environment_terminal.title')})}));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await user.click(screen.getByRole('switch'));await user.click(screen.getByRole('button',{name:i18n.t('machines.policy.save')}));
  await screen.findByText(i18n.t('machines.policy.changed',{key:i18n.t('machines.policy.keys.environment_terminal.title')}));
  expect(write).toHaveBeenCalledWith('machine',{'environment.terminal':true});expect(read).toHaveBeenCalledTimes(1);
});
it('quotes Windows JSON values as PowerShell arguments without changing the program prefix',()=>{
  expect(policyCommand('environment.roots',["C:\\Ada's work"],"& 'C:\\Orch\\orch-connector.exe' --data-dir 'C:\\Orch\\one' policy",true)).toBe("& 'C:\\Orch\\orch-connector.exe' --data-dir 'C:\\Orch\\one' policy set environment.roots '[\"C:\\\\Ada''s work\"]'");
});

for (const locale of ['en', 'zh-CN'] as const) it(`the MCP import help flips the current value using the reported program (${locale})`, async () => {
  await setLocale(locale);
  try {
    const current = policy("'/opt/orch/bin/orch-connector' --data-dir '/srv/orch instance' policy");
    current.keys = [entry('mcp.import', ['environment'], false)];
    read.mockResolvedValue(current);
    show(<MachinePolicyTab machine={{ id: 'machine' }} />);
    const term = i18n.t('machines.policy.keys.mcp_import.title');
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('help.hint.label', { term }) }));
    expect(screen.getByText("'/opt/orch/bin/orch-connector' --data-dir '/srv/orch instance' policy set mcp.import true")).toBeVisible();
  } finally { await setLocale('en'); }
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the driver draft and reports each schema fault beside it (${locale})`, async () => {
  await setLocale(locale);
  try {
    const initial = policy();
    initial.keys = [{ ...entry('drivers', ['node'], { allowed: ['cluster'], configurations: { cluster: { namespace: 7, storage_class: '' } } }), delegated: true, writable: true }];
    read.mockResolvedValue(initial);
    write.mockRejectedValue(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', status: 400,
      title: 'Invalid', detail: 'Invalid configuration', request_id: 'fixture', errors: [
        { path: 'drivers.configurations.cluster/namespace', message: 'the value has the wrong type' },
        { path: 'drivers.configurations.cluster/storage_class', message: 'the text has a length outside the allowed range' },
        { path: 'drivers.configurations.cluster/other', message: 'toString' },
      ] }, null));
    const user = userEvent.setup();
    show(<MachinePolicyTab machine={{ id: 'machine' }} />);
    await user.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.drivers.title') }) }));
    const editor = screen.getByRole('textbox', { name: i18n.t('machines.policy.value') });
    expect(editor).toBeVisible();
    const draft = (editor as HTMLTextAreaElement).value;
    await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
    await screen.findByText(/drivers\.configurations\.cluster\/namespace/);
    expect(editor).toHaveValue(draft);
    expect(editor).toHaveAttribute('aria-invalid', 'true');
    expect(editor).toHaveAccessibleDescription(new RegExp(i18n.t('machines.policy.driverFaults.type')));
    expect(editor).toHaveAccessibleDescription(new RegExp(i18n.t('machines.policy.driverFaults.length')));
    expect(editor).toHaveAccessibleDescription(new RegExp(i18n.t('machines.policy.driverFaults.rule')));
    await user.type(editor, ' ');
    expect(editor).not.toHaveAttribute('aria-invalid', 'true');
  } finally { await setLocale('en'); }
});

it('keeps an unrelated save failure in the existing error alert', async () => {
  const initial = policy();
  initial.keys = [{ ...entry('drivers', ['node'], { allowed: ['cluster'] }), delegated: true, writable: true }];
  read.mockResolvedValue(initial);
  write.mockRejectedValue(new ApiError({ type: 'orch:error:internal', code: 'internal', status: 500,
    title: 'Unavailable', detail: 'The change could not be confirmed', request_id: 'fixture' }, null));
  const user = userEvent.setup();
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  await user.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.drivers.title') }) }));
  expect(screen.getByRole('textbox', { name: i18n.t('machines.policy.value') })).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('textbox', { name: i18n.t('machines.policy.value') })).not.toHaveAttribute('aria-invalid', 'true');
});

it('opens the driver settings guide from the existing help card', async () => {
  const initial = policy();
  initial.keys = [{ ...entry('drivers', ['node'], { allowed: ['cluster'] }), delegated: true, writable: true }];
  read.mockResolvedValue(initial);
  const user = userEvent.setup();
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  const hint = await screen.findByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('machines.policy.keys.drivers.title') }) });
  expect(hint).toHaveAttribute('data-help-article', 'connect-a-machine');
  expect(hint).toHaveAttribute('data-help-heading', 'change-driver-settings');
  await user.click(hint);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.more') }));
  expect(openGuide).toHaveBeenCalledWith('connect-a-machine', 'change-driver-settings');
});

it('shows a mixed failure in the field and the existing alert', async () => {
  const initial = policy();
  initial.keys = [{ ...entry('drivers', ['node'], { allowed: ['cluster'], configurations: { cluster: { namespace: 7 } } }), delegated: true, writable: true }];
  read.mockResolvedValue(initial);
  write.mockRejectedValue(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', status: 400,
    title: 'Invalid', detail: 'Invalid configuration', request_id: 'fixture', errors: [
      { path: 'drivers.configurations.cluster/namespace', message: 'the value has the wrong type' },
      { path: 'drivers.allowed', message: 'the driver cannot be used' },
    ] }, null));
  const user = userEvent.setup();
  show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  await user.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.drivers.title') }) }));
  const editor = screen.getByRole('textbox', { name: i18n.t('machines.policy.value') });
  await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(editor).toHaveAttribute('aria-invalid', 'true');
  expect(editor).toHaveAccessibleDescription(new RegExp(i18n.t('machines.policy.driverFaults.type')));
});

it('finds every complete policy declaration while its open driver draft survives filtering and paging', async () => {
  const initial = policy();
  initial.keys = [{ ...entry('drivers', ['node'], { allowed: ['docker'] }), delegated: true, writable: true },
    ...Array.from({ length: 1000 }, (_, index) => ({ ...entry(`future.${index}`, ['node'], true), definition: { ...entry(`future.${index}`, ['node'], true).definition, title: `Future ${index}` } }))];
  read.mockResolvedValue(initial); write.mockResolvedValue(initial);
  const user = userEvent.setup(); show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  await user.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: i18n.t('machines.policy.keys.drivers.title') }) }));
  const editor = screen.getByRole('textbox', { name: i18n.t('machines.policy.value') });
  await user.clear(editor); await user.paste('{"allowed":["podman"]}');
  const search = screen.getByRole('searchbox', { name: i18n.t('ui.collection.find', { name: i18n.t('machines.policy.sections.node.name') }) });
  await user.click(search); await user.paste('future.999'); expect(await screen.findByText('Future 999')).toBeVisible();
  expect(editor).toHaveValue('{"allowed":["podman"]}');
  await user.clear(search); await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  expect(editor).toHaveValue('{"allowed":["podman"]}');
  await user.click(screen.getByRole('button', { name: i18n.t('machines.policy.save') }));
  await waitFor(() => expect(write).toHaveBeenCalledWith('machine', { drivers: { allowed: ['podman'] } }));
  expect(read).toHaveBeenCalledOnce();
});

it('confirms only the system build currently reported by the machine and can revoke it', async () => {
  const build = { os: 'windows', arch: 'x86_64', executable: 'a'.repeat(64) };
  const key = { ...entry('exec.platform', ['machine'], { state: 'off' }), delegated: true, writable: true };
  const initial: MachinePolicy = { ...policy(), keys: [key], platform_review: { build, approved: false, approval: null } };
  read.mockResolvedValue(initial);
  write.mockResolvedValue({ ...initial, platform_review: { build, approved: true, approval: null } });
  const view = show(<MachinePolicyTab machine={{ id: 'machine' }} />);
  try {
    await userEvent.click(await screen.findByRole('button', { name: 'Confirm this system' }));
    expect(write).toHaveBeenCalledWith('machine', { 'exec.platform': { state: 'approved', build } });
    await userEvent.click(await screen.findByRole('button', { name: 'Revoke confirmation' }));
    expect(write).toHaveBeenCalledWith('machine', { 'exec.platform': { state: 'off' } });
  } finally { view.unmount(); }
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps the machine file bound a named choice independent of agent sandbox (${locale})`, async () => {
  await setLocale(locale);
  try {
    read.mockResolvedValue({ ...policy(), keys: [{ ...entry('environment.files_reach', ['environment'], 'roots'), writable: true, delegated: true }, entry('agent.max_sandbox', ['agent'], 'workspace')] });
    show(<MachinePolicyTab machine={{ id: 'machine' }} />);
    const name = i18n.t('machines.policy.keys.environment_files_reach.title');
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('machines.policy.editTitle', { key: name }) }));
    const choice = screen.getByRole('combobox');
    expect(choice).toHaveTextContent(i18n.t('fileReach.roots'));
    await userEvent.click(choice);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('fileReach.machine') }));
    expect(choice).toHaveTextContent(i18n.t('fileReach.machine'));
    expect(screen.getByText(i18n.t('machines.policy.sandbox.workspace'))).toBeVisible();
  } finally { await setLocale('en'); }
});
