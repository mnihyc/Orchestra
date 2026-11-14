/** The actual screen keeps one visible Switch owner and its existing menu/finder paths. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import type * as Router from '@tanstack/react-router';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { commandsNow } from '@/features/finder/commands';
import { ThreadScreen } from '@/features/thread/Thread';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { actionName } from '@/ui/actions';
import { A, B, projection, snapshot } from '../unit/continuation-fixtures';
import { executionDisplay } from '../doubles/thread-display';
import { show } from './support';

const calls = vi.hoisted(() => ({ change: vi.fn() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ to, params, search, children, className }: { to: string; params?: { id: string }; search?: { tab: string }; children: ReactNode; className?: string }) =>
  <a className={className} href={`${to.replace('$id', params?.id ?? '')}${search === undefined ? '' : `?tab=${search.tab}`}`}>{children}</a> }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: () => undefined, useOrgTreeFeed: () => undefined }));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', async (loadOriginal) => ({ ...await loadOriginal<object>(), selectOrg: () => undefined }));
vi.mock('@/features/inbox/Counters', async (loadOriginal) => ({ ...await loadOriginal<object>(), useReadWhenOpened: () => undefined }));
vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: calls.change }));
vi.mock('@/features/navigation/favorites', async (loadOriginal) => ({ ...await loadOriginal<object>(), StarButton: () => <button>Star</button> }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => <button>Notifications</button> }));
vi.mock('@/features/navigation/NodeMenu', async (loadOriginal) => ({ ...await loadOriginal<object>(), useNodeMenu: () => [] }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: vi.fn() }) }));
vi.mock('@/features/running/live', () => ({ useRunning: () => ({ roster: null, closed: false, error: null }) }));
vi.mock('@/features/timeline/preferences', () => ({ useTimelinePreferences: () => ({ detail: 'steps', hideHistory: false, setDetail: vi.fn(), setHideHistory: vi.fn() }) }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => <textarea aria-label="Thread draft" /> }));
vi.mock('@/features/composer/QueueTray', () => ({ QueueTray: () => null }));
vi.mock('@/features/schedules/Schedules', () => ({ PendingAgentSchedules: () => null }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => <p>Native lock progress</p> }));
vi.mock('@/features/thread/CallRecovery', () => ({ ThreadCallRecovery: () => <p>Native call recovery</p> }));
vi.mock('@/features/thread/ContinuationNotice', () => ({ ContinuationNotice: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => <p>The redirect has been checked.</p> }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ children }: { children: ReactNode }) => children }));

const detail = "Codex on Build machine is 0.164.0; this agent runs 0.165.0. An environment manager can allow it on the environment's page.";
let native: Thread;
beforeEach(() => { calls.change.mockReset(); threadWriter.clear(); native = snapshot().thread; });
afterEach(async () => { cleanup(); threadWriter.clear(); await setLocale('en'); });

function draw(more: Partial<Thread> = {}, historical = false) {
  const thread: Thread = { ...native, state: 'blocked', display: executionDisplay('blocked'), lock: null,
    blocked: { reason: 'binding_invalid', detail, sandbox: null }, ...more };
  const selected = historical ? B : null;
  selectThreadContinuation(thread.id, selected);
  const rows = { ...snapshot(projection(historical ? B : A, A, 1, !historical)), thread };
  expect(threadWriter.reset(thread.id, rows, false, captureThreadRead(thread.id))).not.toBeNull();
  show(<ThreadScreen threadId={thread.id} item={null} continuation={selected} />);
  return thread;
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`gives the visible mismatch recovery one Switch owner while retaining menu and finder (${locale})`, async () => {
    await setLocale(locale); const thread = draw(), user = userEvent.setup();
    const name = i18n.t('change.banner.switchProfile');
    const recovery = within(screen.getByRole('alert')).getByRole('button', { name });
    expect(recovery).toBeVisible();
    expect(screen.getAllByRole('button', { name })).toHaveLength(1);
    await user.click(recovery);
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
    const command = commandsNow().find((entry) => entry.id === 'thread.change-switch');
    expect(command).toBeDefined();
    if (command === undefined) throw new Error('The existing Switch command must remain registered');
    act(() => command.run('here'));
    expect(calls.change).toHaveBeenCalledTimes(2);
    const actions = within(screen.getByRole('banner')).getByRole('button', { name: actionName('actions', thread.name) });
    await user.click(actions);
    const placement = screen.getByRole('menuitem', { name: i18n.t('thread.header.placement') });
    act(() => placement.focus()); await user.keyboard('{ArrowRight}');
    expect(await screen.findByRole('menuitem', { name: i18n.t('change.menu.switch') })).toBeVisible();
    await user.keyboard('{Escape}{Escape}'); expect(actions).toHaveFocus();
  });

  it(`keeps the header's existing action state in a historical view without a recovery banner (${locale})`, async () => {
    await setLocale(locale); draw({}, true);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(within(screen.getByRole('banner')).getByRole('button', { name: i18n.t('change.menu.switch') })).toHaveAttribute('aria-disabled', 'true');
  });

  it(`does not offer a manager's Switch to a thread reader (${locale})`, async () => {
    await setLocale(locale); draw({ access: { ...native.access, permissions: ['thread.view', 'thread.interact'] } });
    expect(screen.queryByRole('button', { name: i18n.t('change.banner.switchProfile') })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: i18n.t('resourceFacts.openEnvironment') })).toBeVisible();
  });
}

it.each(['idle', 'locked', 'second step', 'call recovery', 'history removal'] as const)('retains the existing ordinary owner for %s', (state) => {
  const blocked: Thread['blocked'] = { reason: 'binding_invalid', detail, sandbox: null };
  const more: Partial<Thread> = state === 'idle' ? { state: 'idle', blocked: null }
    : state === 'locked' ? { lock: { id: 'held-job', kind: 'thread_switch', phase: 'switching', percent: 30, state: 'running' } }
      : { blocked: { ...blocked, detail: state === 'second step' ? 'second_step_required' : state === 'call recovery' ? 'call_recovery' : i18n.getFixedT('en')('agentHistory.removingDetail') } };
  draw(more);
  const button = within(screen.getByRole('banner')).queryByRole('button', { name: i18n.t('change.menu.switch') });
  if (state === 'locked') expect(button).not.toBeInTheDocument();
  else expect(button).toBeVisible();
  expect(screen.queryByRole('alert')?.querySelector('button') ?? null).toBeNull();
});

it('keeps an opaque second-step-looking detail on another reason in its existing recovery', () => {
  draw({ blocked: { reason: 'agent_disabled', detail: 'second_step_required', sandbox: null } });
  expect(within(screen.getByRole('alert')).getByRole('button', { name: i18n.t('change.banner.switchProfile') })).toBeVisible();
  expect(screen.getAllByRole('button', { name: i18n.t('change.banner.switchProfile') })).toHaveLength(1);
});
