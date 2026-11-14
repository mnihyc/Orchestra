/** Enter starts with a valid setup; core choices and More remain separate from optional input. */
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { ApiError, client } from '@/api/client';
import type * as AccessApi from '@/api/access';
import type { StartOptions } from '@/api/start';
import { NewThreadDialog } from '@/features/start/BindingDialog';
import { QuickStart } from '@/features/start/QuickStart';
import { rememberSetup } from '@/features/start/lastSetup';
import { NO_PICKS } from '@/features/start/choices';
import type { StartPlan } from '@/features/start/start';
import type { Approval, CreateThread, Project, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { agent, arrangement, environment as environmentRow, host, options, provider } from '../unit/start-fixtures';
import { BEA, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const state = vi.hoisted(() => ({ answer: null as StartOptions | null, freshAnswer: null as StartOptions | null, existingAnswer: null as StartOptions | null, refusal: null as Error | null, refusedEnvironment: null as string | null, askFor: null as string | null, askClosed: false, approvals: [] as Approval[], request: vi.fn<typeof AccessApi.requestAccess>(), start: vi.fn<(plan: StartPlan) => void>(), reads: vi.fn<(environment: string | null) => void>() }));
function answerForEnvironment(environment: string | null) {
  return environment === 'new' ? state.freshAnswer ?? state.answer : environment === null ? state.answer : state.existingAnswer ?? state.answer;
}
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), startOptionsQuery: (_id: string, environment: string | null) => ({
  queryKey: ['start-flow', environment], initialData: environment === state.askFor && environment !== null ? undefined : answerForEnvironment(environment),
  queryFn: () => {
    state.reads(environment);
    if (environment !== null && environment === state.askFor) {
      const row = state.answer?.environments.find((entry) => entry.environment.id === environment);
      const owner = row?.environment.owner ?? null;
      return Promise.reject(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Access denied', detail: 'Use is needed', request_id: 'use-required', requestable: true,
        missing: [{ object: { kind: 'environment', id: environment, name: row?.environment.name ?? null, owner }, permissions: ['environment.use'], ask: owner === null || state.askClosed ? [] : [owner], ask_more: 0, closed: state.askClosed ? 'locked' : null, options: null, override: false }] }, null));
    }
    return state.refusedEnvironment !== null && environment === state.refusedEnvironment ? Promise.reject(new Error('The selected environment is unavailable')) : state.refusal === null ? Promise.resolve(answerForEnvironment(environment)) : Promise.reject(state.refusal);
  },
}) }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(),
  requestAccess: (...args: Parameters<typeof AccessApi.requestAccess>) => state.request(...args),
  accessRequestQuery: (batch: string) => ({ queryKey: ['start-access', batch], queryFn: () => Promise.resolve({ batch_id: batch, approvals: state.approvals, complete: false }) }),
}));
vi.mock('@/features/start/start', async (original) => ({ ...await original<object>(), useStart: () => ({ start: state.start, pending: false, error: null, step: null, standing: { topic: null, environment: null } }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), providersQuery: () => ({ queryKey: ['start-flow-providers'], queryFn: () => Promise.resolve([]) }), machinesQuery: { queryKey: ['start-flow-machines'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['start-flow-node', id], queryFn: () => Promise.resolve({ org_id: 'org', access: { permissions: [] } }) }) }));
vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => () => 'Reported harness' }));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/access/NewPage', async (original) => ({ ...await original<object>(), NewPage: () => <div data-access-options /> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const topic = { id: 'topic', name: 'login', kind: 'topic', path: [], parent_id: null, archived: false, folder: false, status: 'open', access: { permissions: ['topic.interact', 'topic.start_threads'] } } as unknown as Topic;
const parent = { id: topic.id, name: topic.name, kind: 'topic' as const, parentId: null, archived: false, folder: false, status: 'open' as const };
// These cases exercise current-topic placement and access; More categories have their own tests.
const environment: typeof environmentRow = (id, args) => ({ ...environmentRow(id, args), used: 'topic' });
beforeEach(() => { vi.clearAllMocks(); localStorage.clear(); sessionStorage.clear(); state.refusal = null; state.refusedEnvironment = null; state.askFor = null; state.askClosed = false; state.approvals = []; state.freshAnswer = null; state.existingAnswer = null; state.answer = options(); state.request.mockResolvedValue({ batch_id: 'start-request', approvals: [] }); client.session.setState({ status: 'signed_in', userId: 'starter' }); });
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

function submittedPlan(): StartPlan {
  expect(state.start).toHaveBeenCalledOnce();
  const call = state.start.mock.calls[0];
  if (call === undefined) throw new Error('The start plan was not submitted');
  return call[0];
}

async function showStarter(ui: ReactNode) {
  const root = createRootRoute({ component: () => ui });
  const providerRoute = createRoute({ getParentRoute: () => root, path: '/resources/providers/$id',
    validateSearch: (search: Record<string, unknown>) => ({ tab: typeof search.tab === 'string' ? search.tab : undefined }) });
  const router = createRouter({ routeTree: root.addChildren([providerRoute]), history: createMemoryHistory({ initialEntries: ['/'] }) });
  await router.load();
  return show(<RouterProvider router={router} />);
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`preselects an existing environment and judges that location when no host can make New (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ environment_default: 'fresh', hosts: [], providers: [provider('p1', { agents: [agent('a1', { reason: 'no_host' })] })] });
    state.existingAnswer = options({ environment_default: 'fresh', hosts: [] });
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
    const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
    await waitFor(() => expect(where).toHaveDisplayValue('Env e1'));
    await waitFor(() => expect(state.reads).toHaveBeenCalledWith('e1'));
    expect(state.reads).not.toHaveBeenCalledWith('new');
    const selectedAgent = screen.getByRole('combobox', { name: i18n.t('start.agent.title') });
    await waitFor(() => expect(selectedAgent).toHaveDisplayValue('Agent a1'));
    await userEvent.click(selectedAgent);
    const choice = screen.getByRole('option', { name: 'Agent a1' });
    await waitFor(() => expect(choice).not.toHaveAttribute('aria-disabled', 'true'));
    await userEvent.keyboard('{Escape}');
    expect(selectedAgent).toHaveDisplayValue('Agent a1');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
    await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
    expect(submittedPlan()).toMatchObject({ resolved: { fresh: false, environment: { environment: { id: 'e1' } }, agent: { id: 'a1' } } });
  });
  it(`shows a reported program error and its native details instead of waiting forever (${locale})`, async () => {
    await setLocale(locale);
    const failed = { ...agent('a1', { reason: 'harness_error' }), program_report: { status: 'error' as const, detail: 'The selected executable was not found.' } };
    state.answer = options({ providers: [provider('p1', { agents: [failed, agent('a2', { reason: 'harness_unreported' })] })] });
    await showStarter(<QuickStart node={topic} />);
    expect(screen.getByText(i18n.t('startPreflight.agentUnavailable', { agent: 'Agent a1', why: i18n.t('start.agent.programFailed') }))).toBeVisible();
    expect(screen.queryByText(i18n.t('start.missing.agentUnreported', { agent: 'Agent a1' }))).toBeNull();
    await userEvent.click(screen.getByText(i18n.t('start.agent.programDetails', { agent: 'Agent a1' }), { selector: 'summary' }));
    expect(screen.getByText('The selected executable was not found.')).toBeVisible();
    expect(screen.getByRole('link', { name: i18n.t('start.agent.openProgramReports') })).toHaveAttribute('href', '/resources/providers/p1?tab=settings');
    expect(screen.getByRole('button', { name: i18n.t('start.missing.choose') })).toBeVisible();
    expect(state.start).not.toHaveBeenCalled();
  });

  it(`names the reported error on the disabled agent choice (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ providers: [provider('p1', { agents: [{ ...agent('a1', { reason: 'harness_error' }), program_report: { status: 'error', detail: 'The selected executable was not found.' } }] })] });
    await showStarter(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole('combobox', { name: i18n.t('start.agent.title') }));
    const profile = screen.getByRole('option', { name: 'Agent a1' });
    expect(profile).toHaveAttribute('aria-disabled', 'true');
    expect(profile).toHaveAccessibleDescription(i18n.t('start.agent.programFailed'));
    await userEvent.keyboard('{Escape}');
    expect(screen.getByRole('button', { name: i18n.t('start.create') })).toBeDisabled();
    expect(state.start).not.toHaveBeenCalled();
  });

  it(`keeps one shared configured-default mark when it also matches the remembered setup (${locale})`,async()=>{
    await setLocale(locale);
    rememberSetup('starter',{place:'topic',picks:{...NO_PICKS,provider:'p1',agent:'a1',model:'m1',environment:'e2'}});
    show(<QuickStart node={topic} />);
    const setup=document.querySelector<HTMLElement>('[data-runs-on]');
    if(setup===null)throw new Error('The selected setup is required');
    const choices=within(setup).getByRole('list');
    expect(within(setup).getAllByText(i18n.t('start.topicDefault'))).toHaveLength(1);
    for(const field of ['provider','agent','model']) {
      const value=document.querySelector<HTMLElement>(`[data-start-value="${field}"]`);
      if(value===null)throw new Error('The selected field is required');
      expect(choices).toContainElement(value);
      expect(value).toHaveAttribute('role','listitem');
      expect(value).not.toHaveTextContent(i18n.t('start.topicDefault'));
      expect(value).not.toHaveTextContent(i18n.t('startImplicit.origin.last_used'));
    }
  });

  it(`does not invent an environment-viewer default when the server did not report it (${locale})`,async()=>{
    await setLocale(locale);
    state.answer=options({viewer_default:null});
    const quick=show(<QuickStart node={topic} />);
    await userEvent.type(screen.getByRole('textbox',{name:i18n.t('start.quick.label.topic')}),'Wait for the default{Enter}');
    expect(state.start).not.toHaveBeenCalled();
    quick.unmount();
    show(<NewThreadDialog parent={parent} open onClose={()=>undefined} />);
    const create=await screen.findByRole('button',{name:i18n.t('start.create')});
    expect(create).toBeDisabled();
    await userEvent.click(create);
    expect(state.start).not.toHaveBeenCalled();
    expect(within(await screen.findByRole('dialog')).getByText(i18n.t('startImplicit.checkingViewerDefault'))).toBeVisible();
  });
  for (const value of [false,true]) it(`uses the same reported viewers default in both start entries (${locale}, ${value})`, async () => {
    await setLocale(locale);
    state.answer = options({viewer_default:{value,source:{scope:'default',node:null}}});
    const quick = show(<QuickStart node={topic} />);
    expect(screen.getByRole('checkbox',{name:i18n.t('access.general.letIn')})).toHaveAttribute('data-state',value?'checked':'unchecked');
    await userEvent.type(screen.getByRole('textbox',{name:i18n.t('start.quick.label.topic')}),'Start{Enter}');
    expect(submittedPlan().letIntoEnvironment).toBe(value);
    quick.unmount();
    state.start.mockClear();
    show(<NewThreadDialog parent={parent} open onClose={()=>undefined} />);
    await userEvent.click(await screen.findByRole('button',{name:i18n.t('start.create')}));
    expect(submittedPlan().letIntoEnvironment).toBe(value);
  });
  for (const value of [false, true]) it(`retains a prepared birth's explicit viewer choice against a different current default (${locale}, ${value})`, async () => {
    await setLocale(locale);
    state.answer = options({ viewer_default: { value: !value, source: { scope: 'default', node: null } } });
    const choose = vi.fn<(config: CreateThread) => void>();
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} prepare={{ initial: { let_into_environment: value }, onChoose: choose }} />);
    const form = await screen.findByRole('dialog');
    const useSetup = within(form).getByRole('button', { name: i18n.t('sendLater.useSetup') });
    await waitFor(() => expect(useSetup).toBeEnabled());
    await userEvent.click(useSetup);
    await waitFor(() => expect(choose).toHaveBeenCalledOnce());
    expect(choose.mock.calls[0]?.[0]).toHaveProperty('let_into_environment', value);
    expect(state.start).not.toHaveBeenCalled();
  });
  it(`marks ranked values once and clears the mark after an explicit provider choice (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false }, environments: [environment('ranked')], can_create: { allowed: false, missing: [] } });
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
    const dialog = await screen.findByRole('dialog');
    const headings = within(dialog).getAllByRole('heading');
    for (const label of ['start.provider.title', 'start.agent.title', 'start.model.model', 'start.where.title'] as const) {
      const heading = headings.find((row) => row.textContent.startsWith(i18n.t(label)));
      expect(heading).toBeDefined();
      expect(within(heading!).getAllByText(i18n.t('startImplicit.origin.suggested'))).toHaveLength(1);
    }
    await userEvent.click(within(dialog).getByRole('combobox', { name: i18n.t('start.provider.title') }));
    await userEvent.click(screen.getByRole('option', { name: /Provider p2/ }));
    const providerHeading = within(dialog).getAllByRole('heading').find((row) => row.textContent.startsWith(i18n.t('start.provider.title')));
    expect(providerHeading).not.toHaveTextContent(i18n.t('startImplicit.origin.suggested'));
  });

  it(`starts on Enter with defaults, preserving Shift+Enter and writing no place defaults (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<QuickStart node={topic} />);
    const box = screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') });
    await user.type(box, 'First');
    await user.keyboard('{Shift>}{Enter}{/Shift}second');
    expect(state.start).not.toHaveBeenCalled();
    expect(box).toHaveValue('First\nsecond');
    await user.keyboard('{Enter}');
    expect(submittedPlan()).toMatchObject({ message: 'First\nsecond', resolved: { provider: { id: 'p1' }, agent: { id: 'a1' }, model: { model_name: 'm1' } } });
  });

  it(`uses the person's last usable setup when defaults are absent, with a current-place location (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false } });
    rememberSetup('starter', { place: 'elsewhere', picks: { ...NO_PICKS, provider: 'p2', agent: 'a2', model: 'm2', environment: 'foreign-directory', arrangement: 'key_separated' } });
    show(<QuickStart node={topic} />);
    expect(screen.queryByText(i18n.t('start.missing.provider', { place: topic.name }))).toBeNull();
    expect(screen.getAllByText(i18n.t('startImplicit.origin.last_used'), { exact: true })).toHaveLength(1);
    for (const field of ['provider','agent','model']) {
      expect(document.querySelector(`[data-start-value="${field}"]`)).not.toHaveTextContent(i18n.t('startImplicit.origin.last_used'));
    }
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Continue{Enter}');
    expect(submittedPlan()).toMatchObject({ resolved: { provider: { id: 'p2' }, agent: { id: 'a2' }, model: { model_name: 'm2' }, environment: { environment: { id: 'e1' } } } });
  });

  it(`asks only provider, agent, model and where, retaining optional input and permission choices (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ environments: [{ ...environment('e1', { isDefault: true }), directory: '~/work' }] });
    const user = userEvent.setup();
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} draft={{ picks: NO_PICKS, message: 'Keep this message' }} />);
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getAllByRole('combobox')).toHaveLength(4);
    expect(within(dialog).queryByRole('combobox', { name: i18n.t('start.where.moreTitle') })).toBeNull();
    expect(within(dialog).getByRole('radiogroup', { name: i18n.t('start.where.choice') })).toBeVisible();
    expect(within(dialog).queryByRole('textbox')).toBeNull();
    const more = within(dialog).getByRole('button', { name: i18n.t('start.options.title') });
    expect(more).toHaveAttribute('aria-expanded', 'false');
    await user.click(more);
    expect(within(dialog).getByRole('textbox', { name: i18n.t('start.message.title') })).toHaveValue('Keep this message');
    await user.type(within(dialog).getByRole('textbox', { name: new RegExp(`^${i18n.t('start.options.name')}`) }), 'Named work');
    expect(within(dialog).getByRole('radiogroup', { name: i18n.t('start.arrangement.label') })).toBeVisible();
    await user.click(within(dialog).getByRole('radio', { name: i18n.t('choiceCatalog.arrangement.key_separated.name') }));
    await user.click(more);
    expect(within(dialog).queryByRole('textbox')).toBeNull();
    const summary = dialog.querySelector('[data-start-options-summary]');
    expect(summary).toHaveTextContent(i18n.t('startImplicit.effort', { value: i18n.t('choiceCatalog.effort.low.name') }));
    expect(summary).toHaveTextContent(i18n.t('choiceCatalog.arrangement.key_separated.name'));
    expect(summary).toHaveTextContent('~/work');
    expect(summary).toHaveTextContent(i18n.t('startImplicit.name', { value: 'Named work' }));
    expect(summary).toHaveTextContent(i18n.t('startImplicit.message', { value: 'Keep this message' }));
    await user.click(within(dialog).getByRole('button', { name: i18n.t('start.create') }));
    await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
    expect(submittedPlan()).toMatchObject({ message: 'Keep this message', generalAccess: 'defaults', resolved: { arrangement: 'key_separated' } });
  });

  it(`disables a profile that cannot run in the selected environment and explains it inside the picker (${locale})`, async () => {
    await setLocale(locale);
    state.answer = options({ providers: [provider('p1', { agents: [agent('a1', { arrangements: [{ ...arrangement('separated', 1, false, 'executor_missing'), detail: 'No compatible executor is installed.' }] })] })] });
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole('combobox', { name: i18n.t('start.agent.title') }));
    const profile = screen.getByRole('option', { name: 'Agent a1' });
    expect(profile).toHaveAttribute('aria-disabled', 'true');
    expect(profile).toHaveAccessibleDescription(/No compatible executor is installed/);
    await userEvent.click(profile);
    await userEvent.keyboard('{Escape}');
    const create = screen.getByRole('button', { name: i18n.t('start.create') });
    expect(create).toBeDisabled();
    await userEvent.click(create);
    expect(state.start).not.toHaveBeenCalled();
  });

  it(`judges an explicit New against its template instead of the topic's existing default (${locale})`, async () => {
    await setLocale(locale);
    const closed = { ...arrangement('separated', 1, false, 'executor_missing'), detail: null };
    state.freshAnswer = options({ providers: [provider('p1', { agents: [agent('a1', { reason: 'executor_missing', arrangements: [closed] })] })] });
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole('radio', { name: i18n.t('start.where.createNew') }));
    await waitFor(() => expect(state.reads).toHaveBeenCalledWith('new'));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('start.agent.title') }));
    const profile = screen.getByRole('option', { name: 'Agent a1' });
    // The reason is the answer about the template: the choice says it once the answer has been applied.
    await waitFor(() => {
      expect(profile).toHaveAttribute('aria-disabled', 'true');
      expect(profile).toHaveAccessibleDescription(new RegExp(i18n.t('start.reason.executor_missing')));
    });
    await userEvent.click(profile);
    await userEvent.keyboard('{Escape}');
    expect(screen.getByRole('button', { name: i18n.t('start.create') })).toBeDisabled();
    expect(state.start).not.toHaveBeenCalled();
  });

  it(`does not advertise or submit a default profile that cannot run on the chosen template (${locale})`, async () => {
    await setLocale(locale);
    const closed = { ...arrangement('separated', 1, false, 'executor_missing'), detail: null };
    state.answer = options({ environment_default: 'fresh', providers: [provider('p1', { agents: [agent('a1', { reason: 'executor_missing', arrangements: [closed] })] })] });
    show(<QuickStart node={topic} />);
    expect(document.querySelector('[data-runs-on]')).toBeNull();
    expect(screen.getByText(i18n.t('startPreflight.agentUnavailable', { agent: 'Agent a1', why: i18n.t('start.reason.executor_missing') }))).toBeVisible();
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Keep this request{Enter}');
    expect(state.start).not.toHaveBeenCalled();
    expect(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') })).toHaveValue('Keep this request');
  });
}

it('does not reuse another account’s setup', async () => {
  state.answer = options({ defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false } });
  rememberSetup('someone-else', { place: topic.id, picks: { ...NO_PICKS, provider: 'p2', agent: 'a2', model: 'm2' } });
  show(<QuickStart node={topic} />);
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Start{Enter}');
  expect(submittedPlan()).toMatchObject({ resolved: { provider: { id: 'p1' } } });
});

it.each(['topic', 'project'] as const)('offers the starter for the creation permission alone on a %s', async (kind) => {
  const right = kind === 'topic' ? 'topic.start_threads' : 'project.create_topics';
  const node = { ...topic, kind, access: { permissions: [`${kind}.view`, right] } } as unknown as Topic | Project;
  show(<QuickStart node={node} />);
  const box = screen.getByRole('textbox', { name: i18n.t(`start.quick.label.${kind}`) });
  await userEvent.type(box, 'Start work{Enter}');
  expect(submittedPlan()).toMatchObject({ message: 'Start work', place: { kind } });
});

for (const locale of ['en', 'zh-CN'] as const) it(`never offers a failed Main as the topic default (${locale})`, async () => {
  await setLocale(locale);
  const failed = environment('failed', { isDefault: true });
  failed.environment.name = 'Main'; failed.environment.status = 'failed';
  state.answer = options({ environments: [failed] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await userEvent.click(await screen.findByRole('radio', { name: i18n.t('start.where.useExisting') }));
  const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
  expect(where).not.toHaveTextContent('Main');
  await userEvent.click(where);
  const main = screen.getByRole('option', { name: 'Main' });
  expect(main).toHaveAttribute('aria-disabled', 'true');
  expect(main).toHaveAccessibleDescription(locale === 'en' ? /Unavailable after a failed start/ : /上次启动失败/);
  expect(screen.getByText(i18n.t('startRecovery.failedChoice'))).toBeVisible();
  expect(main).not.toHaveAccessibleDescription(new RegExp(i18n.t('start.topicDefault')));
  await userEvent.click(main);
  expect(state.start).not.toHaveBeenCalled();
  await userEvent.keyboard('{Escape}');
  expect(where).not.toHaveTextContent('Main');
});

for (const locale of ['en', 'zh-CN'] as const) it(`groups environments by home while preserving Use and request access (${locale})`, async () => {
  await setLocale(locale);
  const askable = { ...environment('view', { usable: false }), reason: 'environment_use_required' };
  const blocked = environment('blocked', { usable: false });
  state.answer = options({ environments: [askable, blocked, environment('use', { isDefault: true })].map((row) => ({ ...row, home: TOPIC, home_path: [TOPIC] })) });
  state.askFor = 'view';
  const user = userEvent.setup();
  await showStarter(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
  const choices = screen.getByRole('dialog', { name: i18n.t('start.where.title') });
  const ownedOptions = () => within(choices).queryAllByRole('listbox').flatMap((list) => within(list).queryAllByRole('option'));
  // Group actions belong to the picker dialog; each permission list owns its actual options.
  // One Home folds all three permission sections together.
  expect(ownedOptions()).toHaveLength(3);
  await user.click(within(choices).getByRole('button', { name: i18n.t('resourceGroups.collapse', { name: TOPIC.name ?? '' }) }));
  expect(ownedOptions()).toHaveLength(0);
  await user.click(within(choices).getByRole('button', { name: i18n.t('resourceGroups.expand', { name: TOPIC.name ?? '' }) }));
  expect(ownedOptions()).toHaveLength(3);
  const usable = within(choices).getByRole('group', { name: i18n.t('start.where.canUse') });
  const askableChoices = within(choices).getByRole('group', { name: i18n.t('start.where.askAccess') });
  const blockedChoices = within(choices).getByRole('group', { name: TOPIC.name ?? '' });
  expect(within(within(usable).getByRole('listbox')).getByRole('option', { name: 'Env use' })).not.toHaveAttribute('aria-disabled', 'true');
  expect(within(within(askableChoices).getByRole('listbox')).getByRole('option', { name: 'Env view' })).not.toHaveAttribute('aria-disabled', 'true');
  expect(within(within(blockedChoices).getByRole('listbox')).getByRole('option', { name: 'Env blocked' })).toHaveAttribute('aria-disabled', 'true');
  expect(within(choices).queryByRole('option', { name: i18n.t('choiceCatalog.environmentDestination.new.name') })).toBeNull();
  await user.click(within(within(askableChoices).getByRole('listbox')).getByRole('option', { name: 'Env view' }));
  expect(await screen.findByRole('textbox', { name: i18n.t('access.request.reason') })).toBeVisible();
  expect(state.start).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) for (const access of ['usable', 'askable'] as const) it(`omits environment group labels when only ${access} choices exist (${locale})`, async () => {
  await setLocale(locale);
  const row = access === 'usable' ? environment('single') : { ...environment('single', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [row] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await userEvent.click(await screen.findByRole('radio', { name: i18n.t('start.where.useExisting') }));
  await userEvent.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
  expect(screen.getByRole('option', { name: 'Env single' })).toBeEnabled();
  expect(screen.queryByRole('group', { name: i18n.t('start.where.canUse') })).toBeNull();
  expect(screen.queryByRole('group', { name: i18n.t('start.where.askAccess') })).toBeNull();
  if (access === 'askable') expect(screen.getByText(i18n.t('start.where.askAccess'))).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`can leave a location whose current options are refused without losing the first message (${locale})`, async () => {
  await setLocale(locale);
  state.refusedEnvironment = 'e2';
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} draft={{ picks: { ...NO_PICKS, environment: 'e2' }, message: 'Keep this request' }} />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('startRecovery.elsewhere') }));
  expect(await screen.findByRole('combobox', { name: i18n.t('start.where.title') })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('start.create') }));
  await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
  expect(submittedPlan()).toMatchObject({ message: 'Keep this request', resolved: { environment: { environment: { id: 'e1' } } } });
});

it.each(['en', 'zh-CN'] as const)('does not start in the previous environment while asking for another (%s)', async (locale) => {
  await setLocale(locale);
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [environment('e1', { isDefault: true }), requested] });
  state.askFor = 'e2';
  const pending: Extract<Approval, { kind: 'access' }> = {
    id: 'approval', batch_id: 'start-request', kind: 'access', state: 'pending', title: 'Use environment',
    object: { kind: 'environment', id: 'e2', name: 'Env e2' }, node: TOPIC, requester: { user: BEA },
    request: { permissions: ['environment.use'], reason: 'Please grant use.', holder: { kind: 'user', user: BEA }, options: null, for_thread: null },
    required_permission: 'access.manage', can_decide: false, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
    decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
  };
  state.approvals = [pending];
  state.request.mockResolvedValue({ batch_id: 'start-request', approvals: [pending] });
  sessionStorage.setItem(`orch.access.request.v2.${JSON.stringify(['starter', 'e2', null, false])}`, 'another-permission');
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} draft={{ picks: NO_PICKS, message: 'Keep this request' }} />);
  const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
  await user.click(where);
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  expect(where).toHaveValue('Env e2');
  const create = screen.getByRole('button', { name: i18n.t('start.create') });
  expect(create).toBeDisabled();
  const reason = await screen.findByRole('textbox', { name: i18n.t('access.request.reason') });
  expect(reason).toBeVisible();
  fireEvent.change(reason, { target: { value: 'Please grant use.' } });
  fireEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(state.request).toHaveBeenCalledOnce());
  expect(state.request.mock.calls[0]?.[0]).toEqual({ items: [{ object: { kind: 'environment', id: 'e2' }, permissions: ['environment.use'] }], reason: 'Please grant use.' });
  await waitFor(() => expect(sessionStorage.getItem(`orch.access.request.v2.${JSON.stringify(['starter', 'e2.environment.use', null, false])}`)).toBe('start-request'));
  expect(screen.getByRole('button', { name: 'Ada Lovelace' })).toBeVisible();
  fireEvent.click(create);
  expect(state.start).not.toHaveBeenCalled();
});

it('keeps the first message when leaving an access request for a usable environment', async () => {
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [environment('e1', { isDefault: true }), requested] });
  state.askFor = 'e2';
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} draft={{ picks: NO_PICKS, message: 'Keep this request' }} />);
  const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
  await user.click(where);
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  await user.click(where);
  await user.click(screen.getByRole('option', { name: 'Env e1' }));
  const create = screen.getByRole('button', { name: i18n.t('start.create') });
  await waitFor(() => expect(create).toBeEnabled());
  fireEvent.click(create);
  await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
  expect(submittedPlan()).toMatchObject({ message: 'Keep this request', resolved: { environment: { environment: { id: 'e1' } } } });
});

// This full grant-and-reselect workflow crosses several rendered pickers; each read assertion keeps its own deadline.
it.each(['en','zh-CN'] as const)('selects the requested environment only after a current read confirms Use (%s)', async (locale) => {
  await setLocale(locale);
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [environment('e1', { isDefault: true }), requested] });
  state.askFor = 'e2';
  state.request.mockImplementation(() => {
    state.askFor = null;
    state.answer = options({ environments: [environment('e1', { isDefault: true }), environment('e2')] });
    return Promise.resolve({ batch_id: 'start-request', approvals: [] });
  });
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
  await user.click(where);
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  const reason = await screen.findByRole('textbox', { name: i18n.t('access.request.reason') });
  fireEvent.change(reason, { target: { value: 'Please grant use.' } });
  fireEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  const create = screen.getByRole('button', { name: i18n.t('start.create') });
  await waitFor(() => expect(create).toBeEnabled());
  expect(where).toHaveValue('Env e2');
  expect(screen.getByText(i18n.t('startImplicit.grantedSelected'))).toBeVisible();
  await user.click(where);
  await user.click(screen.getByRole('option',{name:'Env e1'}));
  expect(screen.queryByText(i18n.t('startImplicit.grantedSelected'))).toBeNull();
  await user.click(where);
  await user.click(screen.getByRole('option',{name:'Env e2'}));
  expect(screen.queryByText(i18n.t('startImplicit.grantedSelected'))).toBeNull();
  await waitFor(()=>expect(create).toBeEnabled());
  fireEvent.click(create);
  await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
  expect(submittedPlan()).toMatchObject({ resolved: { environment: { environment: { id: 'e2' } } } });
}, 10_000);

it('shows a locked environment refusal without offering an impossible access request', async () => {
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [environment('e1', { isDefault: true }), requested] });
  state.askFor = 'e2';
  state.askClosed = true;
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  expect(await screen.findByText(i18n.t('access.missing.closed.locked'))).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('access.request.send') })).toBeNull();
  expect(screen.getByRole('button', { name: i18n.t('start.create') })).toBeDisabled();
  expect(state.request).not.toHaveBeenCalled();
});

it('does not show new-environment choices or arrangements for an access request', async () => {
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environment_default: 'fresh', environments: [requested] });
  state.askFor = 'e2';
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  expect(await screen.findByRole('switch', { name: i18n.t('temporary.name') })).toBeVisible();
  await user.click(screen.getByRole('radio', { name: i18n.t('start.where.useExisting') }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  expect(screen.queryByRole('switch', { name: i18n.t('temporary.name') })).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('start.options.title') }));
  expect(screen.queryByRole('radiogroup', { name: i18n.t('start.arrangement.label') })).toBeNull();
  expect(document.querySelector('[data-directory-sign]')).toBeNull();
});

it('does not show the previous environment’s default directory for an access request', async () => {
  const previous = { ...environment('e1', { isDefault: true }), directory: 'old-directory' };
  const requested = { ...environment('e2', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [previous, requested] });
  state.askFor = 'e2';
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  fireEvent.click(await screen.findByRole('button', { name: i18n.t('start.options.title') }));
  expect(screen.getByText(i18n.t('start.where.directoryIs', { directory: 'old-directory' }))).toBeVisible();
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(screen.getByRole('option', { name: 'Env e2' }));
  expect(screen.queryByText(i18n.t('start.where.directoryIs', { directory: 'old-directory' }))).toBeNull();
  expect(screen.getByText(i18n.t('start.where.directoryHint'))).toBeVisible();
});

it('does not submit cached options after the current read is refused', async () => {
  state.refusal = new Error('Options are no longer allowed');
  show(<QuickStart node={topic} />);
  expect(await screen.findByRole('alert')).toBeVisible();
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Keep this draft{Enter}');
  expect(state.start).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') })).toHaveValue('Keep this draft');
});

it('reviews a remembered same-place directory against its own native options before reuse', async () => {
  state.answer = options({ environments: [environment('e2')], can_create: { allowed: false, missing: [] } });
  rememberSetup('starter', { place: topic.id, picks: { ...NO_PICKS, provider: 'p1', agent: 'a1', model: 'm1', environment: 'e2' } });
  show(<QuickStart node={topic} />);
  await waitFor(() => expect(state.reads).toHaveBeenCalledWith('e2'));
  // Starting waits for the answers about the place: Enter is pressed once the button says they are in.
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Same location');
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('start.quick.send') })).not.toHaveAttribute('aria-disabled', 'true'));
  await userEvent.keyboard('{Enter}');
  expect(submittedPlan()).toMatchObject({ resolved: { fresh: false, environment: { environment: { id: 'e2' } } } });
});

it.each(['en','zh-CN'] as const)('offers Temporary only for a new environment, with its consent in the start plan (%s)', async (locale) => {
  await setLocale(locale);
  state.answer=options({ environment_default:'fresh' });
  const user=userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  const temporary=await screen.findByRole('switch',{name:i18n.t('temporary.name')});
  expect(temporary).not.toBeChecked();
  await user.click(temporary);
  expect(screen.queryByText(i18n.t('temporary.consent'))).not.toBeInTheDocument();
  // Leave for another dialog control before re-entering; the modal's backdrop is inert.
  await user.hover(screen.getByRole('button',{name:i18n.t('common.cancel')}));
  await user.hover(temporary);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t('temporary.consent'));
  expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
  await user.click(screen.getByRole('button',{name:i18n.t('start.create')}));
  await waitFor(()=>expect(state.start).toHaveBeenCalledOnce());
  expect(submittedPlan()).toMatchObject({temporary:true,resolved:{fresh:true}});
});

it.each(['en', 'zh-CN'] as const)('asks nothing about Git in a place without a repository, and offers a topic repository as an optional clone (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  // A plain directory is normal: Options name no repository and no Git.
  const plain = show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  let dialog = await screen.findByRole('dialog');
  await user.click(within(dialog).getByRole('button', { name: i18n.t('start.options.title') }));
  expect(within(dialog).queryByRole('checkbox', { name: i18n.t('source.prepare.choice') })).toBeNull();
  expect(dialog).not.toHaveTextContent(/Git|repository|仓库/i);
  plain.unmount();

  state.answer = options({ source: { repository_url: 'https://example.test/team/repo.git', branch: null, secret_name: null, from: null } });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  dialog = await screen.findByRole('dialog');
  await user.click(within(dialog).getByRole('button', { name: i18n.t('start.options.title') }));
  const clone = within(dialog).getByRole('checkbox', { name: i18n.t('source.prepare.choice') });
  expect(clone).toBeChecked();
  expect(within(dialog).getByText('https://example.test/team/repo.git')).toBeVisible();
  await user.click(clone);
  await user.click(within(dialog).getByRole('button', { name: i18n.t('start.create') }));
  await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
  expect(submittedPlan()).toMatchObject({ prepareSource: false });
});

it.each(['en', 'zh-CN'] as const)('says why a host cannot take a new environment and what to know of one that can (%s)', async (locale) => {
  await setLocale(locale);
  state.answer = options({ environment_default: 'fresh', hosts: [
    host('roomy', 'Roomy host', { automatic: true, warnings: ['no_room'] }),
    host('closed', 'Closed host', { usable: false, reasons: ['template_not_allowed'] }),
  ] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await screen.findByRole('radio', { name: i18n.t('start.where.createNew') });
  const list = document.querySelector<HTMLElement>('[data-start-hosts]');
  if (list === null) throw new Error('the new environment option lists the hosts it could be made on');
  const hosts = within(list);
  expect(hosts.getByText(i18n.t('change.where.hostReason.template_not_allowed'))).toBeVisible();
  expect(hosts.getByText(i18n.t('change.where.hostWarning.no_room'))).toBeVisible();
  expect(hosts.queryByText(i18n.t('start.where.hostClosed'))).toBeNull();
});
