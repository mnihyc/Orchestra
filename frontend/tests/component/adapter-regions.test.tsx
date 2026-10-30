/** Program tabs retain build metadata and declared names; generic inspector previews stay bounded. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Harnesses } from '@/features/admin/Harnesses';
import type { AdminSearch } from '@/features/admin/search';
import { featureName } from '@/features/resources/agents/buildChecks';
import type { HarnessBuild, ProgramDetail } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { JsonTree } from '@/ui/JsonTree';
import { show } from './support';
import { agentDescriptor } from '../doubles/agent-descriptor';

const server = vi.hoisted(() => ({ detail: vi.fn<() => Promise<ProgramDetail>>(), builds: vi.fn<() => Promise<HarnessBuild[]>>(), legacy: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/programAdmin', async (original) => ({ ...await original<object>(),
  adminProgramQuery: (kind: string) => ({ queryKey: ['admin', 'programs', kind, 'detail'], queryFn: server.detail }),
  adminProgramBuildsQuery: (kind: string, cursor: string | undefined) => ({ queryKey: ['admin', 'programs', kind, 'builds', cursor ?? null],
    queryFn: async () => ({ items: await server.builds(), next_cursor: null }) }),
}));
vi.mock('@/api/harnesses', async (original) => ({ ...await original<object>(),
  programBuildsQuery: { queryKey: ['program-builds'], queryFn: server.legacy },
  harnessUsageQuery: { queryKey: ['admin', 'harness-usage'], queryFn: () => Promise.resolve([]) },
}));
vi.mock('@/features/admin/SettingRows', () => ({
  useSystemSettings: () => ({ settings: { isPending: false, isError: false }, find: () => ({ value: {} }) }),
  useSettingChange: () => ({ writable: true, pending: false, save: { variables: undefined, mutate: vi.fn() } }),
  SettingChangeStatus: () => null,
}));
const name = 'Research adapter';
const kind = 'research_wire_kind';
const descriptor = agentDescriptor(name, { kind,
  ...Object.fromEntries(Array.from({ length: 12 }, (_, index) => [`field${index}`, { value: index }])) }, { fork_native: true });
const build: HarnessBuild = { kind, version: '2.0', os: 'linux', arch: 'x86_64', url: 'https://downloads.example/adapter',
  sha256: 'ab'.repeat(32), is_default: true, source: 'admin', status: 'untested' };
function detail(): ProgramDetail {
  return { summary: { harness: { kind, name, names: {} }, builtin: false, protocol: 'acp', profiles: 1, machines: 1,
    platforms: [], channel: 'none', check: 'untested' }, descriptor, program: null, versions: {}, resolved_builds: [build],
    channels: { interval_hours: 0, last_read_at: null, reason: 'disabled', requested: [] },
    capabilities: ['conversation', 'tools', 'sessions', 'usage'].map((id) => ({ id,
      title: { en: id, 'zh-CN': id }, items: [{ id: `${id}.extension`, title: { en: `${id} extension`, 'zh-CN': `${id} 扩展` }, available: true, reason: null }] })) };
}
function ProgramView() {
  const [search, setSearch] = useState<AdminSearch>({ program: kind });
  return <Harnesses search={search} onSearch={(patch) => setSearch((held) => ({ ...held, ...patch }))} />;
}
beforeEach(async () => {
  await setLocale('en');
  class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
  vi.stubGlobal('ResizeObserver', Observer);
  vi.clearAllMocks();
  server.detail.mockResolvedValue(detail());
  server.builds.mockResolvedValue([build]);
});
afterEach(async () => { expect(server.legacy).not.toHaveBeenCalled(); cleanup(); vi.unstubAllGlobals(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps build metadata with its named row and opens the grouped capabilities in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<ProgramView />);
  const builds = await screen.findByRole('table', { name: i18n.t('admin.harnesses.buildsTitle') });
  const support = builds.closest('[data-scroll-allow="table"]');
  expect(support).toHaveAttribute('role', 'region');
  expect(support).toHaveAttribute('tabindex', '0');
  const line = within(builds).getByRole('row', { name: new RegExp(name) });
  expect(within(line).getAllByText(name)).toHaveLength(1);
  expect(within(line).queryByText(kind)).not.toBeInTheDocument();
  expect(within(line).getByText(`${i18n.t(`admin.harnesses.systems.${build.os}`)} · ${build.arch}`)).toBeVisible();
  expect(within(line).getByText(i18n.t('admin.harnesses.sources.admin'))).toBeVisible();
  expect(within(line).getByRole('button', { name: i18n.t('admin.harnesses.copySha', { kind: name, version: build.version }) })).toBeVisible();
  await user.click(screen.getByRole('tab', { name: i18n.t('programList.capabilities') }));
  for (const group of ['conversation', 'tools', 'sessions', 'usage']) {
    expect(screen.getByRole('region', { name: group })).toHaveTextContent(locale === 'en' ? `${group} extension` : `${group} 扩展`);
  }
  expect(screen.queryByRole('table')).not.toBeInTheDocument();
  expect(screen.queryByText('field11:')).not.toBeInTheDocument();
});

// The approved program tabs replace the raw-descriptor comparison. The shared inspector's
// existing preview assertions stay here independently of that removed Admin presentation.
it.each(['en', 'zh-CN'] as const)('keeps a large descriptor folded in the shared inspector in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const label = locale === 'en' ? 'Research adapter metadata' : '研究代理元数据';
  show(<JsonTree value={descriptor} label={label} preview />);
  const preview = screen.getByRole('region', { name: label });
  expect(preview).toHaveAttribute('tabindex', '0');
  expect(screen.queryByText('field11:')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('ui.preview.showAll') }));
  expect(screen.getByText('field11:')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('ui.json.collapse', { name: label }) })).toHaveAttribute('aria-expanded', 'true');
});

it('shows how far each build is known to work and lets an admin choose which build places get', async () => {
  const checked: HarnessBuild = { ...build, version: '2.1', source: 'channel', is_default: false, status: 'failed',
    check: { at: '2026-10-23T12:00:00Z', machine: 'host-2', missing: ['turn/start'], off: ['plan_mode'], error: null, stage: 'complete', cases: [] } };
  server.builds.mockResolvedValue([build, checked]);
  show(<ProgramView />);
  const builds = await screen.findByRole('table', { name: i18n.t('admin.harnesses.buildsTitle') });
  expect(within(builds).getByText(i18n.t('admin.harnesses.status.untested'))).toBeVisible();
  // A build names a disabled feature before its overall failure status.
  expect(within(builds).getByText(i18n.t('programVersions.offInBuild', { feature: featureName('plan_mode', i18n.t) }))).toBeVisible();
  expect(within(builds).getByText(i18n.t('admin.harnesses.sources.channel'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('admin.harnesses.readChannels') })).toBeVisible();
  await userEvent.click(screen.getByRole('tab', { name: i18n.t('programList.settings') }));
  const choice = await screen.findByRole('combobox', { name });
  expect(choice).toHaveTextContent(i18n.t('choiceCatalog.harnessVersion.shipped.name'));
});

it('uses a program’s supplied Chinese name throughout the management view', async () => {
  await setLocale('zh-CN');
  const named = detail();
  named.summary.harness.names = { 'zh-CN': '研究助理' };
  server.detail.mockResolvedValue(named);
  show(<ProgramView />);
  const builds = await screen.findByRole('table', { name: i18n.t('admin.harnesses.buildsTitle') });
  expect(within(builds).getByText('研究助理')).toBeVisible();
  expect(within(builds).queryByText(name)).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: new RegExp('研究助理') })).toBeVisible();
  await userEvent.click(screen.getByRole('tab', { name: i18n.t('programList.settings') }));
  expect(await screen.findByRole('combobox', { name: '研究助理' })).toBeVisible();
  expect(screen.queryByText(name)).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('uses the registry catalogue in the management build table in %s', async (locale) => {
  await setLocale(locale);
  const checks = { cases: [{ name: 'extension.case', title: { en: 'Checks research handoffs', 'zh-CN': '检查研究交接' }, feature: 'extension.handoff' }],
    features: [{ id: 'extension.handoff', title: { en: 'research handoffs', 'zh-CN': '研究交接' } }], runner: { en: 'Returns a report', 'zh-CN': '返回报告' } };
  const program = { builtin: false, vendor: null, protocol: 'acp', sources: { channels: [], exact_fetch: false }, checks, carry: 'handoff' as const, executors: [], files: { instruction_files: [], workspace_config: [] }, colocated: true, privacy: [] };
  const named = detail();
  named.summary.harness.names = { 'zh-CN': '研究助理' };
  named.program = program;
  const failed: HarnessBuild = { ...build, status: 'failed', check: { at: '2026-10-23T12:00:00Z', machine: 'host-2', missing: [], off: ['extension.handoff'], error: null, stage: 'complete', cases: [{ name: 'extension.case', passed: false, feature: 'extension.handoff', error: 'Handoff refused', elapsed_ms: 12 }] } };
  server.detail.mockResolvedValue(named); server.builds.mockResolvedValue([failed]);
  show(<ProgramView />);
  const builds = await screen.findByRole('table', { name: i18n.t('admin.harnesses.buildsTitle') });
  const label = i18n.t('programVersions.offInBuild', { feature: locale === 'zh-CN' ? '研究交接' : 'research handoffs' });
  await userEvent.click(await within(builds).findByRole('button', { name: label }));
  const card = screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') });
  expect(card).toHaveTextContent(locale === 'zh-CN' ? '检查研究交接' : 'Checks research handoffs');
  expect(card).toHaveTextContent('Handoff refused');
  expect(card).not.toHaveTextContent('extension.case');
});

it.each(['en', 'zh-CN'] as const)('labels mirror checksum authority independently of compatibility in %s', async (locale) => {
  await setLocale(locale);
  server.builds.mockResolvedValue(['untested', 'checking', 'checked', 'failed'].map((status, index) => ({
    ...build, source: 'mirror', version: `3.${index}`, status: status as HarnessBuild['status'], is_default: false,
  })));
  show(<ProgramView />);
  const table = await screen.findByRole('table', { name: i18n.t('admin.harnesses.buildsTitle') });
  const rows = within(table).getAllByRole('row').slice(1);
  expect(rows).toHaveLength(4);
  for (const row of rows) {
    expect(within(row).getByText(i18n.t('admin.harnesses.sources.mirror'))).toBeVisible();
  }
});
