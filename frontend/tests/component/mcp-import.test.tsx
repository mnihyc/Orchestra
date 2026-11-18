/** Machine discoveries keep their exact origin, native import right and committed receipt. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Environments from '@/api/environments';
import type * as Library from '@/api/resourceLibrary';
import { McpServers } from '@/features/resources/environments/McpServers';
import { MachineTools } from '@/features/resources/machines/Tools';
import { FoundMcp, ImportMcp } from '@/features/resources/mcp/Import';
import { ImportHere } from '@/features/resources/mcp/ImportHere';
import type { Environment, EnvironmentMcp, EnvironmentRef, McpFound } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const environment: EnvironmentRef = { id: 'environment-1', name: 'Workspace', address: 'workspace', placement: 'connected', status: 'running', owner: ADA, ports_exposed: false };
const found: McpFound = { name: 'docs', source: 'codex', scope: 'project', directory: '/workspace/project', transport: 'stdio', imported: null };
const snapshot: EnvironmentMcp = { applies: 'next_turn', found: [found], servers: [], import: { allowed: true, reason: null } };
const calls = vi.hoisted(() => ({
  read: vi.fn<() => Promise<EnvironmentMcp>>(), import: vi.fn<typeof Environments.importEnvironmentMcp>(), sameReader: vi.fn<() => boolean>(),
}));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.sameReader() }));
vi.mock('@/api/environments', async (original) => ({ ...await original<typeof Environments>(),
  environmentMcpQuery: (id: string) => ({ queryKey: ['environment-mcp', id], queryFn: calls.read }),
  importEnvironmentMcp: (...args: Parameters<typeof Environments.importEnvironmentMcp>) => calls.import(...args),
}));
vi.mock('@/features/resources/mcp/Enabled', () => ({ EnvironmentMcpChoices: () => null }));
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject }: { subject: { server?: { name: string }; agent?: { name: string } } }) => <span>{subject.server?.name ?? subject.agent?.name}</span> }));
beforeEach(() => {
  vi.clearAllMocks();
  calls.read.mockResolvedValue(snapshot);
  calls.import.mockResolvedValue({ id: 'made-server', server: null });
  calls.sameReader.mockReturnValue(true);
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('imports the exact machine discovery and switches it on in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<MachineTools machine={{ id: 'machine-1', name: 'Workstation', environments: [environment] }} />);
  await user.click(await screen.findByRole('button', { name: i18n.t('mcpImport.import') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('mcpImport.title') });
  expect(within(dialog).getByText(i18n.t('mcpImport.stayOnMachine'))).toBeInTheDocument();
  expect(within(dialog).getByRole('switch', { name: i18n.t('mcpImport.enableHere') })).toBeChecked();
  await user.click(within(dialog).getByRole('button', { name: i18n.t('mcpImport.import') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledOnce());
  expect(calls.import).toHaveBeenCalledWith(environment.id, { name: found.name, source: found.source, scope: 'project', directory: found.directory, enable: true }, expect.any(String));
  await waitFor(() => expect(screen.getAllByText(i18n.t('mcpImport.imported')).length).toBeGreaterThan(0));
  expect(screen.queryByRole('button', { name: i18n.t('mcpImport.import') })).toBeNull();
});

it.each(['policy', 'not_owner'] as const)('keeps %s refusal visible and never imports', async (reason) => {
  calls.read.mockResolvedValue({ ...snapshot, import: { allowed: false, reason } });
  show(<FoundMcp environment={environment} />);
  const button = await screen.findByRole('button', { name: i18n.t('mcpImport.import') });
  expect(button).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByText(i18n.t(reason === 'policy' ? 'mcpImport.policyOff' : 'mcpImport.notOwner'))).toBeInTheDocument();
  await userEvent.click(button);
  expect(calls.import).not.toHaveBeenCalled();
});

it('supports an alias and opt-out without changing the discovered identity', async () => {
  const user = userEvent.setup();
  show(<ImportMcp environment={environment} found={{ ...found, name: 'Outside Pattern' }} right={snapshot.import} onClose={vi.fn()} onImported={vi.fn()} />);
  const input = screen.getByRole('textbox', { name: i18n.t('mcpImport.name') });
  await user.clear(input); await user.type(input, 'work-docs');
  await user.click(screen.getByRole('switch', { name: i18n.t('mcpImport.enableHere') }));
  await user.click(screen.getByRole('button', { name: i18n.t('mcpImport.import') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledWith(environment.id,
    { name: 'Outside Pattern', as: 'work-docs', source: 'codex', scope: 'project', directory: found.directory, enable: false }, expect.any(String)));
});

it('retries a lost response with the same key', async () => {
  calls.import.mockRejectedValueOnce(new Error('Lost response')).mockResolvedValueOnce({ id: 'made-server', server: null });
  show(<ImportMcp environment={environment} found={found} right={snapshot.import} onClose={vi.fn()} onImported={vi.fn()} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.import') }));
  await screen.findByRole('alert');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.import') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledTimes(2));
  expect(calls.import.mock.calls[0]?.[2]).toBe(calls.import.mock.calls[1]?.[2]);
});

it('does not publish an old reader’s accepted import into the next reader’s UI', async () => {
  let finish: ((made: Awaited<ReturnType<typeof Environments.importEnvironmentMcp>>) => void) | undefined;
  calls.import.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const close = vi.fn(), imported = vi.fn();
  show(<ImportMcp environment={environment} found={found} right={snapshot.import} onClose={close} onImported={imported} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.import') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledOnce());
  const settle = finish;
  if (settle === undefined) throw new Error('The import request did not start');
  calls.sameReader.mockReturnValue(false);
  await act(async () => { settle({ id: 'old-reader-server', server: null }); await Promise.resolve(); });
  await waitFor(() => expect(calls.sameReader).toHaveBeenCalledOnce());
  expect(calls.import).toHaveBeenCalledOnce();
  expect(imported).not.toHaveBeenCalled();
  expect(close).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('imports and switches on every received origin in one act in %s', async (locale) => {
  await setLocale(locale);
  calls.import.mockImplementation((_id, body) => Promise.resolve({ id: `made-${body.name}`, server: null }));
  const second = { ...found, name: 'search', scope: 'user' as const, directory: null };
  show(<ImportHere environment={environment} machine={{ id: 'machine', name: 'Workstation', online: true }} value={{ ...snapshot, found: [found, second] }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.importEnable') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledTimes(2));
  expect(calls.import.mock.calls.map(([, body]) => [body.name, body.scope, body.directory, body.enable]))
    .toEqual([['docs', 'project', found.directory, true], ['search', 'user', null, true]]);
  await waitFor(() => expect(screen.queryByRole('button', { name: i18n.t('mcpImport.importEnable') })).toBeNull());
});

it('keeps accepted origins and retries only an unfinished origin with its old key', async () => {
  calls.import.mockResolvedValueOnce({ id: 'docs-id', server: null }).mockRejectedValueOnce(new Error('Lost response')).mockResolvedValueOnce({ id: 'search-id', server: null });
  show(<ImportHere environment={environment} machine={{ id: 'machine', name: 'Workstation', online: true }} value={{ ...snapshot, found: [found, { ...found, name: 'search' }] }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.importEnable') }));
  await screen.findByRole('alert');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('mcpImport.importEnable') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledTimes(3));
  expect(calls.import.mock.calls.filter(([, body]) => body.name === 'docs')).toHaveLength(1);
  expect(calls.import.mock.calls[1]?.[2]).toBe(calls.import.mock.calls[2]?.[2]);
});

it('does not describe policy-suppressed discovery as an empty native configuration', async () => {
  calls.read.mockResolvedValue({ ...snapshot, found: [], import: { allowed: false, reason: 'policy' } });
  show(<FoundMcp environment={environment} />);
  expect(await screen.findByText(i18n.t('mcpImport.policyOff'))).toBeInTheDocument();
  expect(screen.queryByText(i18n.t('mcpImport.empty'))).toBeNull();
});

const ownEnvironment: Pick<Environment, 'id' | 'name' | 'status' | 'access' | 'machine'> = {
  ...environment, machine: { id: 'machine-1', name: 'Workstation', online: true, placement: 'connected', platform_os: 'linux' },
  access: { permissions: ['environment.view', 'environment.manage'], administers: false, participating: true, admin_only: [], through: [] },
};

it.each(['en', 'zh-CN'] as const)('lists native connections in the thread Environment panel before any import in %s', async (locale) => {
  await setLocale(locale);
  calls.read.mockResolvedValue({ ...snapshot, found: [{ ...found, state: 'running', using: 'native' }] });
  show(<McpServers environment={ownEnvironment} threadId="thread-1" compact />);
  const table = await screen.findByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText(found.name)).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpServers.states.running'))).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpImport.usingNative'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('mcpImport.importEnable') })).toBeVisible();
  expect(screen.queryByText(i18n.t('mcpServers.empty'))).not.toBeInTheDocument();
  expect(calls.import).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('imports a copy from the native row without changing the connection the program uses in %s', async (locale) => {
  await setLocale(locale);
  let value: EnvironmentMcp = { ...snapshot, found: [{ ...found, state: 'running', using: 'native' }] };
  calls.read.mockImplementation(() => Promise.resolve(value));
  calls.import.mockImplementation(() => {
    value = { ...value, found: value.found.map((row) => ({ ...row, imported: 'native-copy' })) };
    return Promise.resolve({ id: 'native-copy', server: null });
  });
  show(<McpServers environment={ownEnvironment} threadId="thread-1" compact />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('mcpImport.importEnable') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledOnce());
  expect(calls.import).toHaveBeenCalledWith(environment.id, { source: found.source, scope: found.scope, name: found.name, directory: found.directory, enable: true }, expect.any(String));
  const table = screen.getByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText(i18n.t('mcpImport.usingNative'))).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpServers.states.running'))).toBeVisible();
  expect(value.found[0]).toMatchObject({ name: found.name, source: found.source, directory: found.directory, imported: 'native-copy' });
});

it.each(['native', 'orch'] as const)('names the reported %s definition when an imported resource shares the native name', async (using) => {
  calls.read.mockResolvedValue({ ...snapshot, found: [{ ...found, imported: 'native-copy', state: 'running', using }] });
  show(<FoundMcp environment={environment} />);
  const table = await screen.findByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText(i18n.t(using === 'native' ? 'mcpImport.usingNative' : 'mcpImport.usingOrch'))).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpImport.imported'))).toBeVisible();
});

it('keeps unreported native states honest and searches every received configuration', async () => {
  calls.read.mockResolvedValue({ ...snapshot, found: Array.from({ length: 13 }, (_, index) => ({ ...found, name: `native-${index + 1}` })) });
  show(<FoundMcp environment={environment} />);
  const search = await screen.findByRole('textbox', { name: i18n.t('mcpImport.search') });
  await userEvent.type(search, 'native-13');
  const table = screen.getByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText('native-13')).toBeVisible();
  expect(within(table).queryByText('native-1')).not.toBeInTheDocument();
  expect(within(table).getByText(i18n.t('mcpImport.notReported'))).toBeVisible();
  expect(within(table).queryByText(i18n.t('mcpServers.states.running'))).not.toBeInTheDocument();
  expect(table.closest('[data-scroll-allow="table"]')).toHaveClass('max-h-80');
});

it('says that the effective definition was not reported rather than guessing from an imported name', async () => {
  calls.read.mockResolvedValue({ ...snapshot, found: [{ ...found, imported: 'native-copy', state: 'running' }] });
  show(<FoundMcp environment={environment} />);
  const table = await screen.findByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText(i18n.t('mcpImport.definitionNotReported'))).toBeVisible();
  expect(within(table).queryByText(i18n.t('mcpImport.usingNative'))).not.toBeInTheDocument();
  expect(within(table).queryByText(i18n.t('mcpImport.usingOrch'))).not.toBeInTheDocument();
});

it('imports the selected native profile when two profiles configure the same name', async () => {
  calls.read.mockResolvedValue({ ...snapshot, found: [
    { ...found, origin: 'first-native-origin', profile: { id: 'first-profile', name: 'First native home' } },
    { ...found, origin: 'second-native-origin', profile: { id: 'second-profile', name: 'Second native home' } },
  ] });
  show(<FoundMcp environment={environment} />);
  const profile = await screen.findByText('Second native home');
  const row = profile.closest('tr');
  if (row === null) throw new Error('Each native profile has its own discovery row');
  await userEvent.click(within(row).getByRole('button', { name: i18n.t('mcpImport.import') }));
  const dialog = screen.getByRole('dialog', { name: i18n.t('mcpImport.title') });
  expect(within(dialog).getByText('Second native home')).toBeVisible();
  await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('mcpImport.import') }));
  await waitFor(() => expect(calls.import).toHaveBeenCalledOnce());
  expect(calls.import.mock.calls[0]?.[1]).toMatchObject({ origin: 'second-native-origin', name: found.name, source: found.source, enable: true });
});

it('lists a reported connection even when the program gave no importable static configuration', async () => {
  calls.read.mockResolvedValue({ ...snapshot, found: [], connections: [{ name: 'dynamic-tools', source: 'codex', state: 'running', using: 'native' }] });
  show(<McpServers environment={ownEnvironment} threadId="thread-1" compact />);
  const table = await screen.findByRole('table', { name: i18n.t('mcpImport.found') });
  expect(within(table).getByText('dynamic-tools')).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpServers.states.running'))).toBeVisible();
  expect(within(table).getByText(i18n.t('mcpImport.configurationNotReported'))).toBeVisible();
  expect(calls.import).not.toHaveBeenCalled();
});
