/** Both pools show applicable origins and timing, with owner-guarded literal commands and recoverable failures. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type * as SettingsApi from '@/api/settings';
import { putSetting, targetSettingsKey, type SettingsTarget } from '@/api/settings';
import { UnitCommandFailure, UnitConfiguration, type UnitConfigurationProps } from '@/features/resources/UnitConfiguration';
import { localizedSetting } from '@/features/settings/settingText';
import type { ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n,setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({
  rows: [] as ResolvedSetting[], spec: {},
  read: vi.fn<() => Promise<ResolvedSetting[]>>(),
  job: { state: 'failed', error: { code: 'after_up_failed', message: 'after-start command 20-cache exited with 7: cache is locked' } },
}));
vi.mock('@/api/settings', async (original) => {
  const actual=await original<typeof SettingsApi>();
  return {...actual,putSetting:vi.fn(),registryQuery:{queryKey:['settings-registry'],queryFn:()=>Promise.resolve(SETTINGS_REGISTRY.filter((definition)=>server.rows.some((row)=>row.key===definition.key))),staleTime:Infinity},
    targetSettingsQuery:(target:SettingsTarget)=>({queryKey:actual.targetSettingsKey(target),queryFn:server.read})};
});
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  templateQuery: (id: string) => ({ queryKey: ['unit-template', id], queryFn: () => Promise.resolve({ spec: server.spec }) }),
  providerQuery:(id:string)=>({queryKey:['provider',id],queryFn:()=>Promise.resolve(planProvider({id}))}),
}));
vi.mock('@/api/environments',async(original)=>({...await original<object>(),environmentQuery:(id:string)=>({queryKey:['environment',id],queryFn:()=>Promise.resolve({...detailEnvironment,id})})}));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), jobQuery: (id: string) => ({ queryKey: ['unit-job', id], queryFn: () => Promise.resolve(server.job) }) }));
vi.mock('@/features/help/SettingHint', () => ({ SettingHelpLink: () => null }));

const setting = (key: string, value: unknown, patch: Partial<ResolvedSetting> = {}): ResolvedSetting => ({ key, value, own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [], ...patch });
const place = { name: 'Team unit', owner: { display_name: 'Ada' }, access: { permissions: ['environment.manage', 'provider.manage'] }, kind: null };

beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks(); server.rows = []; server.spec = {};
  server.read.mockImplementation(() => Promise.resolve(server.rows));
  vi.mocked(putSetting).mockImplementation((_, key, write) => {
    const previous = server.rows.find((row) => row.key === key);
    const next = setting(key, write.value, { own: { value: write.value, revision: (previous?.own?.revision ?? 0) + 1, locked: false } });
    server.rows = server.rows.map((row) => row.key === key ? next : row);
    return Promise.resolve(next);
  });
});

/** Identify the actual served setting, then assert its familiar label rather than a retired raw-key column. */
function visibleSetting(list:HTMLElement,key:string):HTMLElement {
  const row=list.querySelector<HTMLElement>(`[data-setting="${key}"]`);
  if(row===null)throw new Error(`No visible setting row for ${key}`);
  const definition=SETTINGS_REGISTRY.find((definition)=>definition.key===key);
  if(definition===undefined)throw new Error(`The shipped registry is missing ${key}`);
  expect(within(row).getByText(localizedSetting(definition,i18n).title,{exact:true,ignore:'.sr-only, .sr-only *'})).toBeVisible();
  expect(row).not.toHaveTextContent(key);
  return row;
}

it.each(['environment','provider'] as const)('offers named storage settings for a directory %s without a placement gate',async(pool)=>{
  server.rows=[setting(pool==='environment'?'env.storage':'runtime.storage',{home:'fast'})];
  draw({pool,placement:'connected',lifecycle:'attach'});
  const storage=await screen.findByRole('region',{name:'Storage'});
  await waitFor(()=>expect(storage).toHaveTextContent('fast'));
  expect(within(storage).getAllByText(/Existing data keeps its recorded location/)).toHaveLength(1);
  expect(within(storage).getByText(/Existing data keeps its recorded location/)).toBeVisible();
  expect(within(storage).getAllByText('Storage',{exact:true,ignore:'.sr-only, .sr-only *'})).toHaveLength(1);
  expect(storage.querySelectorAll('[data-setting]')).toHaveLength(1);
});

function draw(props: Partial<UnitConfigurationProps> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const full: UnitConfigurationProps = { pool: 'environment', id: 'unit', placement: 'managed', lifecycle: 'unit', place, ...props };
  const result = show(<QueryClientProvider client={client}><UnitConfiguration {...full} /></QueryClientProvider>);
  return { client, full, ...result };
}

it.each(['environment','provider'] as const)('shows the declared resource setting and each actual controller for a directory %s',async(pool)=>{
  const key=pool==='environment'?'env.resources':'runtime.resources';
  server.rows=[setting(key,{memory_mb:64})];
  draw({pool,lifecycle:'attach',placement:'connected',resourceSupport:{cpu:'enforced',memory:'reported',pids:'none',disk:'reported'}});
  const values=await screen.findByRole('list',{name:'Unit configuration'});
  const declared=visibleSetting(values,key);
  expect(declared).toHaveTextContent('64');
  expect(within(declared).getByText('Next attachment')).toBeVisible();
  const facts=screen.getByRole('region',{name:'Resource controls'});
  expect(within(facts).getByText('CPU').closest('div')).toHaveTextContent('Enforced');
  expect(within(facts).getByText('Memory').closest('div')).toHaveTextContent('Declared');
  expect(within(facts).getByText('Processes').closest('div')).toHaveTextContent('Unavailable');
  expect(within(facts).getByText('Disk').closest('div')).toHaveTextContent('Declared');
});

it('shows each applicable value with its actual ancestor and when it takes effect', async () => {
  server.rows = [setting('env.variables', { PATH_HINT: 'literal' }, { source: { scope: 'org', node: { id: 'org', kind: 'org', name: 'Acme', redacted: false } } }),
    setting('env.storage', { workspace: 'fast' }), setting('env.after_up', {}), setting('runtime.renew_days', 7)];
  draw();
  const values = await screen.findByRole('list', { name: 'Unit configuration' });
  const variables = visibleSetting(values,'env.variables');
  expect(variables).toHaveTextContent('literal');
  expect(variables).toHaveTextContent('Acme');
  expect(variables).toHaveTextContent('Next attachment');
  const storage=await screen.findByRole('region',{name:'Storage'});
  await waitFor(()=>expect(storage).toHaveTextContent('fast'));
  const commands=screen.getByRole('region',{name:'Commands after start'});
  expect(within(commands).getByText(i18n.t('unitConfiguration.commandTiming'))).toBeVisible();
  expect(values.querySelector('[data-setting="runtime.renew_days"]')).toBeNull();
  expect(values.querySelector('[data-setting="env.after_up"]')).toBeNull();
});

it('runs the template layer first in the displayed list and never makes its fields editable here', async () => {
  server.rows = [setting('env.after_up', { '10-inherited': { command: ['inherited'], timeout_secs: 12 } })];
  server.spec = { after_up: [{ name: '90-template', command: ['/opt/template', 'a b'], timeout_secs: 60 }], init: { command: ['/opt/legacy'], timeout_secs: 30 } };
  draw({ templateId: 'template' });
  const commands = await screen.findByRole('region', { name: 'Commands after start' });
  const names = within(commands).getAllByRole('listitem').map((row) => row.textContent);
  expect(names[0]).toContain('90-template'); expect(names[1]).toContain('init'); expect(names[2]).toContain('10-inherited');
  expect(within(commands).queryByDisplayValue('90-template')).not.toBeInTheDocument();
  expect(within(commands).queryByDisplayValue('/opt/template')).not.toBeInTheDocument();
});

it('saves only this layer, preserving literal arguments and a null removal of an inherited name', async () => {
  const user = userEvent.setup();
  server.rows = [setting('env.after_up', { '10-inherited': { command: ['inherited'] } })];
  draw();
  const commands = await screen.findByRole('region', { name: 'Commands after start' });
  await user.click(within(commands).getByRole('button', { name: 'Remove inherited 10-inherited here' }));
  await user.click(within(commands).getByRole('button', { name: 'Add command' }));
  await user.type(within(commands).getByLabelText('Command name'), '20-cache');
  await user.type(within(commands).getByLabelText('Program'), '/opt/cache');
  fireEvent.change(within(commands).getByLabelText('Arguments · one per line'), { target: { value: 'a path with spaces\n$(do-not-expand)\n--literal' } });
  fireEvent.change(within(commands).getByLabelText('Time limit · seconds'), { target: { value: '45' } });
  await user.click(within(commands).getByRole('button', { name: 'Save commands' }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith({ kind: 'environment', id: 'unit' }, 'env.after_up', {
    expectedRevision: null, value: { '10-inherited': null, '20-cache': { command: ['/opt/cache', 'a path with spaces', '$(do-not-expand)', '--literal'], timeout_secs: 45 } },
  }));
});

it('requires provider ownership even when the setting is served writable to a manager', async () => {
  server.rows = [setting('runtime.after_up', {}, { own: { value: { warm: { command: ['/opt/warm'], timeout_secs: 300 } }, revision: 4, locked: false } })];
  draw({ pool: 'provider' });
  const commands = await screen.findByRole('region', { name: 'Commands after start' });
  expect(within(commands).getByText(i18n.t('unitConfiguration.credentialWarning'),{exact:true})).toBeVisible();
  expect(within(commands).getByText("Only this provider's owners can change these commands.")).toBeInTheDocument();
  expect(within(commands).getByText('warm')).toBeInTheDocument();
  expect(within(commands).queryByRole('textbox')).not.toBeInTheDocument();
  expect(within(commands).queryByRole('button', { name: 'Save commands' })).not.toBeInTheDocument();
  expect(document.querySelector('[data-setting="runtime.after_up"]')).toBeNull();
  expect(putSetting).not.toHaveBeenCalled();
});

it.each(['managed','connected'] as const)('offers provider variables, sealed-secret names and commands by attach facts for %s display placement',async (placement)=>{
  server.rows=[setting('runtime.variables',{}),setting('runtime.secret_names',[]),setting('runtime.after_up',{}),setting('runtime.template_id',null),setting('runtime.host_id',null)];
  draw({pool:'provider',placement,lifecycle:'attach',owns:true,commands:{supported:true,reason:null},afterUp:false});
  const values=await screen.findByRole('list',{name:'Unit configuration'});
  const variables=visibleSetting(values,'runtime.variables');
  const secrets=visibleSetting(values,'runtime.secret_names');
  for(const key of ['runtime.template_id','runtime.host_id'])expect(values.querySelector(`[data-setting="${key}"]`)).toBeNull();
  expect(within(variables).getByText('Next attachment')).toBeVisible();
  expect(within(secrets).getByText('Next attachment')).toBeVisible();
  const commands=await screen.findByRole('region',{name:'Commands after start'});
  expect(within(commands).getByRole('button',{name:'Save commands'})).toBeInTheDocument();
  expect(screen.getByText(i18n.t('unitConfiguration.commandsPolicyOff'),{exact:true})).toBeVisible();
  expect(within(commands).queryByText('Template commands')).not.toBeInTheDocument();
});

it('shows the declared command constraint and keeps those commands read-only even for an owner',async ()=>{
  const reason="This machine cannot stop a command's whole process group.";
  server.rows=[setting('runtime.after_up',{})];
  draw({pool:'provider',lifecycle:'attach',owns:true,commands:{supported:false,reason},afterUp:true});
  const commands=await screen.findByRole('region',{name:'Commands after start'});
  expect(within(commands).getByText(reason)).toBeInTheDocument();
  expect(within(commands).queryByRole('button',{name:'Save commands'})).not.toBeInTheDocument();
  expect(putSetting).not.toHaveBeenCalled();
});

it('lets an owner edit under the read revision and refuses empty programs or deadlines beyond the bounds', async () => {
  const user = userEvent.setup();
  server.rows = [setting('runtime.after_up', {}, { own: { value: {}, revision: 4, locked: false } })];
  draw({ pool: 'provider', owns: true });
  const commands = await screen.findByRole('region', { name: 'Commands after start' });
  await user.click(within(commands).getByRole('button', { name: 'Add command' }));
  expect(within(commands).getByRole('button', { name: 'Save commands' })).toBeDisabled();
  await user.type(within(commands).getByLabelText('Command name'), 'warm');
  await user.type(within(commands).getByLabelText('Program'), '/opt/warm');
  fireEvent.change(within(commands).getByLabelText('Time limit · seconds'), { target: { value: '3601' } });
  expect(within(commands).getByRole('button', { name: 'Save commands' })).toBeDisabled();
  fireEvent.change(within(commands).getByLabelText('Time limit · seconds'), { target: { value: '1' } });
  await user.click(within(commands).getByRole('button', { name: 'Save commands' }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith({ kind: 'provider', id: 'unit' }, 'runtime.after_up', { expectedRevision: 4, value: { warm: { command: ['/opt/warm'], timeout_secs: 1 } } }));
});

it('keeps a draft when a newer own revision arrives and lets the person explicitly read it', async () => {
  const user = userEvent.setup();
  server.rows = [setting('env.after_up', {}, { own: { value: {}, revision: 4, locked: false } })];
  const { client } = draw();
  const commands = await screen.findByRole('region', { name: 'Commands after start' });
  await user.click(within(commands).getByRole('button', { name: 'Add command' }));
  await user.type(within(commands).getByLabelText('Command name'), 'my-draft');
  act(() => { client.setQueryData(targetSettingsKey({ kind: 'environment', id: 'unit' }), [setting('env.after_up', {}, { own: { value: { remote: { command: ['/opt/remote'] } }, revision: 5, locked: false } })]); });
  expect(await within(commands).findByText(/These commands changed elsewhere/)).toBeInTheDocument();
  expect(within(commands).getByLabelText('Command name')).toHaveValue('my-draft');
  await user.click(within(commands).getByRole('button', { name: 'Read current commands' }));
  expect(within(commands).getByLabelText('Command name')).toHaveValue('remote');
  expect(within(commands).getByLabelText('Program')).toHaveValue('/opt/remote');
  expect(putSetting).not.toHaveBeenCalled();
});

it('offers common environment values and commands on a directory without offering a unit recipe', async () => {
  server.rows = [setting('env.after_up',{}),setting('env.template_id','template'),setting('env.host_id','host'),setting('env.resources',{memory_mb:64}),
    setting('network.allowed_hosts',[]),setting('env.variables',{}),setting('env.secret_names',[]),setting('workspace.index','full')];
  draw({ placement: 'connected',lifecycle:'attach',commands:{supported:true,reason:null},afterUp:false });
  const values = await screen.findByRole('list', { name: 'Unit configuration' });
  for(const key of ['env.variables','env.secret_names','workspace.index','env.resources'])visibleSetting(values,key);
  for (const key of ['env.template_id', 'env.host_id', 'network.allowed_hosts']) expect(values.querySelector(`[data-setting="${key}"]`)).toBeNull();
  expect(screen.getByRole('region', { name: 'Commands after start' })).toBeInTheDocument();
  expect(screen.queryByText(/Orch creates no unit and runs no commands after start here/)).not.toBeInTheDocument();
});

it('reads common provider settings for a directory runtime without inventing a unit recipe', async () => {
  server.rows=[setting('runtime.storage',{home:'fast'})];
  draw({ pool: 'provider', placement: 'connected',lifecycle:'attach' });
  const storage=await screen.findByRole('region',{name:'Storage'});
  await waitFor(()=>expect(storage).toHaveTextContent('fast'));
  const values=screen.getByRole('list',{name:'Unit configuration'});
  expect(values.querySelector('[data-setting="runtime.template_id"]')).toBeNull();
  expect(storage.querySelectorAll('[data-setting]')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Add command' })).not.toBeInTheDocument();
  expect(server.read).toHaveBeenCalled();
});

it('shows the stopped start failure with bounded first lines and a link to the setting', async () => {
  const detail = `after-start command cache exited with 7: first\n${Array.from({ length: 12 }, (_, index) => `output-${index + 1}`).join('\n')}`;
  const user=userEvent.setup();
  show(<UnitCommandFailure detail={detail} href="/resources/environments/unit?tab=settings#unit-after-up" />);
  expect(screen.getByText(i18n.t('ui.error.afterStart'),{exact:true})).toBeVisible();
  expect(screen.queryByText(/output-1/)).not.toBeInTheDocument();
  await user.click(screen.getByText(i18n.t('unitConfiguration.commandOutput'),{selector:'summary'}));
  expect(await screen.findByText(/output-9/)).toBeVisible();
  expect(screen.queryByText(/output-10/)).not.toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Change commands after start' })).toHaveAttribute('href', '/resources/environments/unit?tab=settings#unit-after-up');
  await user.click(screen.getByRole('button',{name:i18n.t('ui.preview.showAll')}));
  expect(screen.getByText(/output-12/)).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('ui.preview.showLess')}));
  expect(screen.queryByText(/output-10/)).not.toBeInTheDocument();
  await user.click(screen.getByText(i18n.t('unitConfiguration.commandOutput'),{selector:'summary'}));
  await waitFor(()=>expect(screen.queryByText(/output-9/)).not.toBeInTheDocument());
  expect(screen.getByText(i18n.t('ui.error.afterStart'),{exact:true})).toBeVisible();
});

it('shows a failed active job and does not mistake a generic status for a command failure', async () => {
  const result = show(<UnitCommandFailure detail="the machine is offline" />);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('link',{name:'Change commands after start'})).not.toBeInTheDocument();
  result.unmount();
  show(<UnitCommandFailure jobId="start-job" />);
  expect(await screen.findByText(i18n.t('ui.error.afterStart'),{exact:true})).toBeVisible();
  expect(screen.queryByText(/cache is locked/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByText(i18n.t('unitConfiguration.commandOutput'),{selector:'summary'}));
  expect(await screen.findByText(/cache is locked/)).toBeVisible();
  expect(screen.getByRole('link',{name:'Change commands after start'})).toHaveAttribute('href','#unit-after-up');
});
