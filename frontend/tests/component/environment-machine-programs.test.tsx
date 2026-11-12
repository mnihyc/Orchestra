import type * as ResourceToolsApi from '@/api/resource-tools';
import { resourceRows, type ResourceRows } from '@/api/listed';
import type * as RouterModule from '@tanstack/react-router';
/** The real Environment Tools entrance shows and changes only current machine-program consent. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as EnvironmentApi from '@/api/environments';
import { environmentKey } from '@/api/environments';
import { ApiError } from '@/api/client';
import { EnvironmentScreen, type EnvironmentTab } from '@/features/resources/environments/Environment';
import type { Environment, EnvironmentMachineProgram, EnvironmentMcp, EnvironmentUpdateBody, McpServer } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, GRACE } from '../doubles/chat-rows';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ available: vi.fn<() => Promise<ResourceRows<McpServer>>>(), read: vi.fn<() => Promise<Environment>>(), mcp: vi.fn<() => Promise<EnvironmentMcp>>(), write: vi.fn<(id: string, body: EnvironmentUpdateBody) => Promise<Environment>>() }));
vi.mock('@/api/environments', async (load) => {
  const actual = await load<typeof EnvironmentApi>();
  return { ...actual, environmentQuery: (id: string) => ({ queryKey: actual.environmentKey(id), queryFn: calls.read, staleTime: Infinity }),
    environmentMcpQuery: (id: string) => ({ queryKey: actual.environmentMcpKey(id), queryFn: calls.mcp, staleTime: Infinity }), updateEnvironment: calls.write };
});
vi.mock('@/api/resource-tools', async (load) => {
  const actual = await load<typeof ResourceToolsApi>();
  return { ...actual, mcpServersQuery: { ...actual.mcpServersQuery, queryKey: ['consent-mcp-definitions'], staleTime: Infinity, queryFn: () => calls.available() } };
});
vi.mock('@tanstack/react-router', async (loadRouter) => ({ ...await loadRouter<typeof RouterModule>(), useNavigate: () => vi.fn(), Link: ({ children, className }: { children: ReactNode; className?: string }) => <a href="#environment" className={className}>{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/resources/environments/acts', () => ({ useEnvironmentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), keepAwake: () => null, dialogs: null }) }));
vi.mock('@/features/resources/environments/AgentReach', () => ({ AgentReach: () => null }));
vi.mock('@/features/resources/agents/ProgramBuild', () => ({ EnvironmentPrograms: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>, PageHeader: ({ title }: { title: ReactNode }) => <header><h1>{title}</h1></header> }));

const program: EnvironmentMachineProgram = { harness_kind: 'codex', path: '/opt/agent bins/codex', version: '0.164.0', approval: null };
const otherDecision = { path: '/opt/other', version: '1.0.0', by: ADA.id, at: '2026-10-20T00:00:00Z' };
const config = { 'exec.offers': ['codex-exec-server'], 'exec.orch_tools': true, 'env.variables': { KEEP_THIS: 'kept' }, 'exec.version_overrides': { other: otherDecision } };
function row(current = program, manages = true): Environment {
  const ownConfig = current.approval === null ? config : { ...config, 'exec.version_overrides': { other: otherDecision,
    [current.harness_kind]: { path: current.path, version: current.version, by: current.approval.by.id, at: current.approval.at } } };
  return { ...detailEnvironment, placement: 'connected', host: null, revision: 3, config: ownConfig, access: { ...detailEnvironment.access, permissions: manages ? ['environment.view', 'environment.manage'] : ['environment.view'] }, exec: { ...detailEnvironment.exec, machine_programs: [current] } };
}
function approved(current = program): EnvironmentMachineProgram { return { ...current, approval: { by: GRACE, at: '2026-10-31T00:00:00Z' } }; }
function draw(initial: EnvironmentTab = 'tools') {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  function Scene() { const [tab, setTab] = useState<EnvironmentTab>(initial); return <EnvironmentScreen environmentId={detailEnvironment.id} tab={tab} onTab={setTab} />; }
  return { cache, ...show(<QueryClientProvider client={cache}><Scene /></QueryClientProvider>) };
}
beforeEach(() => { vi.clearAllMocks(); calls.available.mockResolvedValue(resourceRows<McpServer>([])); calls.read.mockResolvedValue(row()); calls.mcp.mockResolvedValue({ applies: 'next_turn', found: [], import: { allowed: false, reason: 'policy' }, servers: [] }); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`mounts the current Codex row only in the actual Tools entry (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); draw('overview');
    await screen.findByRole('heading', { name: detailEnvironment.name, level: 1 });
    expect(screen.queryByRole('switch')).not.toBeInTheDocument(); expect(calls.mcp).not.toHaveBeenCalled();
    await user.click(screen.getByRole('tab', { name: i18n.t('resourceDetails.layout.tools') }));
    expect(await screen.findByRole('heading', { name: locale === 'en' ? 'Codex tools' : 'Codex 工具' })).toBeVisible();
    expect(screen.getByText(program.version)).toBeVisible(); expect(screen.getByText('…/agent bins/codex')).toBeVisible();
    expect(screen.getByRole('switch', { name: locale === 'en' ? 'Any agent version' : '任意代理版本' })).not.toBeChecked();
    await waitFor(() => expect(calls.mcp).toHaveBeenCalledTimes(1)); expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.write).not.toHaveBeenCalled();
  });
}

it('preserves the full config, sends no invented attribution, and shows the returned approver', async () => {
  const current = row(), acknowledged = { ...current, revision: 4, config: { ...config, 'exec.version_overrides': { other: otherDecision, codex: { path: program.path, version: program.version, by: GRACE.id, at: '2026-10-31T00:00:00Z' } } }, exec: { ...current.exec, machine_programs: [approved()] } };
  calls.write.mockResolvedValue(acknowledged); const user = userEvent.setup(), view = draw();
  await user.click(await screen.findByRole('switch', { name: 'Any agent version' }));
  expect(calls.write).toHaveBeenCalledExactlyOnceWith(current.id, { expected_revision: current.revision, config: { ...config, 'exec.version_overrides': { other: otherDecision, codex: { path: program.path, version: program.version } } } });
  expect(await screen.findByRole('switch', { name: `Any agent version · ${GRACE.display_name}` })).toBeChecked();
  expect(view.cache.getQueryData(environmentKey(current.id))).toEqual(acknowledged);
  expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.mcp).toHaveBeenCalledTimes(1);
});

it('removes only the displayed kind when version matching is restored', async () => {
  const current = { ...row(approved()), config: { ...config, 'exec.version_overrides': { other: otherDecision, codex: { path: program.path, version: program.version, by: GRACE.id, at: '2026-10-31T00:00:00Z' } } } };
  calls.read.mockResolvedValue(current); calls.write.mockResolvedValue({ ...row(), revision: 4 }); const user = userEvent.setup(); draw();
  await user.click(await screen.findByRole('switch', { name: `Any agent version · ${GRACE.display_name}` }));
  expect(calls.write).toHaveBeenCalledExactlyOnceWith(current.id, { expected_revision: current.revision, config });
  expect(await screen.findByRole('switch', { name: 'Any agent version' })).not.toBeChecked();
});

it('lets a viewer read current consent without a write', async () => {
  calls.read.mockResolvedValue(row(approved(), false)); const user = userEvent.setup(); draw();
  const control = await screen.findByRole('switch', { name: `Any agent version · ${GRACE.display_name}` });
  expect(control).toBeChecked(); expect(control).toBeDisabled(); await user.click(control); expect(calls.write).not.toHaveBeenCalled();
});

it('does not revive a stale path/version decision from raw config', async () => {
  const current = row({ ...program, version: 'unknown' }); current.config = { ...config, 'exec.version_overrides': { codex: { path: program.path, version: '0.160.0', by: GRACE.id, at: '2026-10-20T00:00:00Z' } } };
  calls.read.mockResolvedValue(current); draw();
  expect(await screen.findByText('Version unknown', { exact: true })).toBeVisible();
  expect(screen.getByRole('switch', { name: 'Any agent version' })).not.toBeChecked();
  expect(screen.queryByText(GRACE.display_name)).not.toBeInTheDocument(); expect(calls.write).not.toHaveBeenCalled();
});

it('keeps a historical consent active when its writer has no current name', async () => {
  calls.read.mockResolvedValue(row({ ...program, approval: { by: { ...GRACE, display_name: '', username: '' }, at: '2026-10-20T00:00:00Z' } })); draw();
  expect(await screen.findByRole('switch', { name: 'Any agent version · Name not available' })).toBeChecked();
  expect(calls.write).not.toHaveBeenCalled();
});

it('keeps MCP available without inventing a machine executor when no program projection is present', async () => {
  calls.read.mockResolvedValue({ ...detailEnvironment, config }); draw();
  expect(await screen.findByText(i18n.t('mcpServers.empty'), { exact: true })).toBeVisible();
  expect(screen.queryByRole('heading', { name: 'Codex tools' })).not.toBeInTheDocument(); expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  expect(calls.write).not.toHaveBeenCalled(); expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.mcp).toHaveBeenCalledTimes(1);
});

it('shows the server refusal and keeps the unapproved state when Manage is denied', async () => {
  calls.write.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403,
    title: 'Forbidden', detail: 'Environment Manage is required.', request_id: 'environment-program-consent' }, null));
  const user = userEvent.setup(); draw();
  await user.click(await screen.findByRole('switch', { name: 'Any agent version' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('switch', { name: 'Any agent version' })).not.toBeChecked();
  expect(calls.write).toHaveBeenCalledTimes(1); expect(calls.read).toHaveBeenCalledTimes(1); expect(calls.mcp).toHaveBeenCalledTimes(1); expect(calls.available).toHaveBeenCalledOnce();
});

it('keeps keyboard focus and one write while the server records consent', async () => {
  let finish: ((updated: Environment) => void) | undefined;
  calls.write.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const user = userEvent.setup(); draw();
  const control = await screen.findByRole('switch', { name: 'Any agent version' });
  await user.click(control);
  expect(control).toHaveFocus(); expect(control).toBeChecked();
  await user.keyboard(' ');
  expect(control).toHaveFocus(); expect(calls.write).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('switch', { name: `Any agent version · ${GRACE.display_name}` })).not.toBeInTheDocument();
  if (finish === undefined) throw new Error('The actual Tools control must have sent one consent write');
  const complete = finish;
  await act(async () => { complete({ ...row(approved()), revision: 4 }); await Promise.resolve(); });
  expect(await screen.findByRole('switch', { name: `Any agent version · ${GRACE.display_name}` })).toHaveFocus();
});
