import { unjudgedProvenance } from '../doubles/resource-grouping';
/** Switching an MCP server off says at once how many threads' calls it refuses now; nothing waits for that count. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ResourceApi from '@/api/resource-tools';
import { EditMcp } from '@/features/resources/mcp/Edit';
import type { FactTarget, McpServer, ResourceApplicationGroup, ResourceImpact } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ update: vi.fn<typeof ResourceApi.updateMcp>(), read: vi.fn<(target: FactTarget) => Promise<ResourceImpact>>() }));
vi.mock('@/api/resource-tools', () => ({ createMcp: vi.fn(), updateMcp: calls.update, mcpKey: (id: string) => ['mcp-server', id] }));
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['mcp-switch-off', target], queryFn: () => calls.read(target), retry: false }) }));

const server: McpServer = { ...unjudgedProvenance(), id: 'server', name: 'search-tools', description: 'Search the project', address: 'team.search-tools', org_id: 'org', owner: null,
  home: { id: 'org', kind: 'org', name: 'Workshop', redacted: false }, machine: null, environment: null, imported: null, enabled: true, revision: 7,
  access: { permissions: ['mcp.view', 'mcp.manage'], administers: false, participating: true, admin_only: [], through: [] },
  spec: { transport: 'http', url: 'https://tools.example.test/mcp', secret_env: [], env: {}, tools_allow: [], tools_deny: [], startup_timeout_secs: 45, working_dir: 'home', requires: { commands: [], capabilities: [] } } };
const target: FactTarget = { object: { kind: 'mcp_server', id: server.id }, model_id: null };
const thread = { object: { kind: 'thread' as const, id: '00000000-0000-4000-8000-000000000201' }, name: 'Index the docs', relation: 'binding' as const, until: null };
const group = (when: ResourceApplicationGroup['when'], count: number): ResourceApplicationGroup => ({ when, object_kind: 'thread', count, visible: [thread], hidden: count - 1, apply: null, apply_targets: [] });
function impact(now: number): ResourceImpact {
  const empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  return { target, name: server.name, stopping: false, application_complete: true, applications: [group('now', now), group('before_next_turn', 3)],
    activity: { running: empty, waiting: empty, stopped: empty },
    facts: { subject: { kind: 'resource', object: target.object }, as_of: '2026-11-26T00:00:00Z', dependents: [], dependents_complete: true, uses: null } };
}
function pending(): (value: ResourceImpact) => void {
  let ready: ((value: ResourceImpact) => void) | undefined;
  calls.read.mockImplementation(() => new Promise<ResourceImpact>((resolve) => { ready = resolve; }));
  return (value) => { ready?.(value); };
}
function draw(onClose = vi.fn()) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  show(<QueryClientProvider client={cache}><EditMcp server={server} onClose={onClose} /></QueryClientProvider>);
  return onClose;
}

beforeEach(() => { calls.update.mockResolvedValue({ ...server, enabled: false, revision: 8 }); });
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`switching a server off says how many threads' calls are refused now, and saves before that count arrives (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup();
    const resolve = pending();
    const close = draw();
    await user.click(screen.getByRole('switch', { name: i18n.t('resourceDetails.enabled') }));
    await waitFor(() => expect(calls.read).toHaveBeenCalledWith(target));
    expect(screen.getByRole('button', { name: i18n.t('resources.edit.save') })).toBeEnabled();
    act(() => resolve(impact(2)));
    expect(await screen.findByText(i18n.t('resourceImpact.application.nowThreads', { count: 2, number: '2' }))).toBeVisible();
    // When it lands later is on Show, not in the line.
    expect(screen.queryByText(i18n.t('resourceImpact.application.nextTurnThreads', { count: 3, number: '3' }))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceImpact.showUses') }));
    expect(screen.getByText(i18n.t('resourceImpact.application.nextTurnThreads', { count: 3, number: '3' }))).toBeVisible();
    expect(screen.getAllByText(i18n.t('resourceImpact.application.nowThreads', { count: 2, number: '2' }))).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(server.id, expect.objectContaining({ expected_revision: 7, enabled: false })));
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
  }, 15_000);
}

it('a switch-off that refuses no call now says nothing more than Show uses', async () => {
  const user = userEvent.setup();
  calls.read.mockResolvedValue(impact(0));
  draw();
  await user.click(screen.getByRole('switch', { name: i18n.t('resourceDetails.enabled') }));
  await waitFor(() => expect(calls.read).toHaveBeenCalledWith(target));
  expect(screen.getByRole('button', { name: i18n.t('resourceImpact.showUses') })).toBeVisible();
  expect(screen.queryByText(/Calls refused now/)).not.toBeInTheDocument();
});

it('a renamed server reads where it applies only on Show', async () => {
  const user = userEvent.setup();
  calls.read.mockResolvedValue(impact(2));
  draw();
  const name = screen.getByRole('textbox', { name: i18n.t('resources.edit.name') });
  await user.clear(name); await user.type(name, 'search-tools-2');
  expect(screen.getByRole('button', { name: i18n.t('resourceImpact.showUses') })).toBeVisible();
  expect(calls.read).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('resources.edit.save') }));
  await waitFor(() => expect(calls.update).toHaveBeenCalledOnce());
  expect(calls.read).not.toHaveBeenCalled();
});
