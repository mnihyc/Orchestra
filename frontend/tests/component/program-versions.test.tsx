/** Program policies and reviewed batches preserve version direction, checks and running-turn choices. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as HarnessesApi from '@/api/harnesses';
import type * as ChangesApi from '@/api/changes';
import type * as SettingsApi from '@/api/settings';
import type * as ResourcesApi from '@/api/resources';
import { LiveQueryClient } from '@/api/query';
import { ApiError } from '@/api/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { HarnessVersionsChoice, VersionChoiceControl, resolvedBuild, PROGRAM_VERSIONS_KEY } from '@/features/settings/HarnessVersionsChoice';
import { SettingRow as ScopedSettingRow } from '@/features/settings/SettingRow';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { withGroup } from '@/features/settings/settingsModel';
import { localizedSetting } from '@/features/settings/settingText';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { HarnessVersions } from '@/features/admin/HarnessVersions';
import { ProgramBuild } from '@/features/resources/agents/ProgramBuild';
import { RuntimeChange } from '@/features/resources/TechnicalValue';
import { UpdateDialog } from '@/features/fleet/UpdateDialog';
import { PlanSwitch } from '@/features/composer/PlanSwitch';
import type { EnvironmentUpdateBatchPlan, HarnessBuild, HarnessBuildUsage, HarnessKindInfo, Job, ProviderRuntimeUpdateBatchPreflightReply, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { detailEnvironment, detailUnitProvider } from './resource-detail-fixtures';
import { ADA } from '../doubles/chat-rows';
import { failedStart } from './failed-start-fixtures';
import { show } from './support';
import { agentDescriptor } from '../doubles/agent-descriptor';
import { harnessName } from '../doubles/harness-names';

const state = vi.hoisted(() => ({ kinds: [] as HarnessKindInfo[], builds: [] as HarnessBuild[], usage: [] as HarnessBuildUsage[], plan: null as EnvironmentUpdateBatchPlan | null, fetch: vi.fn<typeof HarnessesApi.fetchProgramVersion>(), write: vi.fn(), settingWrite: vi.fn<typeof SettingsApi.putSetting>(), start: vi.fn<typeof ChangesApi.startEnvironmentUpdates>(), providerRead: vi.fn<typeof ChangesApi.planProviderUpdates>(), providerStart: vi.fn<typeof ChangesApi.startProviderUpdates>() }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(), harnessKindsQuery: { queryKey: ['version-program-kinds'], queryFn: () => Promise.resolve(state.kinds) } }));
vi.mock('@/api/settings', async (load) => ({ ...await load<object>(), putSetting: state.settingWrite }));
vi.mock('@/api/harnesses', async (load) => ({ ...await load<object>(),
  programBuildsQuery: { queryKey: ['program-builds'], queryFn: () => Promise.resolve(state.builds) },
  harnessUsageQuery: { queryKey: ['admin', 'harness-usage'], queryFn: () => Promise.resolve(state.usage) },
  fetchProgramVersion: state.fetch,
}));
vi.mock('@/api/changes', async (load) => ({ ...await load<object>(), planEnvironmentUpdates: () => Promise.resolve(state.plan), startEnvironmentUpdates: state.start, planProviderUpdates: state.providerRead, startProviderUpdates: state.providerStart }));
vi.mock('@/features/admin/SettingRows', () => ({ useSystemSettings: () => ({ find: () => ({ value: {} }) }), useSettingChange: () => ({ writable: true, save: { variables: undefined, mutate: state.write } }), SettingChangeStatus: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/resource-facts/ResourceFacts', () => ({ ResourceChip: ({ subject, children }: { subject: { kind: string; environment?: { name: string }; provider?: { name: string } }; children?: ReactNode }) => <span>{children ?? subject.environment?.name ?? subject.provider?.name}</span> }));
vi.mock('@/features/change/Progress', () => ({ JobProgress: ({ job }: { job: Job }) => <p>{job.state}</p>, committed: () => false }));
vi.mock('@/api/forks', async (load) => ({ ...await load<object>(), jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: () => Promise.resolve({ ...failedStart().job, id }) }) }));

const named = { harness: { kind: 'codex', name: 'Codex', names: {} } };
function build(version: string, status: HarnessBuild['status'] = 'checked'): HarnessBuild {
  return { ...named, kind: 'codex', version, os: 'linux', arch: 'x86_64', url: 'https://build.test/program', sha256: 'c'.repeat(64), is_default: version === '0.162.1', source: 'release', status,
    check: { at: '2026-10-26T00:00:00Z', machine: null, missing: [], off: status === 'failed' ? ['approvals.user_input', 'subagents'] : [], error: null, stage: 'complete', cases: status === 'failed' ? [{ name: 'user_input', passed: false, feature: 'approvals.user_input', error: 'No native question', elapsed_ms: 12 }] : [] } };
}
function plan(): EnvironmentUpdateBatchPlan {
  return { preflight_id: 'batch-plan', expires_at: '2026-10-27T00:00:00Z', executable: 1, estimate: { seconds_low: 1, seconds_high: 2, basis: 'assumed' }, options: { active_turn: 'wait', wait_minutes: 10, verify_timeout_secs: 120, keep_previous: false, stop_processes: true }, notify: { people: [ADA], hidden: 0 },
    plans: [{ preflight_id: 'batch-plan', expires_at: '2026-10-27T00:00:00Z', environment: detailEnvironment, reason: 'update', updates: [{ ...named, what: 'harness', name: 'codex', from: '0.164.0', to: '0.162.1' }], kept: ['Workspace'], lost: ['Running processes'], rollback: 'unit', network: detailEnvironment.network, threads: [{ thread: null, turn_running: true }], blockers: [], options: { active_turn: 'wait', wait_minutes: 10, verify_timeout_secs: 120, keep_previous: false, stop_processes: true }, estimate: { seconds_low: 1, seconds_high: 2, basis: 'assumed' } }],
    skipped: [{ environment: { ...detailEnvironment, id: 'other', name: 'Personal machine' }, reason: 'not_updatable' }],
  };
}
const appearance = useAppearance.getState();
function providerPlan():ProviderRuntimeUpdateBatchPreflightReply {
  const base=plan();
  return {preflight_id:'provider-plan',expires_at:base.expires_at,executable:1,estimate:base.estimate,options:base.options,notify:{people:[],hidden:0},skipped:[],plans:[{
    recovery:false,preflight_id:'provider-plan',expires_at:base.expires_at,provider:detailUnitProvider,reason:'update',updates:base.plans[0]!.updates,kept:['Sign-in'],lost:['Running processes'],rollback:'unit',network:detailEnvironment.network,threads:[],blockers:[],options:base.options,estimate:base.estimate,
  }]};
}
beforeEach(() => { state.kinds = ['codex', 'claude-code'].map((kind) => { const harness = harnessName(kind); return { kind, title: harness.name, builds: [], program: null, harness, descriptor: agentDescriptor(harness.name, { kind }) }; }); state.builds = [build('0.162.1'), build('0.164.0', 'failed')]; state.usage = [{ kind: 'codex', version: '0.162.1', environments: [detailEnvironment], providers: [], update_environments: [detailEnvironment.id], update_providers: [] }]; state.plan = plan(); state.fetch.mockReset(); state.write.mockReset(); state.start.mockReset(); state.start.mockResolvedValue({ batch_id: 'started', jobs: [{ environment: detailEnvironment, job: failedStart().job }] }); });
beforeEach(()=>{state.providerRead.mockReset();state.providerRead.mockResolvedValue(providerPlan());state.providerStart.mockReset();state.providerStart.mockResolvedValue({batch_id:'provider-batch',jobs:[{provider:detailUnitProvider,job:{...failedStart().job,id:'provider-job',environment_id:null,provider_id:detailUnitProvider.id}}]});});
beforeEach(() => { state.settingWrite.mockReset(); });
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); vi.clearAllMocks(); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps resolved status, use counts and filtered units together (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
    const user = userEvent.setup();
    show(<HarnessVersions kinds={['codex']} builds={state.builds} title={() => 'Codex'} />);
    expect(screen.getByLabelText(i18n.t('programVersions.resolved', { name: 'Codex' }))).toHaveTextContent('0.162.1');
    const count = await screen.findByRole('button', { name: i18n.t('programVersions.usage', { version: '0.162.1', environments: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }), providers: i18n.t('programVersions.runtimeCount', { count: 0, number: '0' }) }) });
    await user.click(count);
    const dialog = screen.getByRole('dialog', { name: 'Codex 0.162.1' });
    expect(within(dialog).getByText(detailEnvironment.name)).toBeVisible();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Codex 0.162.1' })).not.toBeInTheDocument();
  });
  it(`reviews a downgrade, skips and a shared interrupt before starting (${locale}/${theme}/${width})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); Object.defineProperty(window, 'innerWidth', { configurable: true, value: width });
    const user = userEvent.setup();
    show(<UpdateDialog targets={{ environments: [detailEnvironment.id, 'other'] }} onClose={vi.fn()} />);
    expect(await screen.findByText(i18n.t('programVersions.change', { name: 'Codex', from: '0.164.0', to: '0.162.1', direction: i18n.t('programVersions.older') }))).toBeVisible();
    expect(screen.getByText(i18n.t('programVersions.skip.not_updatable'))).toBeVisible();
    await user.click(screen.getByRole('radio', { name: i18n.t('changeCompact.turn.interrupt') }));
    expect(screen.getByText(i18n.t('programVersions.stops', { count: 1, number: '1' }))).toBeVisible();
    await user.click(screen.getByRole('checkbox', { name: i18n.t('programVersions.notify', { count: 1, number: '1' }) }));
    await user.click(screen.getByRole('button', { name: i18n.t('programVersions.updateCount', { units: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }) }) }));
    await waitFor(() => expect(state.start).toHaveBeenCalledOnce());
    expect(state.start.mock.calls[0]?.[0]).toEqual({ preflight_id: 'batch-plan', options: { active_turn: 'interrupt', wait_minutes: 10 } });
    expect(state.start.mock.calls[0]?.[2]).toBe(false);
  });
}

for (const locale of ['en', 'zh-CN'] as const) it(`keeps failed features named at their controls without blocking a target (${locale})`, async () => {
  await setLocale(locale);
  const failed = build('0.164.0', 'failed');
  show(<><ProgramBuild program={{ kind: 'codex', version: failed.version, build: failed }} /><PlanSwitch on={false} onChange={vi.fn()} unavailable={i18n.t('programVersions.featureOff', { feature: i18n.t('composer.plan.label'), name: 'Codex', version: failed.version })} /><RuntimeChange change={{ ...named, what: 'harness', name: 'codex', from: '0.162.1', to: '0.164.0' }} /></>);
  expect(screen.getByText(i18n.t('programVersions.featureOff', { feature: i18n.t('programVersions.disabledFeatures.questions'), name: 'Codex', version: failed.version }))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('composer.plan.label') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByText(i18n.t('programVersions.change', { name: 'Codex', from: '0.162.1', to: '0.164.0', direction: i18n.t('programVersions.newer') }))).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses the reported name in program build details (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'zh-CN' ? '研究助理' : 'Research assistant';
  const failed = { ...build('1.2.3', 'failed'), kind: 'research-agent' };
  const named = { harness: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
  const program = { kind: 'research-agent', version: failed.version, build: failed, ...named };
  show(<ProgramBuild program={program} />);
  expect(screen.getByText(name, { exact: true })).toBeVisible();
  expect(screen.getByText(i18n.t('programVersions.featureOff', { feature: i18n.t('programVersions.disabledFeatures.questions'), name, version: failed.version }))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }) }));
  expect(screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') })).toHaveTextContent(`${name} 1.2.3`);
  expect(document.body).not.toHaveTextContent('research-agent');
});

for (const locale of ['en', 'zh-CN'] as const) it(`names a running program even when its catalogue check is unknown (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'zh-CN' ? '研究助理' : 'Research assistant';
  const program = { kind: 'named-external', version: '1.2.3', build: null,
    harness: { kind: 'named-external', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
  show(<ProgramBuild program={program} />);
  expect(screen.getByText(name, { exact: true })).toBeVisible();
  expect(screen.getByText(i18n.t('programVersions.unknownCheck'))).toBeVisible();
  expect(document.body).not.toHaveTextContent(i18n.t('resources.harnessNameUnknown'));
  expect(document.body).not.toHaveTextContent('named-external');
});

it('fetches a typed version before allowing it into the policy and preserves a vendor refusal', async () => {
  const user = userEvent.setup();
  const write = vi.fn();
  state.fetch.mockRejectedValue(new Error('No vendor checksum'));
  const view = show(<VersionChoiceControl kind="codex" choice={{ use: 'exact', version: '0.162.1' }} builds={state.builds} target={{ kind: 'environment', id: 'chosen-environment' }} onChange={write} />);
  const input = screen.getByRole('combobox', { name: i18n.t('admin.harnesses.version') });
  await user.clear(input); await user.type(input, '0.170.0');
  expect(input).toBeInvalid(); expect(write).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('programVersions.fetch') }));
  await waitFor(() => expect(state.fetch).toHaveBeenCalledWith('codex', '0.170.0', { environment_id: 'chosen-environment' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('No vendor checksum'); expect(write).not.toHaveBeenCalled();
  await user.clear(input); await user.type(input, '0.171.0');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument(); expect(write).not.toHaveBeenCalled();
  await user.clear(input); await user.type(input, '0.170.0');
  const checked = build('0.170.0');
  view.rerender(<VersionChoiceControl kind="codex" choice={{ use: 'exact', version: '0.162.1' }} builds={[...state.builds, checked]} target={{ kind: 'environment', id: 'chosen-environment' }} onChange={write} />);
  await user.click(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') }));
  expect(write).toHaveBeenCalledExactlyOnceWith({ use: 'exact', version: '0.170.0' });
});

it('shows the version fault when the API keeps its invalid-request detail generic', async () => {
  const user = userEvent.setup();
  const write = vi.fn();
  state.fetch.mockRejectedValue(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', status: 400, title: 'Invalid request', detail: 'Invalid request', request_id: 'version-fetch', errors: [{ path: 'version', message: 'No vendor checksum' }] }, null));
  show(<VersionChoiceControl kind="codex" choice={{ use: 'exact', version: '0.162.1' }} builds={state.builds} target={{ kind: 'system' }} onChange={write} />);
  const input = screen.getByRole('combobox', { name: i18n.t('admin.harnesses.version') });
  await user.clear(input); await user.type(input, '0.170.0');
  await user.click(screen.getByRole('button', { name: i18n.t('programVersions.fetch') }));
  expect(await screen.findByRole('alert')).toHaveTextContent('No vendor checksum');
  expect(input).toHaveValue('0.170.0');
  expect(write).not.toHaveBeenCalled();
});

it('compares releases numerically and resolves a checked policy independently from an untested newest build', () => {
  const builds = [build('0.99.0'), build('0.162.1'), { ...build('0.164.0', 'untested'), source: 'channel' as const }];
  expect(resolvedBuild(builds, 'codex', { use: 'newest_checked' })?.version).toBe('0.162.1');
  expect(resolvedBuild(builds, 'codex', { use: 'newest' })?.version).toBe('0.164.0');
  expect(resolvedBuild(builds, 'codex', { use: 'exact', version: 'missing' })).toBeUndefined();
});

it('warns about a failed target while leaving the reviewed update available', async () => {
  const planned = plan();
  planned.plans[0]!.updates[0]!.to = '0.164.0';
  state.plan = planned;
  show(<UpdateDialog targets={{ environments: [detailEnvironment.id] }} onClose={vi.fn()} />);
  expect(await screen.findByText(i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }))).toBeVisible();
  expect(screen.queryByText('No native question')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: i18n.t('programVersions.updateCount', { units: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }) }) })).toBeEnabled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }) }));
  expect(screen.getByRole('dialog', { name: i18n.t('admin.harnesses.checkResults') })).toHaveTextContent(`${i18n.t('programVersions.checks.userInput')} · ${i18n.t('admin.harnesses.status.failed')}`);
  expect(screen.getByText('No native question')).toHaveClass('text-text-muted');
});

it('keeps the resolved checked version when switching the policy to Exact', async () => {
  const write = vi.fn();
  show(<VersionChoiceControl kind="codex" choice={{ use: 'newest_checked' }} builds={state.builds} target={{ kind: 'system' }} onChange={write} />);
  await userEvent.click(screen.getByRole('combobox', { name: 'Codex' }));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.harnessVersion.exact.name') }));
  expect(screen.getByRole('combobox', { name: i18n.t('admin.harnesses.version') })).toHaveValue('0.162.1');
  expect(write).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') }));
  expect(write).toHaveBeenCalledExactlyOnceWith({ use: 'exact', version: '0.162.1' });
});

it('keeps a refused resolved version as a draft instead of applying it through the Exact selector', async () => {
  const refused = { ...build('0.162.1', 'failed'), check: { ...build('0.162.1').check!, error: 'refused: checksum mismatch' } };
  const write = vi.fn();
  show(<VersionChoiceControl kind="codex" choice={{ use: 'shipped' }} builds={[refused]} target={{ kind: 'system' }} onChange={write} />);
  await userEvent.click(screen.getByRole('combobox', { name: 'Codex' }));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.harnessVersion.exact.name') }));
  expect(screen.getByRole('combobox', { name: i18n.t('admin.harnesses.version') })).toHaveValue('0.162.1');
  expect(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') })).toBeDisabled();
  expect(write).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`saves a scoped Exact choice through Use version itself (${locale})`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  const definition = SETTINGS_REGISTRY.find((entry) => entry.key === PROGRAM_VERSIONS_KEY);
  if (definition === undefined) throw new Error('The program policy must be in the generated registry');
  const def = withGroup(localizedSetting(definition, i18n, locale));
  const setting: ResolvedSetting = { key: PROGRAM_VERSIONS_KEY, value: { codex: { use: 'newest_checked' } }, own: { value: {}, revision: 1, locked: false }, writable: true, locked_by: null, source: { scope: 'environment_own', node: null }, parts: [] };
  const target = { kind: 'environment' as const, id: detailEnvironment.id };
  state.settingWrite.mockImplementation((_target, _key, change) => Promise.resolve({ ...setting, value: change.value, own: { value: change.value, revision: 2, locked: false } }));
  show(<ScopedSettingRow def={def} setting={setting} target={target} place={{ name: detailEnvironment.name, owner: detailEnvironment.owner, access: detailEnvironment.access, kind: null }} />);
  await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: def.title }) }));
  await user.click(screen.getByRole('combobox', { name: 'Codex' }));
  await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.harnessVersion.exact.name') }));
  expect(state.settingWrite).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') }));
  await waitFor(() => expect(state.settingWrite).toHaveBeenCalledExactlyOnceWith(target, PROGRAM_VERSIONS_KEY, { value: { codex: { use: 'exact', version: '0.162.1' } }, expectedRevision: 1 }));
  expect(await screen.findByText(i18n.t('settings.compact.saved'), { exact: true })).toBeVisible();
});

it('uses the planned artifact check rather than a different processor in the catalogue', async () => {
  const planned=plan();
  planned.plans[0]!.updates[0]!.to='0.164.0';
  planned.plans[0]!.target_programs=[{kind:'codex',version:'0.164.0',build:build('0.164.0')}];
  state.plan=planned;
  show(<UpdateDialog targets={{environments:[detailEnvironment.id]}} onClose={vi.fn()} />);
  await screen.findByText(i18n.t('programVersions.change',{name:'Codex',from:'0.164.0',to:'0.164.0',direction:i18n.t('programVersions.different')}));
  expect(screen.getByText(i18n.t('admin.harnesses.status.checked'))).toBeVisible();
  expect(screen.queryByText(i18n.t('programVersions.offInBuild', { feature: i18n.t('programVersions.disabledFeatures.questions') }))).not.toBeInTheDocument();
});

it('waits for every fetched platform check before offering an exact version for use', async () => {
  const write=vi.fn();
  const unavailable:HarnessBuild={...build('0.170.0','untested'),arch:'aarch64',source:'channel',check:{...build('0.170.0').check!,error:'No Arm host'}};
  const pending:HarnessBuild={...build('0.170.0','checking'),source:'channel'};
  const view=show(<VersionChoiceControl kind="codex" choice={{use:'exact',version:'0.170.0'}} builds={[unavailable,pending]} target={{kind:'system'}} onChange={write} />);
  expect(screen.getByRole('button',{name:i18n.t('programVersions.checking')})).toBeDisabled();
  expect(write).not.toHaveBeenCalled();
  view.rerender(<VersionChoiceControl kind="codex" choice={{use:'exact',version:'0.170.0'}} builds={[unavailable,{...pending,status:'checked'}]} target={{kind:'system'}} onChange={write} />);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('programVersions.useVersion')}));
  expect(write).toHaveBeenCalledExactlyOnceWith({use:'exact',version:'0.170.0'});
});

it('shows inherited policy explicitly and clears only the selected program pin', async () => {
  const write=vi.fn();
  const other={'claude-code':{use:'exact',version:'2.1.296'}};
  const view=show(<HarnessVersionsChoice value={other} effective={{codex:{use:'newest_checked'},...other}} target={{kind:'environment',id:'chosen'}} onChange={write} />);
  const control=await screen.findByRole('combobox',{name:'Codex'});
  expect(control).toHaveTextContent(i18n.t('programVersions.inherit'));
  view.rerender(<HarnessVersionsChoice value={{...other,codex:{use:'exact',version:'0.162.1'}}} effective={{...other,codex:{use:'exact',version:'0.162.1'}}} target={{kind:'environment',id:'chosen'}} onChange={write} />);
  await userEvent.click(control);
  await userEvent.click(screen.getByRole('option',{name:i18n.t('programVersions.inherit')}));
  expect(write).toHaveBeenCalledExactlyOnceWith(other);
});

it('reviews each resource pool separately and keeps active shared-thread updates exclusive',async()=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const job={...failedStart().job,id:'environment-job',environment_id:detailEnvironment.id};
  state.start.mockResolvedValue({batch_id:'environment-batch',jobs:[{environment:detailEnvironment,job}]});
  show(<QueryClientProvider client={client}><UpdateDialog targets={{environments:[detailEnvironment.id],providers:[detailUnitProvider.id]}} onClose={vi.fn()} /></QueryClientProvider>);
  await screen.findByText(detailEnvironment.name);
  expect(state.providerRead).not.toHaveBeenCalled();
  const providers=screen.getByRole('radio',{name:i18n.t('programVersions.runtimeCount',{count:1,number:'1'})});
  await userEvent.click(screen.getByRole('button',{name:i18n.t('programVersions.updateCount',{units:i18n.t('programVersions.environmentCount',{count:1,number:'1'})})}));
  await waitFor(()=>expect(state.start).toHaveBeenCalledOnce());
  expect(providers).toBeDisabled();
  expect(state.providerStart).not.toHaveBeenCalled();
  await act(async ()=>{ client.setQueryData(['job',job.id],{...job,state:'succeeded'}); await Promise.resolve(); });
  await waitFor(()=>expect(providers).toBeEnabled());
  await userEvent.click(providers);
  await screen.findByText(detailUnitProvider.name);
  expect(state.providerRead).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('programVersions.updateCount',{units:i18n.t('programVersions.runtimeCount',{count:1,number:'1'})})}));
  await waitFor(()=>expect(state.providerStart).toHaveBeenCalledOnce());
  expect(state.providerStart.mock.calls[0]?.[0].preflight_id).toBe('provider-plan');
  expect(state.start).toHaveBeenCalledOnce();
});

it('explains selections that disappeared without exposing an inaccessible name',async()=>{
  state.plan={...plan(),plans:[],skipped:[],executable:0};
  show(<UpdateDialog targets={{environments:[detailEnvironment.id]}} onClose={vi.fn()} />);
  expect(await screen.findByText(i18n.t('programVersions.unavailableEnvironments',{count:1,number:'1'}))).toBeVisible();
  expect(screen.queryByText(detailEnvironment.name)).not.toBeInTheDocument();
  expect(screen.getByRole('button',{name:i18n.t('programVersions.updateCount',{units:i18n.t('programVersions.environmentCount',{count:0,number:'0'})})})).toBeDisabled();
});

it('refuses a recorded checksum mismatch before applying Exact while leaving compatibility failures selectable', async () => {
  const refused = build('0.170.0', 'failed');
  refused.check = { ...refused.check!, error: 'refused: checksum mismatch' };
  const write = vi.fn();
  show(<VersionChoiceControl kind="codex" choice={{ use: 'exact', version: refused.version }} builds={[refused, ...state.builds]} target={{ kind: 'system' }} onChange={write} />);
  expect(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') })).toBeDisabled();
  expect(screen.getByRole('button', { name: i18n.t('programVersions.refused') })).toBeEnabled();
  expect(screen.getByText(i18n.t('programVersions.fetchRefused', { name: 'Codex', version: refused.version, reason: refused.check.error }))).toBeVisible();
  expect(write).not.toHaveBeenCalled();
  const input = screen.getByRole('combobox', { name: i18n.t('admin.harnesses.version') });
  await userEvent.clear(input); await userEvent.type(input, '0.164.0');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('programVersions.useVersion') }));
  expect(write).toHaveBeenCalledExactlyOnceWith({ use: 'exact', version: '0.164.0' });
});

it('refreshes affected use counts when the resolved build catalogue changes', async () => {
  const client = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  state.usage = state.usage.map((row) => ({ ...row, update_environments: [] }));
  show(<QueryClientProvider client={client}><HarnessVersions kinds={['codex']} builds={state.builds} title={() => 'Codex'} /></QueryClientProvider>);
  await screen.findByRole('button', { name: i18n.t('programVersions.usage', { version: '0.162.1', environments: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }), providers: i18n.t('programVersions.runtimeCount', { count: 0, number: '0' }) }) });
  expect(screen.queryByRole('button', { name: i18n.t('programVersions.updateAvailable', { units: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }) }) })).not.toBeInTheDocument();
  state.usage = state.usage.map((row) => ({ ...row, update_environments: [detailEnvironment.id] }));
  state.builds = [{ ...build('0.162.1'), is_default: false }, { ...build('0.164.0'), is_default: true }];
  await act(async () => { client.setQueryData(['program-builds'], state.builds); await Promise.resolve(); });
  expect(await screen.findByRole('button', { name: i18n.t('programVersions.updateAvailable', { units: i18n.t('programVersions.environmentCount', { count: 1, number: '1' }) }) })).toBeEnabled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`summarizes program policies by display name and choice (${locale})`, async () => {
  await setLocale(locale);
  const value = { codex: { use: 'exact', version: '0.162.1' }, 'claude-code': { use: 'newest_checked' } };
  const view = show(<SettingSummary value={value} def={{ key: PROGRAM_VERSIONS_KEY }} />);
  const names = [
    i18n.t('programVersions.policySummary', { name: 'Codex', choice: i18n.t('programVersions.exactSummary', { choice: i18n.t('choiceCatalog.harnessVersion.exact.name'), version: '0.162.1' }) }),
    i18n.t('programVersions.policySummary', { name: 'Claude Code', choice: i18n.t('choiceCatalog.harnessVersion.newest_checked.name') }),
  ];
  await waitFor(() => expect(view.container.textContent).toBe(new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(names)));
  expect(view.container).not.toHaveTextContent('2 fields');
  expect(view.container).not.toHaveTextContent('newest_checked');
});
