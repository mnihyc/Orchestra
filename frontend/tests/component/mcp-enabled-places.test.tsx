import { unjudgedProvenance } from '../doubles/resource-grouping';
/** Named enablement consumes native locations, own revisions and restricted write acknowledgements. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ReceiptAcknowledged } from '@/api/client';
import type * as Library from '@/api/resourceLibrary';
import type * as Settings from '@/api/settings';
import { EnvironmentMcpChoices, McpEnabledIn, McpSwitchAt } from '@/features/resources/mcp/Enabled';
import type { PlacePickerProps } from '@/features/settings/PlacePicker';
import type { LibraryEntry, LibraryTarget, McpServer, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const ID = '00000000-0000-4000-8000-000000000001';
const OTHER = '00000000-0000-4000-8000-000000000002';
const server: McpServer = { ...unjudgedProvenance(),
  id: ID, name: 'docs', enabled: true, address: 'team.docs', description: '', environment: null,
  home: null, org_id: null, imported: null, machine: null, owner: null, revision: 1, spec: { transport: 'stdio' },
  access: { administers: false, permissions: ['mcp.view', 'mcp.use'], through: [] },
};
const setting = (on = false): ResolvedSetting => ({
  key: 'mcp.enabled', value: { [ID]: on }, own: { value: { [OTHER]: false }, revision: 7, locked: false },
  parts: [], source: { scope: 'system', node: null }, writable: true, locked_by: null,
});
const entry = (complete = true): LibraryEntry => ({
  target: { object: { kind: 'mcp_server', id: ID }, version_id: null }, threads: null,
  facts: { subject: { kind: 'resource', object: { kind: 'mcp_server', id: ID } }, as_of: '2026-11-07T00:00:00Z', uses: null,
    dependents_complete: complete, dependents: [
      { relation: 'attachment', count: 1, hidden: 0, until: null, visible: [{ object: { kind: 'environment', id: 'attached' }, name: 'Attached only', relation: 'attachment', until: null }] },
      { relation: 'explicit_giving', count: 2, hidden: 1, until: null, visible: [{ object: { kind: 'topic', id: 'topic' }, name: 'Research', relation: 'explicit_giving', until: null }] },
    ] },
});
const calls = vi.hoisted(() => ({
  rows: vi.fn<() => Promise<ResolvedSetting[]>>(), write: vi.fn<typeof Settings.putSetting>(),
  facts: vi.fn<(targets: readonly LibraryTarget[]) => Promise<LibraryEntry[]>>(),
  servers: vi.fn<() => Promise<McpServer[]>>(), sameReader: vi.fn<() => boolean>(),
}));
vi.mock('@/api/settings', async (original) => ({ ...await original<typeof Settings>(),
  targetSettingsQuery: (target: Settings.SettingsTarget) => ({ queryKey: ['enabled-test', target], queryFn: calls.rows }),
  targetSettingsKey: (target: Settings.SettingsTarget) => ['enabled-test', target],
  putSetting: (...args: Parameters<typeof Settings.putSetting>) => calls.write(...args),
}));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(),
  libraryQuery: (targets: readonly LibraryTarget[]) => ({ queryKey: ['resource-library', targets], queryFn: () => calls.facts(targets) }),
  sameLibraryReader: () => calls.sameReader(),
}));
vi.mock('@/api/resource-tools', () => ({ mcpKey: (id: string) => ['mcp-server', id], mcpServersQuery: { queryKey: ['mcp-servers'], queryFn: () => calls.servers() } }));
vi.mock('@/features/settings/PlacePicker', () => ({ PlacePicker: ({ onChange, includeThreads }: PlacePickerProps) => <button type="button" onClick={() => onChange({ id: 'topic', kind: 'topic', color: null, folder: false, path: 'Acme / Research' })}>{includeThreads ? 'Choose a place or thread' : 'Choose a place'}</button> }));
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject }: { subject: { server?: { name: string }; environment?: { name: string } } }) => <span>{subject.server?.name ?? subject.environment?.name}</span> }));
beforeEach(() => {
  vi.clearAllMocks(); calls.rows.mockResolvedValue([setting()]); calls.write.mockResolvedValue(setting(true));
  calls.facts.mockResolvedValue([entry()]); calls.servers.mockResolvedValue([server]); calls.sameReader.mockReturnValue(true);
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('lists explicit switch locations and writes the chosen place by its native revision in %s', async (locale) => {
  await setLocale(locale);
  show(<McpEnabledIn server={server} />);
  expect(await screen.findByRole('button', { name: 'Research' })).toBeVisible();
  expect(screen.queryByText('Attached only')).toBeNull();
  expect(calls.facts).toHaveBeenCalledWith([{ object: { kind: 'mcp_server', id: ID }, version_id: null }]);
  await userEvent.click(screen.getByRole('button', { name: 'Choose a place or thread' }));
  await userEvent.click(await screen.findByRole('switch', { name: i18n.t('mcpSwitch.use', { name: 'docs' }) }));
  await waitFor(() => expect(calls.write).toHaveBeenCalledOnce());
  expect(calls.write).toHaveBeenCalledWith({ kind: 'node', id: 'topic' }, 'mcp.enabled', { value: { [OTHER]: false, [ID]: true }, expectedRevision: 7 });
});

it('does not present an incomplete empty report as nowhere', async () => {
  const partial = entry(false); partial.facts.dependents = [];
  calls.facts.mockResolvedValue([partial]);
  show(<McpEnabledIn server={server} />);
  await waitFor(() => expect(calls.facts).toHaveBeenCalledOnce());
  const section = screen.getByRole('region', { name: i18n.t('mcpSwitch.where') });
  expect(within(section).queryByText(i18n.t('mcpSwitch.nowhere'))).toBeNull();
  expect(await within(section).findByText('–')).toBeVisible();
});

it('keeps a server of one environment on that environment alone', async () => {
  show(<McpEnabledIn server={{ ...server, environment: { id: 'own-env', name: 'Workspace', address: 'workspace', owner: ADA, placement: 'connected', status: 'running', ports_exposed: false }, access: { ...server.access, permissions: ['environment.use'] } }} />);
  await userEvent.click(await screen.findByRole('switch'));
  expect(calls.write).toHaveBeenCalledWith({ kind: 'environment', id: 'own-env' }, 'mcp.enabled', expect.any(Object));
  expect(screen.queryByRole('button', { name: 'Choose a place or thread' })).toBeNull();
});

it('acknowledges a restricted accepted write and waits for readback without repeating it', async () => {
  calls.write.mockRejectedValue(new ReceiptAcknowledged({ projection: 'receipt', changed: true, node_id: 'topic', key: 'mcp.enabled', own_revision: 8 }, 200, 'request'));
  show(<McpSwitchAt target={{ kind: 'node', id: 'topic' }} server={server} />);
  await userEvent.click(await screen.findByRole('switch'));
  expect(await screen.findByText(i18n.t('mcpSwitch.accepted'))).toBeVisible();
  expect(screen.getByRole('switch')).toBeDisabled();
  expect(screen.getByRole('switch')).not.toBeChecked();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(calls.write).toHaveBeenCalledOnce();
});

it('does not publish a held old-reader mutation result', async () => {
  let finish: ((row: ResolvedSetting) => void) | undefined;
  calls.write.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  show(<McpSwitchAt target={{ kind: 'node', id: 'topic' }} server={server} />);
  await userEvent.click(await screen.findByRole('switch'));
  const settle = finish; if (settle === undefined) throw new Error('The setting request did not start');
  calls.sameReader.mockReturnValue(false);
  await act(async () => { settle(setting(true)); });
  expect(screen.getByRole('switch')).not.toBeChecked();
  expect(calls.rows).toHaveBeenCalledOnce();
});

it('offers only usable servers and blocks imported servers on a different machine', async () => {
  calls.servers.mockResolvedValue([
    { ...server, machine: { id: 'other-machine', name: 'Other', online: true, placement: 'connected', platform_os: 'linux' }, imported: { source: 'codex', name: 'docs', by: ADA, directory: null, scope: 'user', at: '2026-11-07T00:00:00Z', stale: false } },
    { ...server, id: OTHER, name: 'Private server', access: { ...server.access, permissions: ['mcp.view'] } },
  ]);
  show(<EnvironmentMcpChoices environment={{ id: 'env', machine: { id: 'this-machine', name: 'Here', online: true, placement: 'connected', platform_os: 'linux' } }} />);
  const control = await screen.findByRole('switch');
  expect(control).toBeDisabled();
  expect(screen.getByText(i18n.t('mcpWords.reasons.otherMachine'))).toBeVisible();
  expect(screen.queryByText('Private server')).toBeNull();
  expect(calls.write).not.toHaveBeenCalled();
});
