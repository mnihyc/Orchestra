/** Machine discoveries keep their exact origin, native import right and committed receipt. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Environments from '@/api/environments';
import type * as Library from '@/api/resourceLibrary';
import { MachineTools } from '@/features/resources/machines/Tools';
import { FoundMcp, ImportMcp } from '@/features/resources/mcp/Import';
import { ImportHere } from '@/features/resources/mcp/ImportHere';
import type { EnvironmentMcp, EnvironmentRef, McpFound } from '@/generated/api-types';
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
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject }: { subject: { server: { name: string } } }) => <span>{subject.server.name}</span> }));
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
