/** Environment creation names its home, respects entry-point presets and exposes the server's field errors. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, client } from '@/api/client';
import type * as EverythingApi from '@/api/everything';
import type * as PoolsApi from '@/api/pools';
import type { EverythingRowPage, Pool } from '@/generated/api-types';
import { inventoryRow } from '../doubles/everything-row';
import { deferred } from './deferred';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { i18n, setLocale } from '@/i18n';
import { useSelectedOrg } from '@/shell/org';
import { show, skipOverflowFallback } from './support';

import { detailHost, detailTemplate } from './resource-detail-fixtures';

skipOverflowFallback();

const server = vi.hoisted(() => ({
  create: vi.fn<(body: Record<string, unknown>, key: string) => Promise<{ id: string; name: string }>>(),
  navigate: vi.fn(), close: vi.fn(),
  withoutHosts: vi.fn<() => boolean | Promise<boolean>>(),
  poolChoices: vi.fn<() => Promise<EverythingRowPage>>(),
  pool: vi.fn<(id: string) => Promise<Pool>>(),
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => server.navigate, useLocation: () => ({ pathname: '/resources/environments', search: {}, searchStr: '' }) }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/environments/DataSources', () => ({
  DataSourcesEditor: () => null,
  madeDataSources: () => ({ workspace: { kind: 'made_by_orch' }, home: { kind: 'made_by_orch' } }),
  borrowsData: () => false,
}));
vi.mock('@/api/everything', async (original) => {
  const actual = await original<typeof EverythingApi>();
  return { ...actual, everythingQuery: (...args: Parameters<typeof actual.everythingQuery>) => ({
    ...actual.everythingQuery(...args), queryFn: server.poolChoices,
  }) };
});
vi.mock('@/api/pools', async (original) => {
  const actual = await original<typeof PoolsApi>();
  return { ...actual, poolQuery: (...args: Parameters<typeof actual.poolQuery>) => ({
    ...actual.poolQuery(...args), queryFn: () => server.pool(args[0]),
  }) };
});
vi.mock('@/api/environments', async (original) => ({ ...await original<object>(), createEnvironment: server.create }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(),
  orgsQuery: { queryKey: ['make-test-orgs'], queryFn: () => Promise.resolve([
    { id: 'acme', name: 'Acme', org_kind: 'team' }, { id: 'beta', name: 'Beta', org_kind: 'team' },
  ]) },
  nodeQuery: (id: string) => ({ queryKey: ['make-test-node', id], queryFn: () => Promise.resolve({ id, org_id: 'beta', kind: 'org', path: [] }) }),
  fetchTree: () => Promise.resolve({ nodes: [] }),
}));
vi.mock('@/api/resources', async (original) => ({ ...await original<object>(),
  templatesQuery: () => ({ queryKey: ['make-test-templates'], queryFn: () => Promise.resolve([
    { ...detailTemplate, id: 'template-one', name: 'Node', driver: 'docker', enabled: true, description: '', access: { ...detailTemplate.access, permissions: [...detailTemplate.access.permissions, 'template.use'] } },
    { ...detailTemplate, id: 'template-two', name: 'Rust', driver: 'docker', enabled: true, description: '', access: { ...detailTemplate.access, permissions: [...detailTemplate.access.permissions, 'template.use'] } },
    { ...detailTemplate, id: 'disabled-template', name: 'Disabled recipe', driver: 'docker', enabled: false, description: '' },
  ]) }),
  hostsQuery: () => ({ queryKey: ['make-test-hosts'], queryFn: async () => await server.withoutHosts() ? [] : [
    { ...detailHost, id: 'host-one', name: 'Docker one', schedulable: true, drivers: [{ ...detailHost.drivers[0], name: 'docker', creates: ['environment'] }], machine: { ...detailHost.machine, id: 'machine-one' }, address: 'one', access: { ...detailHost.access, permissions: [...detailHost.access.permissions, 'host.use'] } },
    { ...detailHost, id: 'host-two', name: 'Docker two', schedulable: true, drivers: [{ ...detailHost.drivers[0], name: 'docker', creates: ['environment'] }], machine: { ...detailHost.machine, id: 'machine-two' }, address: 'two', access: { ...detailHost.access, permissions: [...detailHost.access.permissions, 'host.use'] } },
    { ...detailHost, id: 'read-only-host', name: 'Read-only host', access: { ...detailHost.access, permissions: ['host.view'] } },
  ] }),
}));

beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  server.withoutHosts.mockReturnValue(false);
  server.poolChoices.mockResolvedValue({ items: [], next_cursor: null });
  server.create.mockReset().mockResolvedValue({ id: 'made', name: 'New work' });
  useSelectedOrg.setState({ orgId: 'beta' });
});
afterEach(() => { useSelectedOrg.setState({ orgId: null }); vi.restoreAllMocks(); });

it('defaults Belongs to to the viewed org for a person with two orgs and sends it explicitly', async () => {
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog onClose={server.close} />);
  const belongs = await screen.findByRole('combobox', { name: 'Belongs to' });
  await waitFor(() => expect(belongs).toHaveValue('Beta'));
  await user.type(screen.getByRole('textbox', { name: 'Name' }), 'New work');
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ home_node_id: 'beta' }), expect.any(String)));
  expect(server.close).toHaveBeenCalledOnce();
  expect(server.navigate).toHaveBeenCalledWith({ to: '/resources/environments/$id', params: { id: 'made' } });
});

for (const locale of ['en', 'zh-CN'] as const) it.each([{ forThread: false, lifetime: 'temporary' }, { forThread: true, lifetime: 'thread' }])('Temporary submits explicit cleanup consent ($lifetime)', async ({ forThread, lifetime }) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog forThread={forThread} home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const temporary = await screen.findByRole('switch', { name: i18n.t('temporary.name') });
  const consent = i18n.t(forThread ? 'temporary.consent' : 'temporary.idleConsent');
  expect(temporary).not.toBeChecked();
  expect(screen.queryByText(consent)).not.toBeInTheDocument();
  await user.click(temporary);
  expect(screen.queryByText(consent)).not.toBeInTheDocument();
  await user.hover(screen.getByRole('textbox', { name: i18n.t('environments.make.name') }));
  await user.hover(temporary);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(consent);
  expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
  expect(temporary).toHaveAccessibleDescription(`${i18n.t(forThread ? 'temporary.description' : 'temporary.idleDescription')} ${consent}`);
  await user.click(screen.getByRole('button', { name: i18n.t('environments.make.submit') }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ home_node_id: 'beta', config: { 'env.lifetime': lifetime } }), expect.any(String)));
});

it.each(['en', 'zh-CN'] as const)('keeps Who may use it compact and puts every offered audience behind its one question in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const view = show(<MakeEnvironmentDialog home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const own = await screen.findByRole('radio', { name: i18n.t('choiceCatalog.resourceAccess.only_me.name') });
  expect(own).toBeChecked();
  const label = i18n.t('resources.access.label');
  const home = i18n.t('environments.make.yourPlace');
  const about = i18n.t('resources.access.gives.only_me', { home });
  expect(screen.queryByText(about)).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-choice-selected-description], [data-choice-description], [data-choice-descriptions]')).toBeNull();
  expect(screen.getAllByRole('button', { name: i18n.t('help.hint.label', { term: label }) })).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: label }) }));
  const help = screen.getByRole('dialog', { name: label });
  expect(within(help).getAllByRole('term')).toHaveLength(2);
  expect(help).toHaveTextContent(about);
  expect(help).toHaveTextContent(i18n.t('resources.access.gives.defaults', { home }));
  expect(within(help).getAllByText(i18n.t('resources.access.admins', { home }), { exact: true })).toHaveLength(1);
  expect(own).toBeChecked();
  expect(server.create).not.toHaveBeenCalled();
  await user.keyboard('{Escape}');
  await user.hover(own);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(about);
  expect(own).toHaveAccessibleDescription(`${about} ${i18n.t('resources.access.admins', { home })}`);
  expect(view.container.querySelector('[data-choice-selected-description]')).toBeNull();
  expect(server.create).not.toHaveBeenCalled();
});

it('keeps a current topic selectable without replacing it with one of the orgs', async () => {
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog home={{ id: 'topic', name: 'Current topic' }} onClose={server.close} />);
  const belongs = await screen.findByRole('combobox', { name: 'Belongs to' });
  await waitFor(() => expect(belongs).toHaveValue('Current topic'));
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ home_node_id: 'topic' }), expect.any(String)));
});

it('preserves the template and host selected by Add environment', async () => {
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog templateId="template-two" hostId="host-two" onClose={server.close} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Belongs to' })).toHaveValue('Beta'));
  expect(screen.getByRole('combobox', { name: 'Template' })).toHaveValue('Rust');
  expect(screen.getByRole('combobox', { name: 'Host' })).toHaveValue('Docker two');
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ template_id: 'template-two', host_id: { type: 'single', resource: { kind: 'host', id: 'host-two' } } }), expect.any(String)));
});

it.each(['accepted', 'blocked', 'hidden'] as const)('shows the result of creating through a shared host pool when the individual host list is unavailable (%s)', async (outcome) => {
  const pool: Pool = {
    id: 'shared-hosts', kind: 'host', name: 'Shared build hosts', target: { type: 'pool', kind: 'host', id: 'shared-hosts' },
    enabled: true, policy: { type: 'ordered' }, lend_members: true, revision: 1,
    address: '', description: '', owner: null, home: null, settings: [],
    grouping: { home: null, home_path: [], origin_machine: null, availability: [] },
    access: { permissions: ['pool.view', 'pool.use'], administers: false, through: [] },
  };
  const driverRefusal = 'Build box: its driver cannot make this template';
  if (outcome !== 'accepted') server.create.mockRejectedValueOnce(new ApiError({
    type: 'orch:error:plan.blocked', code: 'plan.blocked', status: 409, title: 'Blocked',
    detail: 'This plan cannot be carried out as it stands.', request_id: 'driver', reason: 'no_host',
    host_blockers: outcome === 'hidden' ? [] : [{ host: { id: 'builder', name: 'Build box', address: '' }, online: true, offline_since: null, reasons: ['driver'], message: driverRefusal }],
  }, null));
  const hosts = deferred<boolean>();
  server.withoutHosts.mockReturnValue(hosts.promise);
  server.poolChoices.mockResolvedValue({
    items: [inventoryRow({ id: pool.id, kind: 'pool', name: pool.name, resource: { kind: 'pool', value: pool } })],
    next_cursor: null,
  });
  server.pool.mockResolvedValue(pool);
  const before = client.session.getState();
  client.session.setState({ status: 'signed_in', userId: 'pool-member' });
  try {
    const user = userEvent.setup();
    show(<MakeEnvironmentDialog home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
    const field = await screen.findByRole('combobox', { name: i18n.t('environments.make.host') });
    expect(server.poolChoices).not.toHaveBeenCalled();
    await user.click(field);
    await user.click(await screen.findByRole('option', { name: pool.name }));
    hosts.reject(new Error('The individual hosts could not be listed.'));
    await screen.findByRole('alert');
    expect(field).toHaveDisplayValue(pool.name);
    expect(screen.queryByText(i18n.t('environments.make.noHost'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('environments.make.submit') }));
    await waitFor(() => expect(server.create).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ template_id: 'template-one', host_id: pool.target }), expect.any(String),
    ));
    expect(server.pool).not.toHaveBeenCalled();
    if (outcome === 'hidden') {
      expect(await screen.findByText(i18n.t('pools.reasons.pool_exhausted'))).toBeVisible();
      expect(screen.queryByText(i18n.t('environments.make.noHostShared'))).not.toBeInTheDocument();
      expect(screen.queryByText(driverRefusal)).not.toBeInTheDocument();
    }
    if (outcome === 'blocked') {
      expect(await screen.findByText(driverRefusal)).toBeVisible();
      expect(field).toHaveDisplayValue(pool.name);
      expect(server.close).not.toHaveBeenCalled();
    }
  } finally {
    client.session.setState(before);
  }
});

it('puts a refused home at Belongs to and scrolls and focuses the invalid control', async () => {
  const user = userEvent.setup();
  const scroll = vi.spyOn(Element.prototype, 'scrollIntoView');
  server.create.mockRejectedValueOnce(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', title: 'Invalid request', status: 400,
    detail: 'Choose another place.', request_id: 'home-refusal', errors: [{ path: 'home_node_id', message: 'Choose another place.' }] }, null));
  show(<MakeEnvironmentDialog onClose={server.close} />);
  const belongs = await screen.findByRole('combobox', { name: 'Belongs to' });
  await waitFor(() => expect(belongs).toHaveValue('Beta'));
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
  await waitFor(() => expect(belongs).toHaveAttribute('aria-invalid', 'true'));
  expect(belongs).toHaveAccessibleDescription('Choose another place.');
  expect(belongs).toHaveFocus();
  expect(scroll).toHaveBeenCalledWith({ block: 'center' });
  expect(server.close).not.toHaveBeenCalled();
});

it('never substitutes another template when an entry-point preset is unavailable', async () => {
  show(<MakeEnvironmentDialog templateId="unavailable" onClose={server.close} />);
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'Belongs to' })).toHaveValue('Beta'));
  expect(screen.getByRole('combobox', { name: 'Template' })).toHaveAccessibleDescription('Choose an available environment template.');
  expect(screen.getByRole('button', { name: 'Add environment' })).toHaveAttribute('aria-disabled', 'true');
  expect(server.create).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`retains Automatic when no named host is currently offered (${locale})`, async () => {
  await setLocale(locale); server.withoutHosts.mockReturnValue(true);
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const host = await screen.findByRole('combobox', { name: i18n.t('environments.make.host') });
  await user.click(host);
  expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.placement.automatic.name') })).not.toHaveAttribute('aria-disabled', 'true');
  await user.keyboard('{Escape}');
  await user.click(screen.getByRole('button', { name: i18n.t('environments.make.submit') }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ host_id: null }), expect.any(String)));
});

for (const locale of ['en', 'zh-CN'] as const) it(`retains an unavailable preset and keeps refused hosts inspectable without creating (${locale})`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog templateId="disabled-template" home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const template = await screen.findByRole('combobox', { name: i18n.t('environments.make.template') });
  await waitFor(() => expect(template).toHaveValue('Disabled recipe'));
  const create = screen.getByRole('button', { name: i18n.t('environments.make.submit') });
  expect(create).toHaveAttribute('aria-disabled', 'true');
  await user.click(template);
  const disabled = screen.getByRole('option', { name: 'Disabled recipe' });
  expect(disabled).toHaveAttribute('aria-disabled', 'true');
  expect(within(disabled).getByText(i18n.t('resourceChoices.disabled'))).toBeVisible();
  await user.click(screen.getByRole('option', { name: 'Node' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('environments.make.host') }));
  const host = screen.getByRole('option', { name: 'Read-only host' });
  expect(host).toHaveAttribute('aria-disabled', 'true');
  expect(within(host).getByText(i18n.t('resourceChoices.hostUse'))).toBeVisible();
  await user.click(host);
  expect(server.create).not.toHaveBeenCalled();
});


it.each(['en', 'zh-CN'] as const)('names the new resource before its other setup choices in %s', async (locale) => {
  await setLocale(locale);
  show(<MakeEnvironmentDialog onClose={server.close} />);
  const name = await screen.findByRole('textbox', { name: i18n.t('environments.make.name') });
  expect(screen.getByRole('dialog').querySelector('input:not([type="hidden"]),textarea,[role="combobox"],select')).toBe(name);
});

for (const locale of ['en', 'zh-CN'] as const) it(`says that no host is shared and who can make it, in the field and in the refusal (${locale})`, async () => {
  await setLocale(locale); server.withoutHosts.mockReturnValue(true);
  // The server's answer when its creator may use no host: nothing is listed to look at.
  server.create.mockRejectedValue(new ApiError({ type: 'orch:error:plan.blocked', code: 'plan.blocked', status: 409, title: 'The plan cannot be carried out',
    detail: 'This plan cannot be carried out as it stands. Look at what blocks it.', request_id: 'no-host', reason: 'no_host', host_blockers: [] }, null));
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const shared = i18n.t('environments.make.noHostShared');
  // Before anything is sent, under Host: no host is shared, hosts are their owners', and the two ways on.
  expect(await screen.findByText(shared)).toBeVisible();
  expect(screen.queryByText(i18n.t('environments.make.noHost'))).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('environments.make.submit') }));
  await waitFor(() => expect(server.create).toHaveBeenCalledTimes(1));
  // The refusal says the same in the dialog, instead of asking the person to look at blockers that are not there.
  const refusal = await screen.findByRole('alert');
  expect(refusal).toHaveTextContent(shared);
  expect(refusal).not.toHaveTextContent(i18n.t('errors.plan.blocked'));
  // Said once: the refusal takes the words the field held.
  expect(screen.getAllByText(shared)).toHaveLength(1);
  expect(server.close).not.toHaveBeenCalled();
});
