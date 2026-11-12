/** Native conversion chooses a real destination without inventing source workspace controls. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ForksApi from '@/api/forks';
import type * as NodesApi from '@/api/nodes';
import type * as ResourcesApi from '@/api/resources';
import { initialChoices, initialThreadChoices, requestOf, type Choices } from '@/features/change/choices';
import type * as RunsOnModule from '@/features/change/RunsOn';
import { ThreadChange } from '@/features/change/ThreadChange';
import { Where } from '@/features/change/Where';
import { Options } from '@/features/change/Options';
import type { ForkPlan, ForkRequest, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { creationHost, placeEnvironment } from '../unit/start-fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const wire = vi.hoisted(() => ({ thread: null as Thread | null, plan: vi.fn<(body: ForkRequest) => Promise<ForkPlan>>() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn() }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof NodesApi>(), nodeQuery: () => ({ queryKey: ['builtin-native-source'], queryFn: () => {
  if (wire.thread === null) throw new Error('A real built-in thread is required'); return Promise.resolve(wire.thread);
} }) }));
vi.mock('@/api/forks', async (load) => ({ ...await load<typeof ForksApi>(),
  placesQuery: () => ({ queryKey: ['builtin-native-places'], queryFn: () => Promise.resolve([creationHost(), placeEnvironment('native-place')]) }),
  planFork: (_thread: string, body: ForkRequest) => wire.plan(body),
}));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof ResourcesApi>(),
  providersQuery: () => ({ queryKey: ['builtin-native-providers'], queryFn: () => Promise.resolve([]) }),
  agentsQuery: () => ({ queryKey: ['builtin-native-agents'], queryFn: () => Promise.resolve([]) }),
}));
vi.mock('@/features/change/RunsOn', async (load) => ({ ...await load<typeof RunsOnModule>(), useModelFor: () => [],
  RunsOn: ({ compact, onChange }: {compact?: boolean; onChange: (patch: Partial<Choices>) => void}) => <>
    <output aria-label="Runtime layout">{compact ? 'compact' : 'full'}</output>
    <button onClick={() => onChange({providerId: 'native-provider', agentId: 'native-agent', model: 'native-model'})}>Choose native binding</button>
  </>,
}));
vi.mock('@/features/change/History', () => ({ HistoryChoices: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

function source(): Thread {
  const thread = snapshot().thread;
  return {...thread, binding: null, environment: null, environment_permissions: [],
    program: {id: 'orch', root_org_id: ACME.org,
      requires_environment: false, requires_native_harness: false, read_only: true}};
}
beforeEach(() => { wire.thread = source(); wire.plan.mockReset().mockImplementation(() => new Promise<ForkPlan>(() => undefined)); });
afterEach(async () => { cleanup(); await setLocale('en'); });

function Placement({result}: {result: 'new' | 'replace'}) {
  const thread = source();
  const [choices, setChoices] = useState(() => initialThreadChoices(thread, result));
  return <><Where thread={thread} choices={choices} fork={result === 'new'} onChange={(patch) => setChoices((current) => ({...current, ...patch}))} />
    <output aria-label="Native request">{JSON.stringify(requestOf(choices))}</output></>;
}

function PageDefaults() {
  const [choices, setChoices] = useState(() => initialThreadChoices(source(), 'new'));
  return <Options choices={choices} applied={null} plan={null} topic="Topic" defaultPage="private"
    onChange={(patch) => setChoices((current) => ({...current, ...patch}))} />;
}

for (const locale of ['en', 'zh-CN'] as const) {
  for (const result of ['new', 'replace'] as const) it(`chooses existing or new native placement without source files (${locale}, ${result})`, async () => {
    // EN: kept Orch thread > native Fork/Switch > select an existing environment >
    // choose New environment, then Existing again; request keeps files:none and no source volumes.
    // 中文：已保留的 Orch 会话 > 原生分叉或切换 > 选择现有环境 >
    // 改选新环境再切回现有环境；请求始终不复制不存在的来源文件或卷。
    await setLocale(locale); const user = userEvent.setup(); show(<Placement result={result} />);
    const destination = screen.getByRole('combobox', {name: i18n.t('change.where.label')});
    expect(screen.queryByRole('radiogroup', {name: i18n.t('fork.files.title')})).not.toBeInTheDocument();
    await user.click(destination);
    expect(screen.queryByRole('option', {name: i18n.t('change.where.sameNone')})).not.toBeInTheDocument();
    await user.keyboard('{Escape}');
    await user.click(await screen.findByRole('combobox', {name: i18n.t('change.where.another')}));
    await user.click(await screen.findByRole('option', {name: /Env native-place/}));
    expect(JSON.parse(screen.getByLabelText('Native request').textContent)).toMatchObject({environment: {use: 'native-place', files: {mode: 'none'}}});
    await user.click(destination); await user.click(screen.getByRole('option', {name: i18n.t('start.where.createNew')}));
    expect(JSON.parse(screen.getByLabelText('Native request').textContent)).toMatchObject({environment: {new: {files: {mode: 'none'}, private_views: false}}});
    expect(screen.queryByRole('checkbox', {name: i18n.t('fork.files.home')})).not.toBeInTheDocument();
    expect(screen.queryByRole('checkbox', {name: i18n.t('fork.files.privateViews')})).not.toBeInTheDocument();
    await user.click(destination); await user.click(screen.getByRole('option', {name: i18n.t('start.where.useExisting')}));
    expect(JSON.parse(screen.getByLabelText('Native request').textContent)).toMatchObject({environment: {use: 'native-place', files: {mode: 'none'}}});
  });

  it(`resets a built-in fork's audience to its Private default (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); show(<PageDefaults />);
    expect(screen.queryByRole('button', {name: i18n.t('changeCompact.reset.access')})).not.toBeInTheDocument();
    await user.click(screen.getByRole('radio', {name: i18n.t('changeCompact.access.original')}));
    await user.click(screen.getByRole('button', {name: i18n.t('changeCompact.reset.access')}));
    expect(screen.getByRole('radio', {name: i18n.t('changeCompact.access.private')})).toBeChecked();
    expect(screen.queryByRole('button', {name: i18n.t('changeCompact.reset.access')})).not.toBeInTheDocument();
  });

  it(`requires an explicit native binding and environment before preflight (${locale})`, async () => {
    // EN: kept Orch thread > Switch provider or agent… > choose native binding >
    // choose a real environment; only then does the native preflight start.
    // 中文：已保留的 Orch 会话 > 切换提供方或智能体… > 选择原生绑定 >
    // 选择实际环境；完成后才读取原生预检方案。
    await setLocale(locale); const user = userEvent.setup(); const thread = source();
    show(<ThreadChange threadId={thread.id} kind="switch" toTurn={null} focus="provider" job={null} say={null} open onClose={() => undefined} />);
    expect(await screen.findByLabelText('Runtime layout')).toHaveTextContent('full');
    const apply = screen.getByRole('button', {name: i18n.t('change.confirm.switch')});
    expect(apply).toHaveAccessibleDescription(i18n.t('changeReady.chooseNativeBinding'));
    expect(wire.plan).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', {name: 'Choose native binding'}));
    expect(apply).toHaveAccessibleDescription(i18n.t('changeCompact.chooseEnvironment'));
    expect(wire.plan).not.toHaveBeenCalled();
    await user.click(await screen.findByRole('combobox', {name: i18n.t('change.where.another')}));
    await user.click(await screen.findByRole('option', {name: /Env native-place/}));
    await waitFor(() => expect(wire.plan).toHaveBeenCalledOnce());
    expect(wire.plan).toHaveBeenCalledWith({result: 'replace', to: {provider_id: 'native-provider', agent_id: 'native-agent', model: 'native-model'}, environment: {use: 'native-place', files: {mode: 'none'}}});
    expect(apply).toHaveAttribute('aria-disabled', 'true');
  });
}

it('keeps ordinary native-source defaults and starts built-in forks Private', () => {
  expect(initialThreadChoices(snapshot().thread, 'replace')).toEqual(initialChoices('replace'));
  const choices = initialThreadChoices(source(), 'new', 2);
  expect(requestOf(choices)).toMatchObject({access: 'private', history: {mode: 'range', to_turn: 2}});
});
