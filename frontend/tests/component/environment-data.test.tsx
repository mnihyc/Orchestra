/** Workspace/home choices follow declared storage facts and the same roots projection on either page. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient } from '@tanstack/react-query';
import { applyRow } from '@/api/cache';
import { enrollmentHost, enrollmentMachine } from '../doubles/enrollment-row';
import { useState } from 'react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DataSourcesEditor, EnvironmentDirectoriesHere, madeDataSources } from '@/features/resources/environments/DataSources';
import type { MachineEnvironmentData } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const { read }=vi.hoisted(()=>({read:vi.fn<(id:string)=>Promise<MachineEnvironmentData>>() }));
vi.mock('@/api/resource-tools',()=>({machineEnvironmentDataQuery:(id:string)=>({queryKey:['machine-environment-data',id],queryFn:()=>read(id)})}));
vi.mock('@tanstack/react-router',()=>({Link:({params,children}:{params:{id:string};children:ReactNode})=><a href={`/resources/${params.id}`}>{children}</a>}));
const facts=():MachineEnvironmentData=>({machine_id:'node',roots:[{path:'/srv/work',uses:[{environment:{id:'env',name:'Project',status:'running'},role:'workspace',path:'/srv/work/project'}]}],direct:{made_by_orch:'directory',existing_directory:true,reason:null},drivers:{container:{made_by_orch:'directory',existing_directory:true,reason:null}}});
beforeEach(()=>{vi.clearAllMocks();read.mockResolvedValue(facts());});
afterEach(async()=>{await setLocale('en');});
function Form({driver}:{driver?:string}) {
  const [value,setValue]=useState(madeDataSources);
  return <><DataSourcesEditor machine="node" driver={driver} value={value} onChange={setValue}/><output aria-label="selection">{JSON.stringify(value)}</output></>;
}

it('uses the declared new-directory choice even for a driver named container, and keeps home independent',async()=>{
  const user=userEvent.setup();show(<Form driver="container"/>);
  await waitFor(()=>expect(read).toHaveBeenCalledWith('node'));
  await user.click(screen.getByRole('combobox',{name:'Workspace'}));
  const existing=()=>screen.getByRole('option',{name:i18n.t('choiceCatalog.dataSource.existing_directory.name')});
  await waitFor(()=>{
    expect(screen.getByRole('option',{name:i18n.t('choiceCatalog.dataSource.made_by_orch.name')})).toHaveAccessibleDescription('A new directory');
    expect(existing()).toHaveAccessibleDescription(i18n.t('environmentData.allowed'));
  });
  await user.click(existing());
  const path=screen.getByRole('textbox',{name:'Directory on this machine'});
  await user.clear(path);await user.type(path,'/srv/work/project');
  expect(screen.getByLabelText('selection')).toHaveTextContent('"workspace":{"kind":"existing_directory","path":"/srv/work/project"}');
  expect(screen.getByLabelText('selection')).toHaveTextContent('"home":{"kind":"made_by_orch"}');
  expect(screen.getByText(/Volume size limits do not apply/)).toBeVisible();
});

for(const language of ['en','zh-CN'] as const) it(`shows allowed roots and readable environment uses (${language})`,async()=>{
  await setLocale(language);show(<EnvironmentDirectoriesHere machine="node"/>);
  expect(await screen.findByText('/srv/work')).toBeVisible();
  expect(screen.getByRole('link',{name:'Project'})).toHaveAttribute('href','/resources/env');
  expect(screen.getByText('/srv/work/project')).toBeVisible();
  expect(screen.getByText(language==='en'?/Directories for environments/:/环境可用的目录/)).toBeVisible();
  expect(screen.queryByText(/environment\.roots/)).not.toBeInTheDocument();
  expect(screen.getByRole('link',{name:language==='en'?"Open the machine's policy":'打开机器策略'})).toBeVisible();
});

it('shows a real driver refusal without enabling an existing directory choice',async()=>{
  const value=facts();value.direct={made_by_orch:'directory',existing_directory:false,reason:'This driver cannot bind an existing path.'};read.mockResolvedValue(value);
  const user=userEvent.setup();show(<Form/>);
  await waitFor(()=>expect(read).toHaveBeenCalled());await user.click(screen.getByRole('combobox',{name:'Workspace'}));
  const existing=()=>screen.getByRole('option',{name:i18n.t('choiceCatalog.dataSource.existing_directory.name')});
  // The refusal is the driver's own answer: the choice says so once it has been applied.
  await waitFor(()=>{
    expect(existing()).toHaveAttribute('aria-disabled','true');
    expect(existing()).toHaveAccessibleDescription('This driver cannot bind an existing path.');
  });
  expect(screen.getByText('This driver cannot bind an existing path.')).toBeVisible();
});

it('keeps general advice behind one question and puts an outside-root error beside the path',async()=>{
  const user=userEvent.setup();show(<Form/>);
  await waitFor(()=>expect(read).toHaveBeenCalled());
  expect(screen.queryByText(i18n.t('environmentData.choices'))).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:i18n.t('help.hint.label',{term:i18n.t('environmentData.title')})}));
  expect(screen.getByText(i18n.t('environmentData.choices'))).toBeVisible();
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('combobox',{name:'Workspace'}));
  await user.click(screen.getByRole('option',{name:i18n.t('choiceCatalog.dataSource.existing_directory.name')}));
  const path=screen.getByRole('textbox',{name:'Directory on this machine'});
  await user.clear(path);await user.type(path,'/outside/project');
  expect(path).toHaveAttribute('aria-invalid','true');
  expect(path).toHaveAccessibleDescription(i18n.t('environmentData.chooseAllowed'));
});

for (const kind of ['host', 'machine'] as const) {
  it(`uses newly reported ${kind} storage facts without reopening the form`, async () => {
    const initial = facts(); initial.drivers = {}; initial.direct = null;
    read.mockResolvedValueOnce(initial);
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const before = kind === 'host'
      ? { ...enrollmentHost('host', true), machine: { ...enrollmentHost('host', true).machine, id: 'node' } }
      : enrollmentMachine('node', true);
    cache.setQueryData(kind === 'host' ? ['hosts', null] : [kind, before.id], kind === 'host' ? [before] : before);
    const view = show(<Form {...(kind === 'host' ? { driver: 'container' } : {})} />, cache);
    const user = userEvent.setup();
    try {
      const choice = screen.getByRole('combobox', { name: 'Workspace' });
      await waitFor(() => expect(choice).toBeEnabled());
      await user.click(choice);
      expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.dataSource.made_by_orch.name') })).toHaveAttribute('aria-disabled', 'true');
      await user.keyboard('{Escape}');
      read.mockResolvedValue(facts());
      const value = kind === 'host'
        ? { ...before, drivers: [{ name: 'container', title: 'Container', creates: ['environment'], runtime_classes: ['default'] }] }
        : { ...before, facts: { environment_descriptor: { data_storage: facts().direct } } };
      await act(async () => { applyRow(cache, { kind, id: before.id, value }); await Promise.resolve(); });
      await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(choice).toBeEnabled());
      await user.click(choice);
      await waitFor(() => expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.dataSource.made_by_orch.name') })).not.toHaveAttribute('aria-disabled', 'true'));
      expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.dataSource.made_by_orch.name') })).toHaveAccessibleDescription('A new directory');
    } finally { view.unmount(); cache.clear(); }
  });
}

it('keeps directory choices quiet for load samples and another host, and expires a hidden projection without reading it', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const host = { ...enrollmentHost('host', true), machine: { ...enrollmentHost('host', true).machine, id: 'node' } };
  cache.setQueryData(['host', host.id], host);
  const view = show(<Form driver="container" />, cache);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Workspace' })).toBeEnabled());
  expect(read).toHaveBeenCalledTimes(1);
  await act(async () => {
    applyRow(cache, { kind: 'host', id: host.id, value: { ...host, usage: { memory_used_mb: 100, cpu_percent_milli: 1000, observed_at: '2026-11-01T00:00:00Z' } } });
    const other = enrollmentHost('other', true);
    applyRow(cache, { kind: 'host', id: other.id, value: other });
    await Promise.resolve();
  });
  expect(read).toHaveBeenCalledTimes(1);
  view.unmount();
  applyRow(cache, { kind: 'host', id: host.id, value: { ...host, drivers: [{ name: 'container', title: 'Container', creates: ['environment'], runtime_classes: ['default'] }] } });
  expect(cache.getQueryData(['machine-environment-data', 'node'])).toBeUndefined();
  expect(read).toHaveBeenCalledTimes(1);
  cache.clear();
});

it('does not let an older directory read or a queued refresh revive a host that is no longer visible', async () => {
  let finish: (value: MachineEnvironmentData) => void = () => { throw new Error('the form must read its directory choices'); };
  const pending = new Promise<MachineEnvironmentData>((resolve) => { finish = resolve; });
  read.mockImplementationOnce(() => pending);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const host = { ...enrollmentHost('host', true), machine: { ...enrollmentHost('host', true).machine, id: 'node' } };
  cache.setQueryData(['host', host.id], host);
  const view = show(<Form driver="container" />, cache);
  try {
    await waitFor(() => expect(read).toHaveBeenCalledTimes(1));
    await act(async () => {
      applyRow(cache, { kind: 'host', id: host.id, value: { ...host, drivers: [{ name: 'container', title: 'Container', creates: ['environment'], runtime_classes: ['default'] }] } });
      applyRow(cache, { kind: 'host', id: host.id, value: null });
      await Promise.resolve();
    });
    await act(async () => { finish(facts()); await pending; });
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(['machine-environment-data', 'node'])).toBeUndefined();
    expect(cache.getQueryState(['machine-environment-data', 'node'])?.status).toBe('error');
    expect(screen.getByRole('combobox', { name: 'Workspace' })).toBeDisabled();
  } finally { view.unmount(); cache.clear(); }
});
