/** Compact resource forms preserve drafts, review competing writes and use only the reviewed revision. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentKey, checkTemplate, templateKey, updateAgent, updateTemplate } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { AgentScreen } from '@/features/resources/agents/Profile';
import { TemplateScreen } from '@/features/resources/templates/Template';
import type { Agent, Template } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
import { harnessName } from '../doubles/harness-names';

const server = vi.hoisted(() => ({ agent: vi.fn<() => Promise<Agent>>(), template: vi.fn<() => Promise<Template>>() }));
const schema = { type: 'object', properties: { image: { type: 'string', title: 'Image reference' } }, additionalProperties: true };
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof ResourcesApi>();
  return { ...actual, updateAgent: vi.fn(), updateTemplate: vi.fn(), checkTemplate: vi.fn(),
    agentQuery: (id: string) => ({ queryKey: actual.agentKey(id), queryFn: server.agent, staleTime: Infinity }),
    templateQuery: (id: string) => ({ queryKey: actual.templateKey(id), queryFn: server.template, staleTime: Infinity }),
    harnessKindsQuery: { queryKey: ['harness-kinds'], queryFn: () => Promise.resolve([]), staleTime: Infinity },
    driversQuery: { queryKey: ['drivers'], queryFn: () => Promise.resolve([{ name: 'docker', descriptor: { spec_schemas: { environment: schema } } }]), staleTime: Infinity },
  };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/resources/agents/acts', () => ({ useAgentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/templates/acts', () => ({ useTemplateActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/features/resources/ResourceCreation', () => ({ ResourceCreateButton: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/TemplateGuideLink', () => ({ TemplateGuideLink: () => null }));
vi.mock('@/features/help/GuideLink', () => ({ GuideLink: () => null }));
// These cases exercise the real tabs and forms; routing and title-bar registration are tested by the browser walks.
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(),
  Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title }: { title: string }) => <h1>{title}</h1>,
}));

const access = { permissions: ['agent.view', 'agent.manage', 'template.view', 'template.manage'], administers: false, through: [], admin_only: [], participating: true };
const agent: Agent = { id: 'profile', name: 'Research', address: 'research', home: null, org_id: null, owner: null,
  machine: null, harness_kind: 'codex', harness: harnessName('codex'), profile: { effort: 'low' }, descriptor: {}, descriptor_overridden: [], disabled: false,
  access, revision: 1, created_at: '2026-10-15T12:00:00Z', updated_at: '2026-10-15T12:00:00Z' };
const template: Template = { id: 'recipe', name: 'Research runtime', address: 'research-runtime', description: 'Original notes',
  home: null, org_id: null, owner: null, driver: 'docker', kind: 'environment', enabled: true,
  spec: { image: 'first', plugin: { enabled: true } }, units: 0, access, revision: 1 };

function frame(kind: 'agent' | 'template', value: Agent | Template = kind === 'agent' ? agent : template) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  client.setQueryData(kind === 'agent' ? agentKey(value.id) : templateKey(value.id), value);
  const view = show(<QueryClientProvider client={client}>{kind === 'agent'
    ? <AgentScreen agentId={value.id} tab="profile" onTab={vi.fn()} />
    : <TemplateScreen templateId={value.id} tab="spec" onTab={vi.fn()} />}</QueryClientProvider>);
  return { ...view, client };
}
beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
  vi.stubGlobal('ResizeObserver', Observer);
  server.agent.mockResolvedValue(agent);
  server.template.mockResolvedValue(template);
  vi.mocked(checkTemplate).mockResolvedValue({ valid: true, errors: [] });
});
afterEach(async () => { cleanup(); vi.unstubAllGlobals(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps an agent name draft across a competing profile change until it is explicitly reviewed in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const { client } = frame('agent');
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('agents.profile.name') }), { target: { value: 'My name' } });
  const fresh = { ...agent, revision: 2, name: 'Other name', profile: { effort: 'high' } };
  act(() => { client.setQueryData(agentKey(agent.id), fresh); });
  const save = screen.getByRole('button', { name: i18n.t('agents.profile.save') });
  await waitFor(() => expect(save).toHaveAttribute('aria-disabled', 'true'));
  await user.click(save);
  expect(updateAgent).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: i18n.t('agents.profile.name') })).toHaveValue('My name');
  await user.click(screen.getByText(i18n.t('settings.compact.review'), { exact: true }));
  const review = screen.getByRole('table', { name: i18n.t('settings.compact.review') });
  expect(within(review).getByText('Other name')).toBeVisible();
  expect(within(review).getByText('My name')).toBeVisible();
  vi.mocked(updateAgent).mockResolvedValue({ ...fresh, revision: 3, name: 'My name' });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.keepDraft') }));
  await user.click(save);
  await waitFor(() => expect(updateAgent).toHaveBeenCalledExactlyOnceWith(agent.id, { expected_revision: 2, name: 'My name' }));
  await waitFor(() => expect(client.getQueryData<Agent>(agentKey(agent.id))?.profile).toEqual({ effort: 'high' }));
  await waitFor(() => expect(save).toHaveAttribute('aria-disabled', 'true'));
});

it('reviews a template conflict, preserves an unedited newer description and saves only its proposed configuration', async () => {
  const user = userEvent.setup();
  const { client } = frame('template');
  const image = await screen.findByRole('textbox', { name: 'Image reference' });
  fireEvent.change(image, { target: { value: 'mine' } });
  const fresh = { ...template, revision: 2, description: 'Other writer’s notes' };
  act(() => { client.setQueryData(templateKey(template.id), fresh); });
  // Query-cache observers notify asynchronously; wait for the competing revision's UI.
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('templates.spec.save') })).toHaveAttribute('aria-disabled', 'true'));
  expect(image).toHaveValue('mine');
  expect(updateTemplate).not.toHaveBeenCalled();
  await user.click(await screen.findByText(i18n.t('settings.compact.review'), { exact: true }));
  const review = screen.getByRole('table', { name: i18n.t('settings.compact.review') });
  expect(within(review).getByText('Other writer’s notes')).toBeVisible();
  const nextSpec = { image: 'mine', plugin: { enabled: true } };
  vi.mocked(updateTemplate).mockResolvedValue({ ...fresh, revision: 3, spec: nextSpec });
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.keepDraft') }));
  await user.click(screen.getByRole('button', { name: i18n.t('templates.spec.save') }));
  await waitFor(() => expect(updateTemplate).toHaveBeenCalledExactlyOnceWith(template.id, { expected_revision: 2, spec: nextSpec }));
  expect(client.getQueryData<Template>(templateKey(template.id))?.description).toBe('Other writer’s notes');
});

it.each(['en', 'zh-CN'] as const)('retains an invalid auxiliary template draft while its row is closed and discards only on request in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  frame('template');
  const editName = i18n.t('settings.compact.editValue', { name: i18n.t('templates.editor.other') });
  await user.click(await screen.findByRole('button', { name: editName }));
  const other = await screen.findByRole('textbox', { name: i18n.t('templates.editor.other') });
  await waitFor(() => expect(checkTemplate).toHaveBeenCalledTimes(1));
  fireEvent.change(other, { target: { value: '{"plugin":' } });
  other.focus();
  await user.keyboard('{Escape}');
  expect(screen.queryByRole('textbox', { name: i18n.t('templates.editor.other') })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: editName })).toHaveFocus();
  expect(screen.getByRole('button', { name: i18n.t('templates.spec.save') })).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByRole('button', { name: editName }));
  expect(screen.getByRole('textbox', { name: i18n.t('templates.editor.other') })).toHaveValue('{"plugin":');
  expect(checkTemplate).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: i18n.t('templates.spec.revert') }));
  expect(screen.queryByRole('textbox', { name: i18n.t('templates.editor.other') })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: editName }));
  expect(screen.getByRole('textbox', { name: i18n.t('templates.editor.other') })).toHaveValue(JSON.stringify({ plugin: { enabled: true } }, null, 2));
  expect(updateTemplate).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('shows an agent reader actual profile values without another Open action or repeated refusals in %s', async (locale) => {
  await setLocale(locale);
  const view = frame('agent', { ...agent, access: { ...access, permissions: ['agent.view'] } });
  expect(await screen.findByText('low', { exact: true })).toBeVisible();
  expect(view.container.querySelectorAll('[data-read-only-settings]')).toHaveLength(1);
  expect(view.container.querySelectorAll('[data-setting-description]')).toHaveLength(1);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-setting-editor-trigger]')).toBeNull();
  expect(view.container).not.toHaveTextContent('"effort"');
  expect(updateAgent).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('shows a template reader its configuration as values under one authority sentence in %s', async (locale) => {
  await setLocale(locale);
  const view = frame('template', { ...template, access: { ...access, permissions: ['template.view'] } });
  expect(await screen.findByText('first', { exact: true })).toBeVisible();
  expect(view.container.querySelectorAll('[data-read-only-settings]')).toHaveLength(1);
  expect(view.container.querySelectorAll('[data-setting-description]')).toHaveLength(1);
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-setting-editor-trigger]')).toBeNull();
  expect(view.container).not.toHaveTextContent('"image"');
  expect(checkTemplate).not.toHaveBeenCalled();
  expect(updateTemplate).not.toHaveBeenCalled();
});
