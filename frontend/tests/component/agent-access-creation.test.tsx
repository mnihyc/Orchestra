/** The new holder's exact agent choice is visible and settled before any work is submitted. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as SettingsApi from '@/api/settings';
import { client } from '@/api/client';
import { NewThreadDialog } from '@/features/start/BindingDialog';
import { QuickStart } from '@/features/start/QuickStart';
import type { StartPlan } from '@/features/start/start';
import { placeOfNode } from '@/features/navigation/places';
import type { AgentAccessAvailability, AgentAccessChoice, ShareParts, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { stepOption } from '@/features/access/agentShareWords';
import { options } from '../unit/start-fixtures';
import { show } from './support';

const state = vi.hoisted(() => {
  const bounds: ShareParts = { documents: 'edit' };
  const preset: AgentAccessChoice = { where: 'topic', parts: { documents: 'edit' } };
  return { topic: null as Topic | null, start: vi.fn<(plan: StartPlan) => void>(),
  settingsReads: vi.fn(), pageReads: vi.fn(), optionsReads: vi.fn(), nodeReads: vi.fn(), bounds, omitOffer: false, preset };
});
function servedOffer(): AgentAccessAvailability {
  return { preset: state.preset, choices: [
    { where: 'nothing', parts: {} }, { where: 'thread', parts: { chat: 'write', documents: 'manage' } },
    { where: 'topic', parts: state.bounds }, { where: 'project', parts: {} },
  ] };
}
vi.mock('@/api/start', async (load) => ({ ...await load<object>(), startOptionsQuery: () => ({ queryKey: ['agent-creation-options'], queryFn: () => {
  state.optionsReads(); return Promise.resolve(options(state.omitOffer ? {} : { agent_access: servedOffer() }));
} }) }));
vi.mock('@/features/start/start', async (load) => ({ ...await load<object>(), useStart: () => ({ start: state.start, pending: false, error: null, step: null, standing: { topic: null, environment: null } }) }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({
  queryKey: ['agent-creation-settings', id], queryFn: () => { state.settingsReads(id); return Promise.resolve([
    { key: 'threads.agent_access', value: state.preset, own: null, writable: false, locked_by: null, source: { scope: 'default', node: null }, parts: [] },
  ]); },
}) }));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(),
  permissionsQuery: { queryKey: ['agent-creation-catalogue'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  pageQuery: (target: { kind: 'topic' | 'project'; id: string }) => ({ queryKey: ['agent-creation-page', target.id], queryFn: () => {
    state.pageReads(target.id); const page = pageFor(target.kind);
    return Promise.resolve({ ...page, object: { ...page.object, id: target.id }, gives: { ...page.gives, agents: [], agent_parts: state.bounds } });
  } }),
}));
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(), providersQuery: () => ({ queryKey: ['agent-creation-providers'], queryFn: () => Promise.resolve([]) }), machinesQuery: { queryKey: ['agent-creation-machines'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<object>(), nodeQuery: (id: string) => ({ queryKey: ['agent-creation-node', id], queryFn: () => { state.nodeReads(id); return Promise.resolve(state.topic); } }) }));
vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => () => 'Reported harness' }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

function topic() { if (state.topic === null) throw new Error('The current topic is required'); return state.topic; }
beforeEach(() => {
  vi.clearAllMocks(); localStorage.clear(); sessionStorage.clear();
  const world = new World(); const reader = world.people.get('ada');
  if (reader === undefined) throw new Error('The ordinary fixture reader is required');
  const node = world.places.node(reader, ACME.fixLogin);
  if (node.kind !== 'topic') throw new Error('The fixture supplies a real topic');
  state.topic = node; state.bounds = { documents: 'edit' }; state.preset = { where: 'topic', parts: { documents: 'edit' } }; state.omitOffer = false;
  client.session.setState({ status: 'signed_in', userId: reader.me.id });
});
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows both audience and exact Documents-only agent access before opening advanced setup (${locale})`, async () => {
    await setLocale(locale);
    show(<NewThreadDialog parent={placeOfNode(topic())} open onClose={vi.fn()} />);
    const choice = await screen.findByRole('region', { name: i18n.t('agentAccess.title') });
    await waitFor(() => expect(choice).toHaveTextContent(i18n.t('agentAccess.where.topic')));
    expect(choice).toHaveTextContent(i18n.t('access.agents.area.documents'));
    expect(screen.getByRole('combobox', { name: i18n.t('access.general.label') })).toBeVisible();
    expect(screen.queryByRole('region', { name: i18n.t('start.options.title') })).not.toBeInTheDocument();
    expect(state.start).not.toHaveBeenCalled();
    expect(state.settingsReads).not.toHaveBeenCalled();
    expect(state.pageReads).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
    expect(state.start).toHaveBeenCalledOnce();
    expect(state.start.mock.calls[0]?.[0]).toMatchObject({ agentAccess: { where: 'topic', parts: { documents: 'edit' } }, generalAccess: 'defaults' });
  });

  it(`does not block an otherwise permitted QuickStart when the owner cannot give the preset (${locale})`, async () => {
    await setLocale(locale); state.bounds = {};
    show(<QuickStart node={topic()} />);
    const choice = await screen.findByRole('region', { name: i18n.t('agentAccess.title') });
    await waitFor(() => expect(state.optionsReads).toHaveBeenCalled());
    await waitFor(() => expect(choice).toHaveTextContent(i18n.t('agentAccess.where.nothing')));
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Start without lending rights{Enter}');
    expect(state.start).toHaveBeenCalledOnce();
    expect(state.start.mock.calls[0]?.[0]).toMatchObject({ agentAccess: { where: 'nothing', parts: {} }, message: 'Start without lending rights' });
    expect(state.settingsReads).not.toHaveBeenCalled();
    expect(state.pageReads).not.toHaveBeenCalled();
  });
}

it('does not mount creation readers or start work for a closed form', () => {
  show(<NewThreadDialog parent={placeOfNode(topic())} open={false} onClose={vi.fn()} />);
  expect(state.settingsReads).not.toHaveBeenCalled();
  expect(state.pageReads).not.toHaveBeenCalled();
  expect(state.optionsReads).not.toHaveBeenCalled();
  expect(state.nodeReads).not.toHaveBeenCalled();
  expect(state.start).not.toHaveBeenCalled();
});

it('uses served future-topic bounds when creating a project’s new topic and first thread', async () => {
  const parent = { ...placeOfNode(topic()), id: topic().parent_id ?? 'project', kind: 'project' as const };
  show(<NewThreadDialog parent={parent} open onClose={vi.fn()} />);
  const choice = await screen.findByRole('region', { name: i18n.t('agentAccess.title') });
  await waitFor(() => expect(choice).toHaveTextContent(i18n.t('agentAccess.where.topic')));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
  expect(state.start.mock.calls[0]?.[0]).toMatchObject({ place: { kind: 'project' }, agentAccess: { where: 'topic', parts: { documents: 'edit' } } });
  expect(state.pageReads).not.toHaveBeenCalled();
  expect(state.settingsReads).not.toHaveBeenCalled();
});

it.each(['en','zh-CN'] as const)('shows the effective reduced preset without inventing its policy source (%s)', async (locale) => {
  await setLocale(locale);
  state.bounds = { documents: 'read' };
  show(<NewThreadDialog parent={placeOfNode(topic())} open onClose={vi.fn()} />);
  await screen.findByText(i18n.t('startImplicit.reducedTo'));
  const reduction=document.querySelector('[data-agent-access-reduced]');
  expect(reduction).toHaveTextContent(i18n.t('startImplicit.byAvailableAccess'));
  expect(reduction).toHaveTextContent(i18n.t('access.agents.area.documents'));
  expect(reduction).toHaveTextContent(String(stepOption(i18n.t,'documents',1).name));
  expect(reduction).not.toHaveTextContent(String(stepOption(i18n.t,'documents',2).name));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
  expect(state.start.mock.calls[0]?.[0]).toMatchObject({ agentAccess: { where: 'topic', parts: { documents: 'read' } } });
});

it('keeps a legacy reply without availability explicit and does not invent future-owner rights', async () => {
  state.omitOffer = true;
  show(<NewThreadDialog parent={placeOfNode(topic())} open onClose={vi.fn()} />);
  expect(await screen.findByText(i18n.t('agentAccess.presetUnavailable'))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
  expect(state.start.mock.calls[0]?.[0]).toMatchObject({ agentAccess: { where: 'nothing', parts: {} } });
  expect(state.pageReads).not.toHaveBeenCalled();
  expect(state.settingsReads).not.toHaveBeenCalled();
});
