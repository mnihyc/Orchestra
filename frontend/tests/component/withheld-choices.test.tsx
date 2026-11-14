/** A speed, an access program or a context size that the model's managers switched on and the thread cannot use stands in its own control, dimmed, with the reason and, where the person may lift it, the act that does. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as NodesApi from '@/api/nodes';
import type * as SettingsApi from '@/api/settings';
import type * as ThreadsApi from '@/api/threads';
import { ModelEffort } from '@/features/composer/ModelEffort';
import { ContextRing } from '@/features/usage/context/ContextRing';
import type { ModelOption, ResolvedSetting, Thread, ThreadParameters, WithheldParameter } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME, MODEL_ESTIMATE } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const server = vi.hoisted(() => ({ settings: new Map<string, ResolvedSetting>(), parameters: new Map<string, ThreadParameters>(), manages: true, change: vi.fn() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof SettingsApi>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve([...server.settings.values()]), staleTime: Infinity }) }));
vi.mock('@/api/threads', async (load) => ({
  ...await load<typeof ThreadsApi>(),
  parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => {
    const parameters = server.parameters.get(id);
    if (parameters === undefined) throw new Error('The controls need the native parameters of their thread');
    return Promise.resolve(parameters);
  }, staleTime: Infinity }),
}));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof NodesApi>(), nodeQuery: (id: string) => ({ queryKey: ['node', id], staleTime: Infinity,
  queryFn: () => Promise.resolve({ id, kind: 'topic', name: 'Release', access: { permissions: server.manages ? ['topic.view', 'settings.manage'] : ['topic.view'] } }) }) }));
vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: server.change }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const opened: ModelOption = { ...MODEL_ESTIMATE, model_name: 'alpha-routing', display_name: 'Alpha', allowed: true, reason: null,
  parameters: { efforts: ['low', 'high'], default_effort: 'high', service_tiers: [{ id: 'fast', label: 'Fast' }], access_programs: ['daybreakBlue'], context_window: 272_000, context_max: 1_000_000 } };
const setting = (key: string, value: unknown, revision: number): ResolvedSetting => ({ key, value, own: { value, revision, locked: false }, source: { scope: 'thread', node: null }, locked_by: null, writable: true, parts: [] });
let native: Thread;

/** What the thread's parameters say is withheld, with the model that opened it. */
function serve(withheld: WithheldParameter[]) {
  server.parameters.set(native.id, { model: { value: 'alpha-routing', options: [opened], writable: true, source: { scope: 'thread', node: null } },
    effort: { value: 'high', writable: true, supported: true, source: { scope: 'thread', node: null } }, parameters: [], withheld, approval_mode: 'on_request', approval_reviewer: null });
}
const chip = (what: string, value: string) => i18n.t('composer.chip.value', { what, value });
const agent = () => native.binding?.agent.name ?? '';

beforeEach(() => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The controls need their native thread');
  native = { ...world.threads.thread(person, row), model: 'alpha-routing', model_display: 'Alpha', effort: 'high',
    path: [{ id: 'org-1', kind: 'org', name: 'Acme', redacted: false }, { id: 'topic-1', kind: 'topic', name: 'Release', redacted: false }] };
  server.parameters.clear(); server.settings.clear(); server.change.mockReset(); server.manages = true;
  for (const one of [setting('agent.model', 'alpha-routing', 3), setting('agent.effort', 'high', 7), setting('agent.params', {}, 1)]) server.settings.set(one.key, one);
});
afterEach(async () => { cleanup(); threadWriter.evict(native.id); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows an access program that a feature switch holds back as a dimmed control that says why and links to the switch (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    serve([{ choice: 'access', key: 'cyber_access_program', reason: { why: 'feature', feature: 'codex.cyber_access' } }]);
    show(<ModelEffort thread={native} />);
    const control = await screen.findByRole('button', { name: chip(i18n.t('composer.model.access.title'), i18n.t('composer.model.access.none')) });
    expect(control).toHaveAttribute('aria-description', i18n.t('composer.withheld.feature'));
    expect(control).toBeEnabled();
    await user.click(control);
    const list = await screen.findByRole('dialog', { name: i18n.t('composer.model.access.title') });
    expect(within(list).getByText(i18n.t('composer.withheld.feature'))).toBeVisible();
    // What the managers switched on is listed, and none of it can be chosen.
    const program = within(list).getByRole('menuitemradio', { name: i18n.t('composer.model.access.names.daybreakBlue') });
    expect(program).toBeDisabled();
    expect(within(list).getByRole('menuitemradio', { name: i18n.t('composer.model.access.none') })).toBeDisabled();
    const act = await within(list).findByRole('link', { name: i18n.t('composer.withheld.act.feature') });
    expect(act).toHaveAttribute('href', '/topic/topic-1?tab=settings&section=defaults&setting=agent.features');
  });
}

it('offers no act to whoever may not manage the settings, and still says why', async () => {
  const user = userEvent.setup();
  server.manages = false;
  serve([{ choice: 'access', key: 'cyber_access_program', reason: { why: 'feature', feature: 'codex.cyber_access' } }]);
  show(<ModelEffort thread={native} />);
  await user.click(await screen.findByRole('button', { name: chip(i18n.t('composer.model.access.title'), i18n.t('composer.model.access.none')) }));
  const list = await screen.findByRole('dialog', { name: i18n.t('composer.model.access.title') });
  expect(within(list).getByText(i18n.t('composer.withheld.feature'))).toBeVisible();
  expect(within(list).queryByRole('link', { name: i18n.t('composer.withheld.act.feature') })).not.toBeInTheDocument();
  expect(within(list).queryByRole('button', { name: /Switch on|Change/ })).not.toBeInTheDocument();
});

it('names the agent that does not offer a speed, with the speed that is switched on listed off', async () => {
  const user = userEvent.setup();
  serve([{ choice: 'speed', reason: { why: 'agent' } }]);
  show(<ModelEffort thread={native} />);
  const control = await screen.findByRole('button', { name: chip(i18n.t('composer.model.speed.title'), i18n.t('composer.model.speed.standard')) });
  expect(control).toHaveAttribute('aria-description', i18n.t('composer.withheld.agent', { agent: agent() }));
  expect(document.querySelectorAll('[data-composer-chip="speed"]')).toHaveLength(1);
  await user.click(control);
  const list = await screen.findByRole('dialog', { name: i18n.t('composer.model.speed.title') });
  expect(within(list).getByRole('menuitemradio', { name: 'Fast' })).toBeDisabled();
  expect(within(list).queryByRole('link')).not.toBeInTheDocument();
  expect(within(list).queryByRole('button', { name: /Switch on|Change/ })).not.toBeInTheDocument();
});

it('offers to change how the thread runs where the credential has to stay with the provider, and where it runs where the environment is of the other kind', async () => {
  const user = userEvent.setup();
  serve([{ choice: 'access', key: 'cyber_access_program', reason: { why: 'access' } }, { choice: 'speed', key: 'service_tier', reason: { why: 'placement' } }]);
  show(<ModelEffort thread={native} />);
  await user.click(await screen.findByRole('button', { name: chip(i18n.t('composer.model.access.title'), i18n.t('composer.model.access.none')) }));
  const access = await screen.findByRole('dialog', { name: i18n.t('composer.model.access.title') });
  expect(within(access).getByText(i18n.t('composer.withheld.access'))).toBeVisible();
  await user.click(within(access).getByRole('button', { name: i18n.t('composer.withheld.act.access') }));
  expect(server.change).toHaveBeenCalledWith({ kind: 'thread', id: native.id, name: native.name }, { kind: 'switch', focus: 'provider' });
  server.change.mockClear();
  await user.keyboard('{Escape}');
  await user.click(await screen.findByRole('button', { name: chip(i18n.t('composer.model.speed.title'), i18n.t('composer.model.speed.standard')) }));
  const speed = await screen.findByRole('dialog', { name: i18n.t('composer.model.speed.title') });
  expect(within(speed).getByText(i18n.t('composer.withheld.placement'))).toBeVisible();
  await user.click(within(speed).getByRole('button', { name: i18n.t('composer.withheld.act.placement') }));
  expect(server.change).toHaveBeenCalledWith({ kind: 'thread', id: native.id, name: native.name }, { kind: 'switch', focus: 'environment' });
});

it('gives nobody the act for a thread that is archived, and lists nothing when nothing is withheld', async () => {
  const user = userEvent.setup();
  serve([{ choice: 'access', key: 'cyber_access_program', reason: { why: 'access' } }]);
  const view = show(<ModelEffort thread={{ ...native, archived: true }} />);
  await user.click(await screen.findByRole('button', { name: chip(i18n.t('composer.model.access.title'), i18n.t('composer.model.access.none')) }));
  const list = await screen.findByRole('dialog', { name: i18n.t('composer.model.access.title') });
  expect(within(list).queryByRole('button', { name: i18n.t('composer.withheld.act.access') })).not.toBeInTheDocument();
  view.unmount();
  server.parameters.clear(); serve([]);
  show(<ModelEffort thread={native} />);
  await screen.findByRole('button', { name: chip(i18n.t('composer.model.chip'), 'Alpha') });
  expect(document.querySelectorAll('[data-composer-chip="speed"], [data-composer-chip="access"]')).toHaveLength(0);
});

it('shows the context sizes that are switched on, off, in the context control of a thread whose agent has no context choice', async () => {
  const user = userEvent.setup();
  serve([{ choice: 'context', reason: { why: 'agent' } }]);
  show(<ContextRing thread={{ ...native, usage: null }} />);
  const control = await screen.findByRole('button', { name: chip(i18n.t('composer.model.context.title'), '272K') });
  await user.click(control);
  const list = await screen.findByRole('dialog', { name: i18n.t('contextUsage.panel.title') });
  expect(within(list).getByText(i18n.t('composer.withheld.agent', { agent: agent() }))).toBeVisible();
  expect(within(list).getByRole('menuitemradio', { name: '1M' })).toBeDisabled();
  expect(within(list).getByRole('menuitemradio', { name: '272K', checked: true })).toBeDisabled();
});
