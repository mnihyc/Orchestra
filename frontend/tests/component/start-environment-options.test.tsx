/** The control for where a thread runs: one-line rows and a search, "New environment…" first with a form that chooses its host and template, the default said where it is inherited or passed over, and an agent that an environment shuts said with the act that lifts it. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, client } from '@/api/client';
import type * as ResourceToolsApi from '@/api/resource-tools';
import { Dialog } from '@/ui/Dialog';
import { readViewChoice, writeViewChoice } from '@/lib/viewChoices';
import type * as EnvironmentsApi from '@/api/environments';
import type * as ForksApi from '@/api/forks';
import type * as ResourcesApi from '@/api/resources';
import type * as StartApi from '@/api/start';
import type { CreationAccessContext, StartOptions } from '@/api/start';
import { NO_PICKS, type NewPlacement, type Picks, resolve } from '@/features/start/choices';
import { useStartOptions } from '@/features/start/options';
import { Where } from '@/features/start/Where';
import { NewEnvironmentForm } from '@/features/start/NewEnvironment';
import type { PlaceRef } from '@/features/navigation/places';
import type { BindingEnvironment, Environment, Machine, MachineDirectoryListQuery, MachineDirectoryPage, MachineEnvironmentData, MachineRef, NodeRef, Place, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { agent, arrangement, environment, host, options, provider } from '../unit/start-fixtures';
import { deferred } from './deferred';
import { detailEnvironment, detailMachine } from './resource-detail-fixtures';
import { show, skipOverflowFallback } from './support';

// The placement cases prove filtering and retained choices, without measuring clipped text.
skipOverflowFallback();

const calls = vi.hoisted(() => ({
  read: vi.fn<(environmentId: string | null, context?: CreationAccessContext) => Promise<StartOptions>>(),
  places: vi.fn<(node: string, template: string | null) => Promise<Place[]>>(),
  templates: vi.fn<() => Promise<Template[]>>(),
  environment: vi.fn<(id: string) => Promise<Environment>>(),
  update: vi.fn<(id: string, body: unknown) => Promise<Environment>>(),
  roots: vi.fn<(id: string) => Promise<MachineEnvironmentData>>(),
  directories: vi.fn<(id: string, query: MachineDirectoryListQuery) => Promise<MachineDirectoryPage>>(),
  register: vi.fn<(id: string, body: unknown) => Promise<Environment>>(),
  registerKey: vi.fn<(key: string) => void>(),
}));
vi.mock('@/api/start', async (load) => {
  const actual = await load<typeof StartApi>();
  return { ...actual, startOptionsQuery: (node: string, id: string | null, context?: CreationAccessContext) => ({ ...actual.startOptionsQuery(node, id, context), queryFn: () => calls.read(id, context) }) };
});
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), templatesQuery: () => ({ queryKey: ['where-templates'], queryFn: () => calls.templates() }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(), placesQuery: (node: string, template: string | null = null) => ({ queryKey: ['where-places', node, template], queryFn: () => calls.places(node, template) }) }));
vi.mock('@/api/environments', async (load) => ({ ...await load<typeof EnvironmentsApi>(),
  environmentQuery: (id: string) => ({ queryKey: ['where-environment', id], queryFn: () => calls.environment(id) }),
  updateEnvironment: (id: string, body: unknown) => calls.update(id, body),
  registerEnvironment: (id: string, body: unknown, key: string) => { calls.registerKey(key); return calls.register(id, body); } }));
vi.mock('@/api/resource-tools', async (load) => ({ ...await load<typeof ResourceToolsApi>(),
  machineEnvironmentDataQuery: (id: string, reader?: string) => ({ queryKey: ['where-directory-roots', id, reader], queryFn: () => calls.roots(id) }) }));
vi.mock('@/api/directories', () => ({ listMachineDirectories: (id: string, query: MachineDirectoryListQuery) => calls.directories(id, query) }));

const place: PlaceRef = { id: 'topic', kind: 'topic', name: 'Work', parentId: null, archived: false, status: null, folder: false };
const project: NodeRef = { id: 'project', kind: 'project', name: 'Web', redacted: false };
const laptop: MachineRef = { id: 'm1', name: 'Ada laptop', online: true, placement: 'connected', platform_os: 'linux' };
const template = (id: string, name: string) => ({ id, name, enabled: true, access: { permissions: ['template.use'] }, description: '' }) as unknown as Template;
beforeEach(() => {
  client.session.setState({ status: 'signed_in', userId: 'u1' });
  calls.read.mockReset().mockResolvedValue(options());
  calls.places.mockReset().mockResolvedValue([]);
  calls.templates.mockReset().mockResolvedValue([]);
  calls.environment.mockReset();
  calls.update.mockReset();
  calls.roots.mockReset().mockImplementation((id) => Promise.resolve({ machine_id: id, roots: [{ path: '/srv/first', uses: [] }, { path: '/srv/work', uses: [] }], direct: null, drivers: {} }));
  calls.directories.mockReset().mockImplementation((_id, query) => {
    if (query.read_kind === 'inspect') return Promise.resolve({ read_kind: 'inspect', entries: [], next_cursor: null });
    const recent: unknown = JSON.parse(query.recent ?? '[]');
    const order = Array.isArray(recent) ? recent : [];
    const entries = query.path === '' ? ['alpha', 'orch', 'organ'].filter((name) => name.startsWith(query.prefix)) : [];
    entries.sort((left, right) => { const l = order.indexOf(left), r = order.indexOf(right); return (l < 0 ? Infinity : l) - (r < 0 ? Infinity : r) || left.localeCompare(right); });
    return Promise.resolve({ read_kind: 'choices', entries, next_cursor: null });
  });
  calls.register.mockReset().mockResolvedValue(detailEnvironment);
  calls.registerKey.mockReset();
});
afterEach(async () => { vi.unstubAllGlobals(); cleanup(); act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

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
  it.each([400, 1440])('names each environment before its location, showing only another owner in '+locale+' at %ipx', async (width) => {
    vi.stubGlobal('innerWidth', width);
    await setLocale(locale);
    const ready = environment('ready'); ready.host = { id: 'h1', name: 'Build host', address: 'acme.h1' };
    const stopped = environment('stopped'); stopped.environment.status = 'stopped'; stopped.machine = laptop;
    stopped.environment.owner = { ...ready.environment.owner, id: 'other', display_name: 'Another owner' };
    const user = userEvent.setup();
    show(<Placement offered={options({ environments: [ready, stopped] })} initial="ready" />);
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue('Env ready');
    await open(user);
    expect(screen.getAllByRole('option')[0]).toHaveAccessibleName(i18n.t('start.where.newRow'));
    expect(screen.getByRole('option', { name: 'Env ready' })).not.toHaveAttribute('aria-disabled', 'true');
    const owned = screen.getByRole('option', { name: 'Env ready' });
    expect(owned).toHaveAccessibleDescription(/Build host/u);
    expect(screen.queryByText(ready.environment.owner.display_name)).toBeNull();
    const shared = screen.getByRole('option', { name: 'Env stopped' });
    expect(shared).toHaveAccessibleDescription(/Ada laptop.*Another owner/u);
    expect(shared.compareDocumentPosition(screen.getByText(laptop.name)) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Another owner' }).closest('[role="listbox"]')).toBeNull();
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

  it('keeps the selected host and template in the placement draft in '+locale, async () => {
    await setLocale(locale);
    calls.templates.mockResolvedValue([template('t1', 'Standard'), template('t2', 'Heavy')]);
    calls.places.mockImplementation((_node, id) => Promise.resolve(id === 't2' ? [host('h3', 'Heavy host', { automatic: true })] : []));
    const user = userEvent.setup();
    show(<Placement offered={options({ hosts: [host('h1', 'Roomy host', { automatic: true }), host('h2', 'Quiet host')] })} initial="e1" />);
    await open(user);
    await user.click(screen.getByRole('option', { name: i18n.t('start.where.newRow') }));
    expect(screen.getByRole('combobox', { name: i18n.t('start.where.title') })).toHaveDisplayValue(i18n.t('start.where.newName'));
    const hostMenu = screen.getByRole('combobox', { name: i18n.t('start.where.host') });
    expect(hostMenu).toHaveDisplayValue(i18n.t('start.where.hostDefault'));
    await user.click(hostMenu);
    await user.click(screen.getByRole('option', { name: 'Quiet host' }));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ host_id: 'h2' }));
    await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.template') }));
    await user.click(screen.getByRole('option', { name: 'Heavy' }));
    // Another template reads its own hosts, and the host chosen for the first is not carried over.
    await waitFor(() => expect(calls.places).toHaveBeenCalledWith('topic', 't2'));
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent(JSON.stringify({ template_id: 't2' }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('start.where.host') })).toHaveDisplayValue(i18n.t('start.where.hostDefault')));
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
    fireEvent.keyDown(hostPicker, { key: 'Enter', isComposing: true });
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('{}');
    fireEvent.keyDown(hostPicker, { key: 'Enter', keyCode: 229 });
    expect(screen.getByLabelText('Creation choices')).toHaveTextContent('{}');
    await user.keyboard('{Enter}');
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

it('keeps a removed host named until the person chooses another host', () => {
  function Draft({ hosts }: { hosts: ReturnType<typeof host>[] }) {
    const [value, onChange] = useState<NewPlacement>({ host_id: 'h2' });
    return <><NewEnvironmentForm place={place} hosts={hosts} value={value} onChange={onChange} /><output>{JSON.stringify(value)}</output></>;
  }
  const view = show(<Draft hosts={[host('h1', 'Roomy'), host('h2', 'Quiet')]} />);
  expect(screen.getByRole('combobox', { name: i18n.t('start.where.host') })).toHaveDisplayValue('Quiet');
  view.rerender(<Draft hosts={[host('h1', 'Roomy', { automatic: true })]} />);
  expect(screen.getByRole('combobox', { name: i18n.t('start.where.host') })).toHaveDisplayValue('Quiet');
  expect(screen.getByText(i18n.t('start.where.hostGone'))).toBeVisible();
  expect(screen.getByText('{"host_id":"h2"}')).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('reopens missing host and template choices with safe withheld names rather than identifiers (%s)', async (locale) => {
  await setLocale(locale);
  const hostId = '99f319f6-f63a-4a57-97d7-3d45f94d02e1';
  const templateId = '997b9bb5-7c57-4a84-920e-ea1e3bcda48c';
  const changed = vi.fn();
  const view = show(<NewEnvironmentForm place={place} hosts={[]} value={{ host_id: hostId, template_id: templateId }} onChange={changed} />);
  expect(screen.getByRole('combobox', { name: i18n.t('start.where.host') })).toHaveDisplayValue(i18n.t('start.where.unavailable'));
  const templateField = screen.getByRole('combobox', { name: i18n.t('start.where.template') });
  await waitFor(() => expect(templateField).toHaveDisplayValue(i18n.t('start.where.unavailable')));
  expect(view.container).not.toHaveTextContent(hostId);
  expect(view.container).not.toHaveTextContent(templateId);
  await waitFor(() => expect(screen.getAllByRole('status').find((line) => line.textContent.includes(i18n.t('start.where.hostGone'))))
    .toHaveTextContent(locale === 'en' ? 'This template is no longer available' : '此模板已不可用'));
  await userEvent.click(templateField);
  expect(await screen.findByRole('option', { name: i18n.t('start.where.unavailable') })).toHaveAttribute('aria-disabled', 'true');
  expect(changed).not.toHaveBeenCalled();
});

it('searches long host and template lists without showing template descriptions', async () => {
  calls.templates.mockResolvedValue(Array.from({ length: 100 }, (_, n) => ({ ...template(`t${n}`, `Template ${n}`), description: 'A long description that belongs on the template page' })));
  const user = userEvent.setup();
  show(<Placement offered={options({ hosts: Array.from({ length: 50 }, (_, n) => host(`h${n}`, `Host ${n}`)) })} initial="new" />);
  const hosts = screen.getByRole('combobox', { name: i18n.t('start.where.host') });
  await user.click(hosts);
  await user.type(hosts, 'Host 49');
  expect(screen.getAllByRole('option')).toHaveLength(1);
  await user.click(screen.getByRole('option', { name: 'Host 49' }));
  const templates = await screen.findByRole('combobox', { name: i18n.t('start.where.template') });
  await user.click(templates);
  await user.type(templates, 'Template 99');
  expect(screen.getAllByRole('option')).toHaveLength(1);
  expect(screen.queryByText('A long description that belongs on the template page')).toBeNull();
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
  expect(row).not.toHaveAttribute('aria-disabled', 'true');
  expect(document.querySelector('[data-placement-arrangement]')).toHaveTextContent('separated');
  expect(document.querySelector('[data-placement-origin]')).toHaveTextContent('suggested');
  expect(calls.read.mock.calls.map(([id]) => id)).toEqual([null, 'e1']);
});


function MachineDirectoryDialog({ machine }: { machine: Machine }) {
  const [value, choose] = useState<string | null>(null);
  return <Dialog open onOpenChange={() => undefined} title={i18n.t('start.where.machineTitle')}>
    <Where place={place} options={options()} value={value} onChange={choose} machines={[machine]} homeId="org"
      requesting={null} onRequestingChange={() => undefined} />
    <output aria-label="Registered directory environment">{value}</output>
  </Dialog>;
}
const directoryMachine = (id: string): Machine => ({ ...detailMachine, id, placement: 'connected', owner: { ...detailEnvironment.owner, id: 'u1' } });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`chooses a root and completes a short directory into the actual registration ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
    const machine = directoryMachine(`root-${locale}-${theme}-${width}`);
    const user = userEvent.setup(); const view = show(<MachineDirectoryDialog machine={machine} />);
    expect(calls.roots).not.toHaveBeenCalled(); expect(calls.directories).not.toHaveBeenCalled();
    await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
    const root = await screen.findByRole('combobox', { name: i18n.t('directoryChoice.root') });
    await user.click(root); await user.click(screen.getByRole('option', { name: '/srv/work' }));
    const short = screen.getByRole('combobox', { name: i18n.t('directoryChoice.subdirectory') });
    await user.type(short, 'or'); await screen.findByRole('option', { name: 'orch' });
    await user.keyboard('{Tab}'); expect(short).toHaveFocus(); expect(short).toHaveValue('orch/');
    await waitFor(() => expect(calls.directories).toHaveBeenCalledWith(machine.id, expect.objectContaining({ root: '/srv/work', path: 'orch' })));
    expect(calls.register).not.toHaveBeenCalled();
    await user.keyboard('{Escape}'); await user.click(screen.getByRole('button', { name: i18n.t('start.where.use') }));
    await waitFor(() => expect(calls.register).toHaveBeenCalledWith(machine.id, expect.objectContaining({ root: '/srv/work/orch', home_id: 'org' })));
    expect(calls.directories).toHaveBeenCalledWith(machine.id, expect.objectContaining({ read_kind: 'inspect', root: '/srv/work', path: 'orch' }));
    expect(await screen.findByLabelText('Registered directory environment')).toHaveTextContent(detailEnvironment.id);
    view.unmount(); show(<MachineDirectoryDialog machine={machine} />);
    await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
    await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('directoryChoice.root') })).toHaveDisplayValue('/srv/work'));
    await user.click(screen.getByRole('combobox', { name: i18n.t('directoryChoice.subdirectory') }));
    await waitFor(() => expect(screen.getAllByRole('option')[0]).toHaveAccessibleName('orch'));
  });
}

it('names an empty roots policy once on the field and lets its owner open that policy', async () => {
  const machine = directoryMachine('empty-roots');
  calls.roots.mockResolvedValue({ machine_id: machine.id, roots: [], direct: null, drivers: {} });
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  expect(await screen.findByText(i18n.t('directoryChoice.noRoots'))).toBeInTheDocument();
  expect(screen.getByRole('link', { name: i18n.t('environmentData.editPolicy') })).toHaveAttribute('href', `/resources/machines/${machine.id}?tab=policy`);
  expect(screen.getByRole('button', { name: i18n.t('start.where.use') })).toBeDisabled();
  expect(calls.directories).not.toHaveBeenCalled(); expect(calls.register).not.toHaveBeenCalled();
});

it('keeps the machine refusal on the short directory field without registering outside its roots', async () => {
  const machine = directoryMachine('refused-root');
  const message = 'environment.roots: the directory is outside the selected root';
  calls.directories.mockRejectedValue(new ApiError({ type: 'orch:error:request.invalid', title: 'Invalid directory', code: 'request.invalid', status: 400, request_id: 'directory-refusal', detail: '', errors: [{ path: 'path', message }] }, null));
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  const short = await screen.findByRole('combobox', { name: i18n.t('directoryChoice.subdirectory') });
  await user.type(short, 'escape/');
  expect(await screen.findByText(message)).toBeInTheDocument(); expect(short).toHaveAttribute('aria-invalid', 'true');
  expect(calls.register).not.toHaveBeenCalled();
});

it('ranks only this reader’s offered roots and directories', async () => {
  const machine = directoryMachine('reader-roots');
  writeViewChoice('another-reader', 'directory', 'roots', JSON.stringify(['/srv/private']), `machine:${machine.id}`);
  writeViewChoice('u1', 'directory', 'roots', JSON.stringify(['/srv/work']), `machine:${machine.id}`);
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  const root = await screen.findByRole('combobox', { name: i18n.t('directoryChoice.root') });
  await waitFor(() => expect(root).toHaveDisplayValue('/srv/work'));
  await user.click(root); expect(screen.queryByRole('option', { name: '/srv/private' })).not.toBeInTheDocument();
  expect(readViewChoice('another-reader', 'directory', 'roots', '[]', `machine:${machine.id}`)).toBe(JSON.stringify(['/srv/private']));
});


it('checks an uncompleted path natively before any environment is registered', async () => {
  const machine = directoryMachine('submit-refused-root');
  const message = 'environment.roots: the directory is outside the selected root';
  calls.directories.mockImplementation((_id, query) => query.read_kind === 'inspect'
    ? Promise.reject(new ApiError({ type: 'orch:error:request.invalid', title: 'Invalid directory', code: 'request.invalid', status: 400, request_id: 'submit-directory-refusal', detail: '', errors: [{ path: 'path', message }] }, null))
    : Promise.resolve({ read_kind: 'choices', entries: [], next_cursor: null }));
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  const short = await screen.findByRole('combobox', { name: i18n.t('directoryChoice.subdirectory') });
  await user.type(short, 'escape'); await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: i18n.t('start.where.use') }));
  expect(await screen.findByText(message)).toBeInTheDocument();
  expect(short).toHaveAttribute('aria-invalid', 'true'); expect(calls.register).not.toHaveBeenCalled();
});


it('retires a pending root read before another person opens directory registration', async () => {
  const machine = directoryMachine('roots-reader-race');
  const old = deferred<MachineEnvironmentData>();
  calls.roots.mockReturnValueOnce(old.promise).mockResolvedValue({ machine_id: machine.id, roots: [{ path: '/srv/new-reader', uses: [] }], direct: null, drivers: {} });
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  await waitFor(() => expect(calls.roots).toHaveBeenCalledTimes(1));
  act(() => client.session.setState({ userId: 'u2' }));
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  await waitFor(() => expect(screen.getByRole('combobox', { name: i18n.t('directoryChoice.root') })).toHaveDisplayValue('/srv/new-reader'));
  await act(async () => { old.resolve({ machine_id: machine.id, roots: [{ path: '/srv/private-old-reader', uses: [] }], direct: null, drivers: {} }); await old.promise; });
  expect(screen.getByRole('combobox', { name: i18n.t('directoryChoice.root') })).toHaveDisplayValue('/srv/new-reader');
  await user.click(screen.getByRole('combobox', { name: i18n.t('directoryChoice.root') }));
  expect(screen.queryByRole('option', { name: '/srv/private-old-reader' })).not.toBeInTheDocument();
});

it('reaches a sent registration receipt after a lost reply without checking a later changed path again', async () => {
  const machine = directoryMachine('roots-write-replay');
  calls.register.mockRejectedValueOnce(new Error('The write reply was lost')).mockResolvedValue(detailEnvironment);
  const user = userEvent.setup(); show(<MachineDirectoryDialog machine={machine} />);
  await user.click(screen.getByText(i18n.t('start.where.machineTitle'), { selector: 'summary' }));
  const short = await screen.findByRole('combobox', { name: i18n.t('directoryChoice.subdirectory') });
  await user.type(short, 'orch'); await user.keyboard('{Escape}');
  const use = screen.getByRole('button', { name: i18n.t('start.where.use') });
  await user.click(use); await waitFor(() => expect(calls.register).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(use).not.toBeDisabled());
  calls.directories.mockImplementation((_id, query) => query.read_kind === 'inspect'
    ? Promise.reject(new Error('The old path changed after acceptance'))
    : Promise.resolve({ read_kind: 'choices', entries: [], next_cursor: null }));
  await user.click(use);
  await waitFor(() => expect(calls.register).toHaveBeenCalledTimes(2));
  expect(calls.registerKey.mock.calls[0]?.[0]).toBe(calls.registerKey.mock.calls[1]?.[0]);
  expect(calls.directories.mock.calls.filter(([, query]) => query.read_kind === 'inspect')).toHaveLength(1);
  expect(screen.getByLabelText('Registered directory environment')).toHaveTextContent(detailEnvironment.id);
});
