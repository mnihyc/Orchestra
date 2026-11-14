/** The program an agent profile reports is shown with Orch's own stored check of exactly that kind and version: unverified while no check
 * has finished, checking while one runs, checked when it passed, and when it failed the functions that are off because of it, by the
 * component and the words the Agent programs page uses for a build. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as ResourcesApi from '@/api/resources';
import { AgentScreen } from '@/features/resources/agents/Profile';
import { storedBuild } from '@/features/resources/agents/buildChecks';
import type { HarnessBuild } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { detailAgent } from './resource-detail-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ builds: vi.fn<() => Promise<HarnessBuild[]>>() }));
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof ResourcesApi>();
  return { ...actual,
    agentQuery: (id: string) => ({ queryKey: actual.agentKey(id), queryFn: () => Promise.resolve(record.current), staleTime: Infinity }),
    harnessKindsQuery: { queryKey: ['program-check-kinds'], queryFn: () => Promise.resolve([]), staleTime: Infinity },
  };
});
vi.mock('@/api/harnesses', async (load) => ({ ...await load<object>(), programBuildsQuery: { queryKey: ['program-check-builds'], queryFn: () => calls.builds(), staleTime: Infinity } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useNavigate: () => vi.fn(), useLocation: () => ({ pathname: '/resources/example', searchStr: '' }),
  Link: ({ children, className }: { children: ReactNode; className?: string }) => <a className={className} href="#fixture">{children}</a> }));
vi.mock('@/features/resources/trail', () => ({ useResourceHeading: () => undefined }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/settings/sourceAddress', () => ({ useSourceOrgAddress: () => 'workshop' }));
vi.mock('@/features/resources/agents/acts', () => ({ useAgentActs: () => ({ page: () => ({ primary: null, edit: null, menu: [] }), dialogs: null }) }));
vi.mock('@/shell/Page', async (load) => ({ ...await load<object>(), Page: ({ children }: { children: ReactNode }) => <main>{children}</main>,
  PageHeader: ({ title, lead, facts, actions }: { title: string; lead?: ReactNode; facts?: ReactNode[]; actions?: ReactNode }) => <header><h1>{title}</h1>{lead}<div>{facts}</div>{actions}</header> }));

/** The profile of the fixture reports Codex 1.2.3 from a machine of its own. */
const record = { current: detailAgent };
const reported = { kind: 'codex', version: '1.2.3' };
const build = (more: Partial<HarnessBuild> = {}): HarnessBuild => ({ ...reported, os: 'linux', arch: 'x86_64', url: 'https://example.com/codex', sha256: 'a'.repeat(64), is_default: false,
  source: 'channel', status: 'untested', ...more });
const check = (more: Partial<NonNullable<HarnessBuild['check']>> = {}): NonNullable<HarnessBuild['check']> => ({ at: '2026-11-01T00:00:00.000Z', machine: 'Check host', missing: [], off: [], error: null, stage: 'complete', cases: [], ...more });

function draw() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  return show(<QueryClientProvider client={client}><AgentScreen agentId={detailAgent.id} tab="overview" onTab={vi.fn()} /></QueryClientProvider>);
}
const row = async () => {
  await screen.findByRole('heading', { level: 1, name: detailAgent.name });
  await waitFor(() => expect(document.querySelector('[data-program="codex"]')).not.toBeNull());
  const found = document.querySelector<HTMLElement>('[data-program="codex"]');
  if (found === null) throw new Error('The overview shows no row for the program the machine reported');
  return found;
};

beforeEach(() => { vi.clearAllMocks(); record.current = detailAgent; calls.builds.mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) describe(`the program check of an agent profile (${locale})`, () => {
  const t = (key: string, options?: Record<string, unknown>) => i18n.getFixedT(locale)(key, options);

  it('says that no check is known while no build of that version is stored, and not that a check is unverified', async () => {
    await setLocale(locale);
    calls.builds.mockResolvedValue([build({ version: '9.9.9', status: 'checked', check: check() }), build({ kind: 'claude-code', status: 'failed', check: check({ off: ['import_native'] }) })]);
    draw();
    const found = await row();
    expect(within(found).getByText(t('programVersions.unknownCheck'))).toBeVisible();
    expect(within(found).queryByText(t('admin.harnesses.status.checked'))).not.toBeInTheDocument();
  });

  it('shows a build nobody has checked as unverified', async () => {
    await setLocale(locale);
    calls.builds.mockResolvedValue([build({ status: 'untested' })]);
    draw();
    const found = await row();
    expect(await within(found).findByText(t('admin.harnesses.status.untested'))).toBeVisible();
    expect(within(found).queryByText(t('programVersions.unknownCheck'))).not.toBeInTheDocument();
  });

  it('shows a check that runs as checking, in its stage', async () => {
    await setLocale(locale);
    calls.builds.mockResolvedValue([build({ status: 'checking', check: check({ stage: 'conformance' }) })]);
    draw();
    const found = await row();
    expect(await within(found).findByText(t('admin.harnesses.checking.conformance'))).toBeVisible();
  });

  it('shows a check that passed as checked, with nothing off', async () => {
    await setLocale(locale);
    calls.builds.mockResolvedValue([build({ status: 'checked', check: check() })]);
    draw();
    const found = await row();
    expect(await within(found).findByText(t('admin.harnesses.status.checked'))).toBeVisible();
    expect(found.querySelector('[data-build-feature]')).toBeNull();
  });

  it('shows a check that failed with the functions that are switched off because of it, in the words of the Agent programs page', async () => {
    await setLocale(locale);
    calls.builds.mockResolvedValue([build({ status: 'failed', check: check({ off: ['import_native', 'steer'] }) })]);
    draw();
    const found = await row();
    const feature = (key: string) => t(`programVersions.disabledFeatures.${key}`);
    expect(await within(found).findByRole('button', { name: t('programVersions.offInBuild', { feature: feature('nativeImport') }) })).toBeVisible();
    expect([...found.querySelectorAll('[data-build-feature]')].map((line) => line.getAttribute('data-build-feature'))).toEqual(['import_native', 'steer']);
    const line = found.querySelector<HTMLElement>('[data-build-feature="import_native"]');
    expect(line).toHaveTextContent(feature('nativeImport'));
    expect(line).toHaveTextContent('1.2.3');
  });
});

describe('which of the stored builds of a version is the one shown', () => {
  const failed = build({ status: 'failed', os: 'linux', arch: 'aarch64' });
  const checked = build({ status: 'checked', os: 'linux', arch: 'x86_64' });
  const windows = build({ status: 'untested', os: 'windows', arch: 'x86_64' });

  it('is exactly the kind and the version, and none when there is none', () => {
    expect(storedBuild([checked, build({ version: '1.2.4', status: 'failed' }), build({ kind: 'claude-code', status: 'failed' })], 'codex', '1.2.3')).toBe(checked);
    expect(storedBuild([], 'codex', '1.2.3')).toBeNull();
    expect(storedBuild(undefined, 'codex', '1.2.3')).toBeNull();
  });

  it('is, among the builds of the system of the machine that reported it, the one known to work least', () => {
    expect(storedBuild([checked, failed, windows], 'codex', '1.2.3', 'linux')).toBe(failed);
    expect(storedBuild([checked, windows], 'codex', '1.2.3', 'windows')).toBe(windows);
    // A machine whose system has no build of it is told of the weakest of the others, and not of nothing.
    expect(storedBuild([checked, failed], 'codex', '1.2.3', 'macos')).toBe(failed);
    expect(storedBuild([checked, build({ status: 'checking' }), build({ status: 'untested' })], 'codex', '1.2.3')?.status).toBe('checking');
  });
});
