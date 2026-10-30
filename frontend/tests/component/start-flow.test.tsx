/** Enter starts with a valid setup; only four choices are asked before Options opens. */
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { client } from '@/api/client';
import type { StartOptions } from '@/api/start';
import { NewThreadDialog } from '@/features/start/BindingDialog';
import { QuickStart } from '@/features/start/QuickStart';
import { rememberSetup } from '@/features/start/lastSetup';
import { NO_PICKS } from '@/features/start/choices';
import type { StartPlan } from '@/features/start/start';
import type { PlaceHost, Topic } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';

import { agent, arrangement, environment, options, provider } from '../unit/start-fixtures';
import { show } from './support';

const state = vi.hoisted(() => ({ answer: null as StartOptions | null, freshAnswer: null as StartOptions | null, refusal: null as Error | null, refusedEnvironment: null as string | null, start: vi.fn<(plan: StartPlan) => void>(), reads: vi.fn<(environment: string | null) => void>() }));
vi.mock('@/api/start', async (original) => ({ ...await original<object>(), startOptionsQuery: (_id: string, environment: string | null) => ({
  queryKey: ['start-flow', environment], initialData: environment === 'new' ? state.freshAnswer ?? state.answer : state.answer,
  queryFn: () => { state.reads(environment); return state.refusedEnvironment !== null && environment === state.refusedEnvironment ? Promise.reject(new Error('The selected environment is unavailable')) : state.refusal === null ? Promise.resolve(environment === 'new' ? state.freshAnswer ?? state.answer : state.answer) : Promise.reject(state.refusal); },
}) }));
vi.mock('@/features/start/start', async (original) => ({ ...await original<object>(), useStart: () => ({ start: state.start, pending: false, error: null, step: null, standing: { topic: null, environment: null } }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(), providersQuery: () => ({ queryKey: ['start-flow-providers'], queryFn: () => Promise.resolve([]) }), machinesQuery: { queryKey: ['start-flow-machines'], queryFn: () => Promise.resolve([]) } }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['start-flow-node', id], queryFn: () => Promise.resolve({ org_id: 'org', access: { permissions: [] } }) }) }));
vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => () => 'Reported harness' }));
vi.mock('@/features/resources/providers/ProviderUsageCard', () => ({ ProviderUsageCard: () => null }));
vi.mock('@/features/access/NewPage', () => ({ NewPage: () => <div data-access-options /> }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const topic = { id: 'topic', name: 'login', kind: 'topic', path: [], parent_id: null, archived: false, folder: false, status: 'open', access: { permissions: ['topic.interact'] } } as unknown as Topic;
const parent = { id: topic.id, name: topic.name, kind: 'topic' as const, parentId: null, archived: false, folder: false, status: 'open' as const };
beforeEach(() => { vi.clearAllMocks(); localStorage.clear(); state.refusal = null; state.refusedEnvironment = null; state.freshAnswer = null; state.answer = options(); client.session.setState({ status: 'signed_in', userId: 'starter' }); });
afterEach(async () => { act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

function submittedPlan(): StartPlan {
  expect(state.start).toHaveBeenCalledOnce();
  const call = state.start.mock.calls[0];
  if (call === undefined) throw new Error('The start plan was not submitted');
  return call[0];
}

for (const locale of ['en', 'zh-CN'] as const) {
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
    await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Continue{Enter}');
    expect(submittedPlan()).toMatchObject({ resolved: { provider: { id: 'p2' }, agent: { id: 'a2' }, model: { model_name: 'm2' }, environment: { environment: { id: 'e1' } } } });
  });

  it(`asks only provider, agent, model and where, retaining optional input and permission choices (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<NewThreadDialog parent={parent} open onClose={() => undefined} draft={{ picks: NO_PICKS, message: 'Keep this message' }} />);
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getAllByRole('combobox')).toHaveLength(4);
    expect(within(dialog).queryByRole('radiogroup')).toBeNull();
    expect(within(dialog).queryByRole('textbox')).toBeNull();
    const more = within(dialog).getByRole('button', { name: i18n.t('start.options.title') });
    expect(more).toHaveAttribute('aria-expanded', 'false');
    await user.click(more);
    expect(within(dialog).getByRole('textbox', { name: i18n.t('start.message.title') })).toHaveValue('Keep this message');
    expect(within(dialog).getByRole('radiogroup', { name: i18n.t('start.arrangement.label') })).toBeVisible();
    await user.click(within(dialog).getByRole('radio', { name: i18n.t('choiceCatalog.arrangement.key_separated.name') }));
    await user.click(more);
    expect(within(dialog).queryByRole('textbox')).toBeNull();
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
    await userEvent.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.environmentDestination.new.name') }));
    await waitFor(() => expect(state.reads).toHaveBeenCalledWith('new'));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('start.agent.title') }));
    const profile = screen.getByRole('option', { name: 'Agent a1' });
    expect(profile).toHaveAttribute('aria-disabled', 'true');
    expect(profile).toHaveAccessibleDescription(new RegExp(i18n.t('start.reason.executor_missing')));
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

for (const locale of ['en', 'zh-CN'] as const) it(`never offers a failed Main as the topic default (${locale})`, async () => {
  await setLocale(locale);
  const failed = environment('failed', { isDefault: true });
  failed.environment.name = 'Main'; failed.environment.status = 'failed';
  state.answer = options({ environments: [failed] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  const where = await screen.findByRole('combobox', { name: i18n.t('start.where.title') });
  expect(where).not.toHaveTextContent('Main');
  await userEvent.click(where);
  const main = screen.getByRole('option', { name: 'Main' });
  expect(main).toHaveAttribute('aria-disabled', 'true');
  expect(main).toHaveAccessibleDescription(locale === 'en' ? /Unavailable after a failed start/ : /上次启动失败/);
  expect(screen.getByText(i18n.t('startRecovery.failedChoice'))).toBeVisible();
  expect(main).not.toHaveAccessibleDescription(new RegExp(i18n.t('start.where.topicDefault')));
  await userEvent.click(main);
  expect(state.start).not.toHaveBeenCalled();
  await userEvent.keyboard('{Escape}');
  expect(where).not.toHaveTextContent('Main');
});

for (const locale of ['en', 'zh-CN'] as const) it(`groups usable environments apart from those that need a request (${locale})`, async () => {
  await setLocale(locale);
  const askable = { ...environment('view', { usable: false }), reason: 'environment_use_required' };
  const blocked = environment('blocked', { usable: false });
  state.answer = options({ environments: [askable, blocked, environment('use', { isDefault: true })] });
  const user = userEvent.setup();
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await user.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
  const canUse = screen.getByRole('group', { name: i18n.t('start.where.canUse') });
  const ask = screen.getByRole('group', { name: i18n.t('start.where.askAccess') });
  expect(within(canUse).getByRole('option', { name: 'Env use' })).toBeEnabled();
  expect(within(ask).getByRole('option', { name: 'Env view' })).toBeEnabled();
  expect(screen.getByRole('option', { name: 'Env blocked' })).toHaveAttribute('aria-disabled', 'true');
  expect(within(ask).queryByRole('option', { name: 'Env blocked' })).toBeNull();
  expect(within(ask).queryByRole('option', { name: i18n.t('choiceCatalog.environmentDestination.new.name') })).toBeNull();
  await user.click(within(ask).getByRole('option', { name: 'Env view' }));
  expect(await screen.findByRole('textbox', { name: i18n.t('access.request.reason') })).toBeVisible();
  expect(state.start).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) for (const access of ['usable', 'askable'] as const) it(`omits environment group labels when only ${access} choices exist (${locale})`, async () => {
  await setLocale(locale);
  const row = access === 'usable' ? environment('single') : { ...environment('single', { usable: false }), reason: 'environment_use_required' };
  state.answer = options({ environments: [row] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
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
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('start.quick.label.topic') }), 'Same location{Enter}');
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
  const host = (id: string, name: string, more: Partial<PlaceHost>): PlaceHost => ({
    host: { id, name, address: `acme.${id}` }, owner: null, labels: [], capacity: { cpu_millis: 4000, memory_mb: 8192, disk_gb: 100 },
    declared: { cpu_millis: 1000, memory_mb: 2048, disk_gb: 10 }, free: { cpu_millis: 3000, memory_mb: 6144, disk_gb: 90 },
    used: { cpu_percent: 5, memory_mb: 1024, disk_gb: 8 }, environments: { running: 1, idle: 0 }, storages: [], mounts: [], when_full: 'warn',
    usable: true, reasons: [], warnings: [], current: false, automatic: false, ...more,
  });
  state.answer = options({ environment_default: 'fresh', hosts: [
    host('roomy', 'Roomy host', { automatic: true, warnings: ['no_room'] }),
    host('closed', 'Closed host', { usable: false, reasons: ['template_not_allowed'] }),
  ] });
  show(<NewThreadDialog parent={parent} open onClose={() => undefined} />);
  await userEvent.click(await screen.findByRole('combobox', { name: i18n.t('start.where.title') }));
  const list = document.querySelector<HTMLElement>('[data-start-hosts]');
  if (list === null) throw new Error('the new environment option lists the hosts it could be made on');
  const hosts = within(list);
  expect(hosts.getByText(i18n.t('change.where.hostReason.template_not_allowed'))).toBeVisible();
  expect(hosts.getByText(i18n.t('change.where.hostWarning.no_room'))).toBeVisible();
  expect(hosts.queryByText(i18n.t('start.where.hostClosed'))).toBeNull();
});
