/** MCP setup uses ordinary fields, exact argv and write-only authentication; Advanced is optional. */
import { cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourceApi from '@/api/resource-tools';
import { ApiError } from '@/api/client';
import { EditMcp } from '@/features/resources/mcp/Edit';
import { ResourceCreateButton } from '@/features/resources/ResourceCreation';
import type { McpServer } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Input } from '@/ui/Input';
import { show } from './support';

const calls = vi.hoisted(() => ({ create: vi.fn<typeof ResourceApi.createMcp>(), update: vi.fn<typeof ResourceApi.updateMcp>() }));
vi.mock('@/api/resource-tools', () => ({ createMcp: calls.create, updateMcp: calls.update, mcpKey: (id: string) => ['mcp-server', id] }));
vi.mock('@/features/resources/HomePicker', () => ({ HomePicker: ({ value, onChange, disabled }: { value: string; onChange: (value: string) => void; disabled?: boolean }) =>
  <Input label={i18n.t('resources.home.label')} value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)} /> }));

const server: McpServer = { id: 'server', name: 'search-tools', description: 'Search the project', address: 'team.search-tools', org_id: 'org', owner: null,
  home: { id: 'org', kind: 'org', name: 'Workshop', redacted: false }, machine: null, environment: null, imported: null, enabled: true, revision: 7,
  access: { permissions: ['mcp.view', 'mcp.manage'], administers: false, participating: true, admin_only: [], through: [] },
  spec: { transport: 'http', url: 'https://tools.example.test/mcp', secret_env: ['TOKEN'], authentication: { kind: 'bearer', secret: 'TOKEN' },
    env: { REGION: 'west' }, tools_allow: ['search'], tools_deny: ['delete'], startup_timeout_secs: 45, working_dir: 'home', requires: { commands: ['git'], capabilities: [] } } };
const appearance = useAppearance.getState();
const oldTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
beforeEach(() => { calls.create.mockReset(); calls.update.mockReset(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', wasDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});
async function look(locale: 'en' | 'zh-CN', theme: 'light' | 'dark') {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
  document.documentElement.classList.toggle('dark', theme === 'dark');
}
function draw(existing?: McpServer, onClose = vi.fn()) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = show(<QueryClientProvider client={cache}><EditMcp server={existing} onClose={onClose} /></QueryClientProvider>);
  return { ...view, cache, onClose };
}
/** Bulk literals exercise one real paste rather than unrelated per-keystroke form reconstruction. */
async function paste(user: ReturnType<typeof userEvent.setup>, field: HTMLElement, value: string) {
  await user.click(field); await user.paste(value);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`adds an authenticated URL with one Add verb and no required JSON (${locale}/${theme})`, async () => {
    await look(locale, theme); const t = i18n.getFixedT(locale), user = userEvent.setup();
    calls.create.mockResolvedValue(server);
    const view = draw();
    const dialog = screen.getByRole('dialog');
    expect(screen.getByRole('heading', { name: t('resourceDetails.mcp.add') })).toBeVisible();
    expect(screen.getByRole('heading')).not.toHaveTextContent('…');
    expect(screen.queryByRole('button', { name: t('resources.edit.save') })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: t('common.add') })).toHaveLength(1);
    expect(screen.queryByLabelText(t('resourceDetails.mcp.form.json'))).not.toBeInTheDocument();
    await paste(user, screen.getByRole('textbox', { name: t('resources.edit.name') }), 'remote-tools');
    await paste(user, screen.getByRole('textbox', { name: t('resources.home.label') }), 'org');
    await paste(user, screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.url') }), 'https://remote.example.test/mcp');
    await user.click(screen.getByRole('combobox', { name: t('resourceDetails.mcp.form.authentication') }));
    await user.click(screen.getByRole('option', { name: t('resourceDetails.mcp.form.bearer') }));
    const add = screen.getByRole('button', { name: t('common.add') });
    expect(add).toHaveAttribute('aria-disabled', 'true'); await user.click(add);
    expect(calls.create).not.toHaveBeenCalled(); expect(view.onClose).not.toHaveBeenCalled();
    const token = screen.getByLabelText(t('resourceDetails.mcp.form.token'));
    expect(token).toHaveAttribute('type', 'password');
    await paste(user, token, 'private-auth-value');
    expect(screen.queryByLabelText(t('resourceDetails.mcp.form.json'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: t('common.add') }));
    expect(calls.create).toHaveBeenCalledTimes(1);
    expect(calls.create).toHaveBeenCalledWith(expect.objectContaining({ name: 'remote-tools', home_id: 'org',
      spec: { transport: 'http', url: 'https://remote.example.test/mcp', authentication: { kind: 'bearer', secret: 'MCP_BEARER_TOKEN' }, secret_env: ['MCP_BEARER_TOKEN'] },
      secrets: { MCP_BEARER_TOKEN: 'private-auth-value' } }), expect.any(String));
    expect(JSON.stringify(calls.create.mock.calls[0]?.[0].spec)).not.toContain('private-auth-value');
    expect(view.cache.getQueryData(['mcp-server', server.id])).toEqual(server);
    expect(view.onClose).toHaveBeenCalledTimes(1);
    expect(dialog).not.toHaveTextContent('private-auth-value');
  });

  it(`keeps argv boundaries and command authentication without a shell parser (${locale}/${theme})`, async () => {
    await look(locale, theme); const t = i18n.getFixedT(locale), user = userEvent.setup();
    calls.create.mockResolvedValue(server); draw();
    await paste(user, screen.getByRole('textbox', { name: t('resources.edit.name') }), 'local-tools');
    await paste(user, screen.getByRole('textbox', { name: t('resources.home.label') }), 'org');
    await user.click(screen.getByRole('combobox', { name: t('resourceDetails.mcp.form.connection') }));
    await user.click(screen.getByRole('option', { name: t('resourceDetails.mcp.form.stdio') }));
    await paste(user, screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.command') }), '/opt/my tools/server');
    const args = screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.args') }); args.focus();
    await user.paste('--project\npath with spaces\n"literal quotes"');
    await user.click(screen.getByRole('combobox', { name: t('resourceDetails.mcp.form.authentication') }));
    await user.click(screen.getByRole('option', { name: t('resourceDetails.mcp.form.variables') }));
    await paste(user, screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.variableNames') }), 'API_KEY');
    await paste(user, screen.getByLabelText('API_KEY'), 'command-private-value');
    await user.click(screen.getByRole('button', { name: t('common.add') }));
    expect(calls.create).toHaveBeenCalledWith(expect.objectContaining({ spec: { transport: 'stdio', command: ['/opt/my tools/server', '--project', 'path with spaces', '"literal quotes"'], secret_env: ['API_KEY'] }, secrets: { API_KEY: 'command-private-value' } }), expect.any(String));
  });

  it(`edits an ordinary field without erasing Advanced settings or returning a saved token (${locale}/${theme})`, async () => {
    await look(locale, theme); const t = i18n.getFixedT(locale), user = userEvent.setup();
    calls.update.mockResolvedValue({ ...server, revision: 8 }); draw(server);
    const token = screen.getByLabelText(t('resourceDetails.mcp.form.token'));
    expect(token).toHaveValue(''); expect(token).toHaveAttribute('placeholder', t('resourceDetails.mcp.form.keepSecret'));
    const url = screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.url') }); await user.clear(url); await user.type(url, 'https://new.example.test/mcp');
    await user.click(screen.getByRole('button', { name: t('resources.edit.save') }));
    expect(calls.update).toHaveBeenCalledWith(server.id, expect.objectContaining({ expected_revision: 7, spec: { ...(server.spec as Record<string, unknown>), url: 'https://new.example.test/mcp' } }));
    expect(calls.update.mock.calls[0]?.[1]).not.toHaveProperty('secrets');
  });
}

it('puts a refusal at its field, retains the token draft and retries one create with the same request identity', async () => {
  const user = userEvent.setup(), t = i18n.t;
  calls.create.mockRejectedValueOnce(new ApiError({ type: 'orch:error:request.invalid', status: 400, code: 'request.invalid', title: 'Invalid', detail: 'Invalid request', request_id: 'private-request',
    errors: [{ path: 'spec.url', message: 'This address is not permitted. Choose another server.' }] }, null)).mockResolvedValueOnce(server);
  const view = draw();
  await user.type(screen.getByRole('textbox', { name: t('resources.edit.name') }), 'remote-tools');
  await user.type(screen.getByRole('textbox', { name: t('resources.home.label') }), 'org');
  await user.type(screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.url') }), 'https://tools.example.test/mcp');
  await user.click(screen.getByRole('combobox', { name: t('resourceDetails.mcp.form.authentication') }));
  await user.click(screen.getByRole('option', { name: t('resourceDetails.mcp.form.bearer') }));
  await user.type(screen.getByLabelText(t('resourceDetails.mcp.form.token')), 'retained-token');
  await user.click(screen.getByRole('button', { name: t('common.add') }));
  expect(await screen.findByText('This address is not permitted. Choose another server.')).toBeVisible();
  expect(screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.url') })).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByLabelText(t('resourceDetails.mcp.form.token'))).toHaveValue('retained-token');
  expect(view.onClose).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: t('common.add') }));
  expect(calls.create).toHaveBeenCalledTimes(2);
  expect(calls.create.mock.calls[1]).toEqual(calls.create.mock.calls[0]);
  expect(view.onClose).toHaveBeenCalledTimes(1);
});

it('keeps a malformed Advanced draft whole until it is corrected, and keeps imported definitions read-only', async () => {
  const user = userEvent.setup(), t = i18n.t;
  calls.update.mockResolvedValue({ ...server, revision: 8 });
  const view = draw(server);
  await user.click(screen.getByText(t('resourceDetails.mcp.form.advanced')));
  const definition = screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.json') });
  await user.clear(definition); await user.paste('[');
  const save = screen.getByRole('button', { name: t('resources.edit.save') });
  expect(save).toHaveAttribute('aria-disabled', 'true'); await user.click(save); expect(calls.update).not.toHaveBeenCalled();
  expect(screen.getByRole('textbox', { name: t('resourceDetails.mcp.form.url') })).toBeDisabled();
  expect(definition).toHaveValue('[');
  await user.clear(definition); await user.paste(JSON.stringify({ transport: 'http', url: 'https://advanced.example.test/mcp', tools_allow: ['one'] }));
  await user.click(screen.getByRole('button', { name: t('resources.edit.save') }));
  expect(calls.update).toHaveBeenCalledWith(server.id, expect.objectContaining({ expected_revision: 7, spec: { transport: 'http', url: 'https://advanced.example.test/mcp', tools_allow: ['one'] } }));
  view.unmount(); calls.update.mockClear();
  const imported: McpServer = { ...server, spec: { transport: 'stdio' }, machine: { id: 'machine', name: 'Laptop', online: true, placement: 'connected', platform_os: 'linux' },
    imported: { source: 'codex', scope: 'user', directory: null, name: 'search-tools', at: '2026-10-15T12:00:00Z', by: { id: 'person', username: 'ada', display_name: 'Ada', kind: 'human', avatar: { color: 1, text: 'A' } }, stale: false } };
  draw(imported);
  expect(screen.getByRole('combobox', { name: t('resourceDetails.mcp.form.connection') })).toBeDisabled();
  expect(screen.queryByLabelText(t('resourceDetails.mcp.form.token'))).not.toBeInTheDocument();
  expect(screen.queryByLabelText(t('resourceDetails.mcp.form.json'))).not.toBeInTheDocument();
  await user.click(screen.getByText(t('resourceDetails.mcp.form.advanced')));
  expect(await screen.findByLabelText(t('resourceDetails.mcp.form.json'))).toHaveAttribute('readonly');
  const name = screen.getByRole('textbox', { name: t('resources.edit.name') }); await user.clear(name); await user.type(name, 'renamed-tools');
  await user.click(screen.getByRole('button', { name: t('resources.edit.save') }));
  expect(calls.update.mock.calls[0]?.[1]).not.toHaveProperty('spec');
  expect(calls.update.mock.calls[0]?.[1]).not.toHaveProperty('secrets');
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`opens the actual lazy Add MCP entry with a form and no JSON or write on open (${locale}/${theme})`, async () => {
    await look(locale, theme); const user = userEvent.setup();
    expect(i18n.t('common.add')).toBe(locale === 'en' ? 'Add' : '添加');
    show(<ResourceCreateButton kind="mcp" />);
    await user.click(screen.getByRole('button', { name: i18n.t('resourceCreation.mcp') }));
    const dialog = await screen.findByRole('dialog', { name: i18n.t('resourceDetails.mcp.add') });
    expect(within(dialog).getByRole('textbox', { name: i18n.t('resources.edit.name') })).toBeVisible();
    expect(within(dialog).getByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.url') })).toBeVisible();
    expect(within(dialog).getByRole('button', { name: i18n.t('common.add') })).toHaveAttribute('aria-disabled', 'true');
    expect(within(dialog).queryByLabelText(i18n.t('resourceDetails.mcp.form.json'))).not.toBeInTheDocument();
    expect(calls.create).not.toHaveBeenCalled();
    await user.click(within(dialog).getByRole('button', { name: i18n.t('common.cancel') }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });
}

it('retains a multiline script as one literal argument and keeps Advanced drafts only behind their disclosure', async () => {
  const user = userEvent.setup(), script = 'echo "first"\necho "second"\nexit 0';
  calls.update.mockResolvedValue({ ...server, revision: 8 });
  draw({ ...server, spec: { transport: 'stdio', command: ['sh', '-c', script], tools_allow: ['search'] } });
  expect(screen.getByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.argument', { number: 2 }) })).toHaveValue(script);
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.argument', { number: 2 }) }), { target: { value: script + '\necho done' } });
  expect(screen.getByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.argument', { number: 1 }) })).toHaveValue('-c');
  await user.click(screen.getByText(i18n.t('resourceDetails.mcp.form.advanced')));
  const definition = await screen.findByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.json') });
  fireEvent.change(definition, { target: { value: '[' } });
  await user.click(screen.getByText(i18n.t('resourceDetails.mcp.form.advanced')));
  await waitFor(() => expect(screen.queryByLabelText(i18n.t('resourceDetails.mcp.form.json'))).not.toBeInTheDocument());
  expect(screen.getByRole('button', { name: i18n.t('resources.edit.save') })).toHaveAttribute('aria-disabled', 'true');
  await user.click(screen.getByText(i18n.t('resourceDetails.mcp.form.advanced')));
  expect(await screen.findByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.json') })).toHaveValue('[');
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('resourceDetails.mcp.form.json') }), { target: { value: JSON.stringify({ transport: 'stdio', command: ['sh', '-c', script + '\necho done'], tools_allow: ['search'] }) } });
  await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
  expect(calls.update).toHaveBeenCalledExactlyOnceWith(server.id, expect.objectContaining({ expected_revision: 7, spec: { transport: 'stdio', command: ['sh', '-c', script + '\necho done'], tools_allow: ['search'] } }));
});
