/** The compact mode row uses only native choices, one shared read and the current session's reports. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Settings from '@/api/settings';
import type * as Threads from '@/api/threads';
import { ConfinementPicker } from '@/features/composer/ApprovalMode';
import { ComposerControls } from '@/features/composer/Controls';
import { PlanSwitch } from '@/features/composer/PlanSwitch';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import type { ResolvedSetting, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { threadWriter } from '@/realtime/stores/thread';
import { modeParameters, nativeModeSetting } from '../doubles/mode-controls';
import { marker } from '../doubles/thread-rows';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ rows: [] as ResolvedSetting[], settings: vi.fn(), parameters: vi.fn(), write: vi.fn<typeof Threads.setThreadSetting>() }));
vi.mock('@/api/settings', async (original) => ({ ...await original<typeof Settings>(), settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => { wire.settings(); return Promise.resolve(wire.rows); }, staleTime: Infinity }) }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), parametersQuery: (id: string) => ({ queryKey: ['thread-parameters', id], queryFn: () => { wire.parameters(); return Promise.resolve(modeParameters()); }, staleTime: Infinity }), setThreadSetting: wire.write }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: () => <button>Model and effort</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => null }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<object>(), refreshThread: vi.fn() }));
const appearance = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme; const darkBefore = document.documentElement.classList.contains('dark'); const ids: string[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  wire.rows = [nativeModeSetting('approval.mode', 'on_request', 1, { own: null, source: { scope: 'topic', node: null } }),
    nativeModeSetting('sandbox.mode', 'workspace', 1, { own: null, source: { scope: 'environment', node: null }, available_values: ['read_only', 'workspace'] })];
  wire.write.mockImplementation((_id, key, body) => Promise.resolve(nativeModeSetting(key === 'sandbox.mode' ? key : 'approval.mode', String(body.value), 1)));
});
afterEach(async () => { cleanup(); for (const id of ids.splice(0)) threadWriter.evict(id); useAppearance.setState(appearance); document.documentElement.classList.toggle('dark', darkBefore); if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore; await setLocale('en'); });
function nativeThread(): Thread {
  const source = snapshot(null); const thread = { ...source.thread, current_segment_id: 'mode-session', access: { ...source.thread.access, permissions: ['thread.view', 'thread.interact', 'thread.manage'] } };
  ids.push(thread.id); threadWriter.reset(thread.id, { ...source, thread }, false); return thread;
}
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps approval, sandbox and Plan together with actual defaults and one reader at ${locale}/${theme}/${width}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark'); const thread = nativeThread();
    const view = show(<ViewCompactContext.Provider value={width === 400}><div style={{ width }}><ComposerControls thread={thread} attachment={<button>Attach</button>} delivery={<button>Send</button>} plan={<PlanSwitch on={false} defaulted onChange={() => undefined} />} /></div></ViewCompactContext.Provider>);
    const sandbox = await screen.findByRole('button', { name: i18n.t('composer.confinement.label', { mode: i18n.t('choiceCatalog.sandboxLevel.workspace.name') }) });
    expect(screen.getByRole('button', { name: i18n.t('composer.mode.label', { mode: i18n.t('composer.mode.labels.on_request') }) })).toBeVisible();
    const order = Array.from(view.container.querySelectorAll<HTMLElement>('[data-composer-control]')).map((node) => node.dataset.composerControl);
    expect(order.slice(order.indexOf('mode'), order.indexOf('mode') + 3)).toEqual(['mode', 'sandbox', 'plan']);
    for (const key of ['composer.mode.placeDefault', 'composer.confinement.placeDefault', 'composer.plan.placeDefault'] as const) expect(screen.getByText(i18n.t(key))).toBeVisible();
    // A narrow cell cuts the note of a place default instead of letting it run past its button.
    for (const key of ['composer.mode.placeDefault', 'composer.confinement.placeDefault'] as const) expect(screen.getByText(i18n.t(key))).toHaveClass('min-w-0', 'truncate');
    await userEvent.click(sandbox); const picker = screen.getByRole('dialog', { name: i18n.t('composer.confinement.title') });
    expect(within(picker).queryByRole('button', { name: i18n.t('choiceCatalog.sandboxLevel.open.name') })).toBeNull();
    const read = within(picker).getByRole('button', { name: i18n.t('choiceCatalog.sandboxLevel.read_only.name') });
    await waitFor(() => expect(read).toBeEnabled()); await userEvent.click(read);
    await waitFor(() => expect(wire.write).toHaveBeenCalledExactlyOnceWith(thread.id, 'sandbox.mode', { value: 'read_only', expected_revision: 0 }));
    expect(await screen.findByRole('button', { name: i18n.t('composer.confinement.label', { mode: i18n.t('choiceCatalog.sandboxLevel.read_only.name') }) })).toBeVisible();
    expect(screen.getByText(i18n.t('composer.mode.placeDefault'))).toBeVisible();
    expect(screen.getByText(i18n.t('composer.plan.placeDefault'))).toBeVisible();
    expect(screen.queryByText(i18n.t('composer.confinement.placeDefault'))).toBeNull();
    expect(wire.settings).toHaveBeenCalledTimes(1); expect(wire.parameters).toHaveBeenCalledTimes(1);
  });
}
for (const reason of ['viewer', 'locked'] as const) it(`retains the sandbox value and explains every refused choice for ${reason}`, async () => {
  const thread = nativeThread();
  if (reason === 'locked') wire.rows[1] = nativeModeSetting('sandbox.mode', 'workspace', 1, { available_values: ['read_only', 'workspace'], writable: false, locked_by: { scope: 'org', node: { id: 'org', kind: 'org', name: 'Acme', redacted: false } } });
  show(<ConfinementPicker thread={thread} readOnly={reason === 'viewer'} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Sandbox: Workspace writes' }));
  const choice = await screen.findByRole('button', { name: 'Read only' });
  await waitFor(() => expect(choice).toBeDisabled()); expect(choice).toHaveAttribute('aria-description', reason === 'viewer' ? i18n.t('composer.row.readOnly') : 'Decided by Acme');
  await userEvent.click(choice); expect(wire.write).not.toHaveBeenCalled();
});
it('shows only the latest actual native divergence in this session, and retires it when the program agrees', async () => {
  const thread = nativeThread();
  const report = (id: string, value: 'readOnly' | 'workspaceWrite' | 'dangerFullAccess', diverged: boolean, segment = 'mode-session', seq = 1) => ({ ...marker(id, { marker: 'harness_changed', data: {
    program: 'Codex', change: 'sandbox_mode', value: { type: value }, previous: { state: 'unsupported' }, expected: { state: 'told', value: { type: 'workspaceWrite' } }, changed: true, diverged, next_turn: true, turn_id: null, reported_at: '2026-11-07T20:00:00Z',
  } }), thread_id: thread.id, segment_id: segment, seq_created: seq, seq_updated: seq });
  act(() => threadWriter.apply(thread.id, { items: [report('old-session', 'dangerFullAccess', true, 'older-session')] }));
  const view = show(<ConfinementPicker thread={thread} />);
  expect(view.container.querySelector('[data-native-mode-report]')).toBeNull();
  act(() => threadWriter.apply(thread.id, { items: [report('current-report', 'readOnly', true, 'mode-session', 2)] }));
  expect(await screen.findByText('Codex: Read only')).toBeVisible();
  act(() => threadWriter.apply(thread.id, { items: [report('current-agrees', 'workspaceWrite', false, 'mode-session', 3)] }));
  expect(view.container.querySelector('[data-native-mode-report]')).toBeNull();
});
