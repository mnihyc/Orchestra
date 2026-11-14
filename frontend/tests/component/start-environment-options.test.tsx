/** The control for where a thread runs: one-line rows and a search, "New environment…" first with a form that chooses its host and template, the default said where it is inherited or passed over, and an agent that an environment shuts said with the act that lifts it. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as EnvironmentsApi from '@/api/environments';
import type * as ForksApi from '@/api/forks';
import type * as ResourcesApi from '@/api/resources';
import type * as StartApi from '@/api/start';
import type { CreationAccessContext, StartOptions } from '@/api/start';
import { NO_PICKS, type NewPlacement, type Picks, resolve } from '@/features/start/choices';
import { useStartOptions } from '@/features/start/options';
import { Where } from '@/features/start/Where';
import type { PlaceRef } from '@/features/navigation/places';
import type { BindingEnvironment, Environment, MachineRef, NodeRef, Place, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { agent, arrangement, environment, host, options, provider } from '../unit/start-fixtures';
import { deferred } from './deferred';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<(environmentId: string | null, context?: CreationAccessContext) => Promise<StartOptions>>(),
  places: vi.fn<(node: string, template: string | null) => Promise<Place[]>>(),
  templates: vi.fn<() => Promise<Template[]>>(),
  environment: vi.fn<(id: string) => Promise<Environment>>(),
  update: vi.fn<(id: string, body: unknown) => Promise<Environment>>(),
}));
vi.mock('@/api/start', async (load) => {
  const actual = await load<typeof StartApi>();
  return { ...actual, startOptionsQuery: (node: string, id: string | null, context?: CreationAccessContext) => ({ ...actual.startOptionsQuery(node, id, context), queryFn: () => calls.read(id, context) }) };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), templatesQuery: () => ({ queryKey: ['where-templates'], queryFn: () => calls.templates() }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(), placesQuery: (node: string, template: string | null = null) => ({ queryKey: ['where-places', node, template], queryFn: () => calls.places(node, template) }) }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof EnvironmentsApi>(),
  environmentQuery: (id: string) => ({ queryKey: ['where-environment', id], queryFn: () => calls.environment(id) }),
  updateEnvironment: (id: string, body: unknown) => calls.update(id, body) }));

const place: PlaceRef = { id: 'topic', kind: 'topic', name: 'Work', parentId: null, archived: false, status: null, folder: false };
const project: NodeRef = { id: 'project', kind: 'project', name: 'Web', redacted: false };
const laptop: MachineRef = { id: 'm1', name: 'Ada laptop', online: true, placement: 'connected', platform_os: 'linux' };
const template = (id: string, name: string) => ({ id, name, enabled: true, access: { permissions: ['template.use'] }, description: '' }) as unknown as Template;
beforeEach(() => {
  calls.read.mockReset().mockResolvedValue(options());
  calls.places.mockReset().mockResolvedValue([]);
  calls.templates.mockReset().mockResolvedValue([]);
  calls.environment.mockReset();
  calls.update.mockReset();
});
afterEach(async () => { cleanup(); act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

function Placement({ offered, initial = null }: { offered: StartOptions; initial?: string | null }) {
  const [value, setValue] = useState<string | null>(initial);
  const [requesting, setRequesting] = useState<BindingEnvironment | null>(null);
  const [made, setMade] = useState<NewPlacement>({});
  return <><Where compact place={place} options={offered} value={value} onChange={setValue} machines={[]} homeId="org"
    requesting={requesting} onRequestingChange={setRequesting} newEnvironment={made} onNewEnvironmentChange={setMade} />
    <output aria-label="Creation choices">{JSON.stringify(made)}</output></>;
}
function Standing() {
  const [picks, setPicks] = useState<Picks>(NO_PICKS);
  const state = useStartOptions(place.id, picks, { suggest: true });
  if (state.options === null || state.resolved === null) return <p role="status">Checking placement</p>;
  return <><span data-placement-origin>{state.resolved.origins.environment}</span>
    <Where compact place={place} options={state.options} value={state.resolved.fresh ? 'new' : state.resolved.environment?.environment.id ?? null}
      onChange={(environment) => setPicks((before) => ({ ...before, environment }))} machines={[]} homeId="org" requesting={null} onRequestingChange={() => undefined}
      note={state.resolved.environmentNote} agentHere={state.resolved.agentHere} /></>;
}
const open = async (user: ReturnType<typeof userEvent.setup>) => { await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') })); };

for (const locale of ['en', 'zh-CN'] as const) {
  it('lists environments one to a line with the host beside the name and a state only where it is not ready in '+locale, async () => {
    await setLocale(locale);
    const ready = environment('ready'); ready.host = { id: 'h1', name: 'Build host', address: 'acme.h1' };
    const stopped = environment('stopped'); stopped.environment.status = 'stopped'; stopped.machine = laptop;
    const user = userEvent.setup();
    show(<Placement offered={options({ environments: [ready, stopped] })} initial="ready" />);
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue('Env ready');
    await open(user);
    expect(screen.getAllByRole('option')[0]).toHaveAccessibleName(i18n.t('start.where.newRow'));
    expect(screen.getByRole('option', { name: 'Env ready' })).not.toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByText(/Build host/u)).toBeVisible();
    expect(screen.getByText(new RegExp(laptop.name, 'u'))).toBeVisible();
    expect(screen.getByText(new RegExp(i18n.t('environments.status.stopped'), 'u'))).toBeVisible();
    expect(screen.queryByText(new RegExp(i18n.t('environments.status.running'), 'u'))).toBeNull();
  });

  it('narrows the list by name and by host and keeps New environment in reach in '+locale, async () => {
    await setLocale(locale);
    const ready = environment('ready'); ready.host = { id: 'h1', name: 'Build host', address: 'acme.h1' };
    const other = environment('other');
    const user = userEvent.setup();
    show(<Placement offered={options({ environments: [ready, other] })} initial="other" />);
    await open(user);
    await user.type(screen.getByRole('combobox', { name: i18n.t('start.where.title') }), 'build');
    expect(screen.getByRole('option', { name: 'Env ready' })).toBeVisible();
    expect(screen.queryByRole('option', { name: 'Env other' })).toBeNull();
    await user.clear(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
    await user.type(screen.getByRole('combobox', { name: i18n.t('start.where.title') }), 'nothing like it');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option', { name: i18n.t('start.where.newRow') })).toBeVisible();
  });

  it('makes the host and the template of a new environment selectable and sends what was chosen in '+locale, async () => {
    await setLocale(locale);
    calls.templates.mockResolvedValue([template('t1', 'Standard'), template('t2', 'Heavy')]);
    calls.places.mockImplementation((_node, id) => Promise.resolve(id === 't2' ? [host('h3', 'Heavy host', { automatic: true })] : []));
    const user = userEvent.setup();
    show(<Placement offered={options({ hosts: [host('h1', 'Roomy host', { automatic: true }), host('h2', 'Quiet host')] })} initial="e1" />);
    await open(user);
    await user.click(screen.getByRole('option', { name: i18n.t('start.where.newRow') }));
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(i18n.t('start.where.newName'));
    const hostMenu = screen.getByRole('combobox', { name: i18n.t('start.where.host') });
    expect(hostMenu).toHaveDisplayValue(i18n.t('start.where.hostAutoNamed', { host: 'Roomy host' }));
    await user.click(hostMenu);
    await user.click(screen.getByRole('option', { name: 'Quiet host' }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ host_id: 'h2' }));
    await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.template') }));
    await user.click(screen.getByRole('option', { name: 'Heavy' }));
    // Another template reads its own hosts, and the host chosen for the first is not carried over.
    await waitFor(() => expect(calls.places).toHaveBeenCalledWith('topic', 't2'));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ template_id: 't2' }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('start.where.host') })).toHaveDisplayValue(i18n.t('start.where.hostAutoNamed', { host: 'Heavy host' })));
  });

  it('finds a host and a template in large creation lists without changing either until chosen in '+locale, async () => {
    await setLocale(locale);
    const hosts = Array.from({ length: 12 }, (_, at) => host(`host-${at}`, `Workstation ${at}`));
    calls.templates.mockResolvedValue(Array.from({ length: 12 }, (_, at) => template(`template-${at}`, `Workspace template ${at}`)));
    calls.places.mockResolvedValue(hosts);
    const user = userEvent.setup();
    show(<Placement offered={options({ hosts })} initial="new" />);
    const hostPicker = screen.getByRole('combobox', { name: i18n.t('start.where.host') });
    await user.click(hostPicker);
    await user.type(hostPicker, 'Workstation 11');
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('{}');
    expect(screen.queryByRole('option', { name: 'Workstation 10' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('option', { name: 'Workstation 11' }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ host_id: 'host-11' }));
    const templatePicker = await screen.findByRole('combobox', { name: i18n.t('start.where.template') });
    await user.click(templatePicker);
    await user.type(templatePicker, 'Workspace template 11');
    expect(screen.queryByRole('option', { name: 'Workspace template 10' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('option', { name: 'Workspace template 11' }));
    await waitFor(() => expect(calls.places).toHaveBeenCalledWith('topic', 'template-11'));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ template_id: 'template-11' }));
  });

  it('starts on a default that a place above the topic sets and says where it is set in '+locale, async () => {
    await setLocale(locale);
    const offered = options({ environments: [environment('spare', { isDefault: true }), environment('e2')], default_environment: { environment: 'spare', from: project, reason: null } });
    calls.read.mockResolvedValue(offered);
    show(<Standing />);
    expect(await screen.findByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue('Env spare');
    expect(document.querySelector('[data-environment-note="inherited"]')).toHaveTextContent(i18n.t('start.where.defaultFrom', { place: 'Web' }));
    expect(document.querySelector('[data-placement-origin]')).toHaveTextContent('shared_default');
    expect(calls.read).toHaveBeenCalledOnce();
  });

  it('says why a default that cannot be used is passed over and what stands instead in '+locale, async () => {
    await setLocale(locale);
    const failed = { ...environment('spare', { usable: false }), reason: 'environment_failed' };
    const offered = options({ environments: [failed, environment('e2')], default_environment: { environment: 'spare', from: project, reason: 'environment_failed' } });
    calls.read.mockResolvedValue(offered);
    show(<Standing />);
    const note = await waitFor(() => { const line = document.querySelector('[data-environment-note="unusable"]'); expect(line).not.toBeNull(); return line; });
    expect(note).toHaveTextContent(i18n.t('start.where.defaultUnusable', { name: 'Env spare', why: i18n.t('start.reason.environment_failed') }));
    expect(note).toHaveTextContent(i18n.t('start.where.defaultSetOn', { place: 'Web' }));
    expect(note).toHaveTextContent(i18n.t('start.where.insteadNew'));
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(i18n.t('start.where.newName'));
  });

  it('says when the place gives every thread an environment of its own, so a default is not used, in '+locale, async () => {
    await setLocale(locale);
    const offered = options({ environment_default: 'fresh', environments: [environment('spare', { isDefault: true })], default_environment: { environment: 'spare', from: null, reason: null } });
    calls.read.mockResolvedValue(offered);
    show(<Standing />);
    const note = await waitFor(() => { const line = document.querySelector('[data-environment-note="fresh"]'); expect(line).not.toBeNull(); return line; });
    expect(note).toHaveTextContent(i18n.t('start.where.defaultFresh', { name: 'Env spare' }));
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(i18n.t('start.where.newName'));
  });
}

/** An answer in which the profile a place defaults to cannot run in the one environment, for the reason the engine's sentence gives. */
function shut(detail: string | null, machine: MachineRef | null = laptop): StartOptions {
  const closed = { ...arrangement('separated', 1, false, 'executor_missing'), detail };
  const row = environment('e1', { isDefault: true }); row.machine = machine;
  return options({ providers: [provider('p1', { agents: [agent('a1', { arrangements: [closed] }), agent('a2')] })], environments: [row] });
}
function Shut({ offered, refresh = () => undefined }: { offered: StartOptions; refresh?: () => void }) {
  const resolved = resolve(offered, NO_PICKS, { suggest: true });
  return <Where compact place={place} options={offered} value="e1" onChange={() => undefined} machines={[]} homeId="org" requesting={null} onRequestingChange={() => undefined}
    onRefresh={refresh} agentHere={resolved.agentHere} />;
}
const VERSION = "Codex on Ada laptop is 0.169.0; this agent runs 0.167.2. An environment manager can allow it on the environment's page.";
const environmentOf = (manage: boolean): Environment => ({ ...detailEnvironment, id: 'e1', config: {}, revision: 3,
  access: { ...detailEnvironment.access, permissions: manage ? ['environment.view', 'environment.manage'] : ['environment.view'] },
  exec: { ...detailEnvironment.exec, machine_programs: [{ harness_kind: 'codex', path: '/usr/bin/codex', version: '0.169.0', approval: null }] } });

for (const locale of ['en', 'zh-CN'] as const) {
  it('says that an environment shuts the agent, why, what runs instead, and lets a manager allow the version in place in '+locale, async () => {
    await setLocale(locale);
    calls.environment.mockResolvedValue(environmentOf(true));
    calls.update.mockResolvedValue(environmentOf(true));
    const refresh = vi.fn();
    const user = userEvent.setup();
    show(<Shut offered={shut(VERSION)} refresh={refresh} />);
    const line = document.querySelector('[data-agent-here]');
    expect(line).toHaveTextContent(i18n.t('start.where.cannotRun', { agent: 'Agent a1', where: 'Env e1' }));
    expect(line).toHaveTextContent(i18n.t('environmentTools.versionMismatch', { machine: 'Ada laptop', programVersion: '0.169.0', agentVersion: '0.167.2' }));
    expect(line).toHaveTextContent(i18n.t('start.where.using', { agent: 'Agent a2' }));
    await user.click(await screen.findByRole('button', { name: i18n.t('start.where.allowVersions') }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledOnce());
    expect(calls.update).toHaveBeenCalledWith('e1', { expected_revision: 3, config: { 'exec.version_overrides': { codex: { path: '/usr/bin/codex', version: '0.169.0' } } } });
    await waitFor(() => expect(refresh).toHaveBeenCalledOnce());
  });

  it('names whom to ask where the person may not change the environment in '+locale, async () => {
    await setLocale(locale);
    calls.environment.mockResolvedValue(environmentOf(false));
    show(<Shut offered={shut(VERSION)} />);
    expect(await screen.findByText(i18n.t('start.where.askOwner', { name: detailEnvironment.owner.display_name }))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('start.where.allowVersions') })).toBeNull();
    expect(screen.getByRole('link', { name: i18n.t('start.where.openTools') })).toHaveAttribute('href', '/resources/environments/e1?tab=tools');
    expect(calls.update).not.toHaveBeenCalled();
  });

  it('says that the machine has not confirmed the agent program and points to the machine in '+locale, async () => {
    await setLocale(locale);
    show(<Shut offered={shut("Codex on Ada laptop is not confirmed. The machine's owner can confirm it on the machine's page.")} />);
    expect(document.querySelector('[data-agent-here]')).toHaveTextContent(i18n.t('machinePrograms.notConfirmed', { name: 'Codex', machine: 'Ada laptop' }));
    expect(screen.getByRole('link', { name: i18n.t('start.where.openMachine') })).toHaveAttribute('href', '/resources/machines/m1');
  });

  it('puts the executors an environment offers into plain words and offers no act for them in '+locale, async () => {
    await setLocale(locale);
    show(<Shut offered={shut('The environment offers codex-exec-server, orch-exec:plain and the harness can work through codex-exec-server.', null)} />);
    const line = document.querySelector('[data-agent-here]');
    expect(line).toHaveTextContent(i18n.t('start.reason.executor_missing'));
    expect(line).not.toHaveTextContent('orch-exec');
    expect(screen.queryByRole('button', { name: i18n.t('start.where.allowVersions') })).toBeNull();
  });
}

it('names the owner of an environment only where it is somebody else’s', async () => {
  client.session.setState({ status: 'signed_in', userId: 'u1' });
  const mine = environment('mine');
  const theirs = environment('theirs'); theirs.environment.owner = { ...theirs.environment.owner, id: 'u2', display_name: 'Bea Smith' };
  const user = userEvent.setup();
  show(<Placement offered={options({ environments: [mine, theirs] })} initial="mine" />);
  await open(user);
  expect(screen.getByText(/Bea Smith/)).toBeVisible();
  expect(screen.queryByText(/Ada Lovelace/)).toBeNull();
});

it('holds hundreds of environments in one field and finds one by what is typed', async () => {
  const many = Array.from({ length: 200 }, (_, index) => environment(`n${index}`));
  const user = userEvent.setup();
  show(<Placement offered={options({ environments: many })} initial="n0" />);
  // The field is all that stands on the page until it is opened.
  expect(screen.queryAllByRole('option')).toHaveLength(0);
  await open(user);
  // The words arrive at once: the list narrows from two hundred rows to one, and the case does not draw the rows again for every letter.
  await user.paste('n199');
  const rows = screen.getAllByRole('option');
  expect(rows).toHaveLength(2);
  expect(rows[0]).toHaveAccessibleName(i18n.t('start.where.newRow'));
  expect(rows[1]).toHaveAccessibleName('Env n199');
});

it('shows the no-host reason in Where and reads the fallback environment’s actual context', async () => {
  const offered = options({ hosts: [], environment_default: 'fresh' });
  const selected = deferred<StartOptions>();
  calls.read.mockImplementation((id) => id === null ? Promise.resolve(offered) : selected.promise);
  function Fallback() {
    const [picks, setPicks] = useState<Picks>(NO_PICKS);
    const state = useStartOptions(place.id, picks, { suggest: true }, true, { threadAccess: 'private' });
    if (state.options === null || state.resolved === null) return <p role="status">Checking placement</p>;
    return <><span data-placement-ready>{state.refreshing ? 'checking' : 'ready'}</span><span data-placement-arrangement>{state.resolved.arrangement}</span><span data-placement-origin>{state.resolved.origins.environment}</span>
      <Where compact place={place} options={state.options} value={state.resolved.fresh ? 'new' : state.resolved.environment?.environment.id ?? null} onChange={(environment) => setPicks((before) => ({ ...before, environment }))}
        machines={[]} homeId="org" requesting={null} onRequestingChange={() => undefined} /></>;
  }
  const user = userEvent.setup();
  show(<Fallback />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith('e1', { threadAccess: 'private' }));
  expect(document.querySelector('[data-placement-ready]')).toHaveTextContent('checking');
  await act(async () => { selected.resolve(offered); await selected.promise; });
  // The reason is the status line that is shown, and also the disabled New environment row's description.
  const noHost = i18n.t('start.where.noHost');
  await waitFor(() => expect(screen.getAllByRole('status').find((node) => node.textContent === noHost)).toBeVisible());
  const where = screen.getByRole('combobox', { name: i18n.t('start.where.title') });
  expect(where).toHaveDisplayValue('Env e1');
  await open(user);
  const row = screen.getByRole('option', { name: i18n.t('start.where.newRow') });
  expect(row).toHaveAttribute('aria-disabled', 'true');
  expect(row).toHaveAccessibleDescription(expect.stringContaining(noHost));
  expect(document.querySelector('[data-placement-arrangement]')).toHaveTextContent('separated');
  expect(document.querySelector('[data-placement-origin]')).toHaveTextContent('suggested');
  expect(calls.read.mock.calls.map(([id]) => id)).toEqual([null, 'e1']);
});
