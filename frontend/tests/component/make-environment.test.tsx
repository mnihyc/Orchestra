/** Environment creation names its home, respects entry-point presets and exposes the server's field errors. */
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { MakeEnvironmentDialog } from '@/features/resources/environments/MakeEnvironment';
import { i18n, setLocale } from '@/i18n';
import { useSelectedOrg } from '@/shell/org';
import { show } from './support';
import { detailHost, detailTemplate } from './resource-detail-fixtures';

const server = vi.hoisted(() => ({
  create: vi.fn<(body: Record<string, unknown>, key: string) => Promise<{ id: string; name: string }>>(),
  navigate: vi.fn(), close: vi.fn(),
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<object>(), useNavigate: () => server.navigate, useLocation: () => ({ pathname: '/resources/environments', search: {}, searchStr: '' }) }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resources/environments/DataSources', () => ({
  DataSourcesEditor: () => null,
  madeDataSources: () => ({ workspace: { kind: 'made_by_orch' }, home: { kind: 'made_by_orch' } }),
  borrowsData: () => false,
}));
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
    { ...detailTemplate, id: 'template-one', name: 'Node', driver: 'docker', enabled: true, description: '' },
    { ...detailTemplate, id: 'template-two', name: 'Rust', driver: 'docker', enabled: true, description: '' },
  ]) }),
  hostsQuery: () => ({ queryKey: ['make-test-hosts'], queryFn: () => Promise.resolve([
    { ...detailHost, id: 'host-one', name: 'Docker one', schedulable: true, drivers: [{ ...detailHost.drivers[0], name: 'docker', creates: ['environment'] }], machine: { ...detailHost.machine, id: 'machine-one' }, address: 'one' },
    { ...detailHost, id: 'host-two', name: 'Docker two', schedulable: true, drivers: [{ ...detailHost.drivers[0], name: 'docker', creates: ['environment'] }], machine: { ...detailHost.machine, id: 'machine-two' }, address: 'two' },
  ]) }),
}));

beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
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

it.each([{ forThread: false, lifetime: 'temporary' }, { forThread: true, lifetime: 'thread' }])('Temporary submits explicit cleanup consent ($lifetime)', async ({ forThread, lifetime }) => {
  const user = userEvent.setup();
  show(<MakeEnvironmentDialog forThread={forThread} home={{ id: 'beta', name: 'Beta' }} onClose={server.close} />);
  const temporary = await screen.findByRole('switch', { name: 'Temporary' });
  const consent = i18n.t('temporary.consent');
  expect(temporary).not.toBeChecked();
  expect(screen.queryByText(consent)).not.toBeInTheDocument();
  await user.click(temporary);
  expect(screen.queryByText(consent)).not.toBeInTheDocument();
  await user.hover(screen.getByRole('textbox', { name: 'Name' }));
  await user.hover(temporary);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(consent);
  expect(document.querySelector('[data-choice-selected-description]')).toBeNull();
  expect(temporary).toHaveAccessibleDescription(`${i18n.t('temporary.description')} ${consent}`);
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
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
  expect(screen.getByRole('combobox', { name: 'Template' })).toHaveTextContent('Rust');
  expect(screen.getByRole('combobox', { name: 'Host' })).toHaveTextContent('Docker two');
  await user.click(screen.getByRole('button', { name: 'Add environment' }));
  await waitFor(() => expect(server.create).toHaveBeenCalledWith(expect.objectContaining({ template_id: 'template-two', host_id: 'host-two' }), expect.any(String)));
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
