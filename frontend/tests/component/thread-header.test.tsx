import { executionDisplay } from '../doubles/thread-display';
/** The head is one row: the title, the state, Changes, Find and the way of reading; the session menu keeps every other act, and the lines that tell of the thread stand in the notes at the top of the conversation. */
import { defaultThreadRead } from '@/realtime/threadScope';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThreadHeader } from '@/features/thread/Header';
import type * as NodesApi from '@/api/nodes';
import type * as ApprovalsApi from '@/api/approvals';
import type * as ThreadsApi from '@/api/threads';
import type * as ThreadStore from '@/realtime/stores/thread';
import type * as RunningUi from '@/features/running/Running';
import { approvalKey } from '@/api/approvals';
import type { Approval, HarnessRequest, Item, Segment, Thread, ThreadBackground, ThreadGoal, Turn } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { actionName } from '@/ui/actions';
import { client } from '@/api/client';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { commandsNow } from '@/features/finder/commands';
import { show } from './support';
import { backgroundWake, backgroundWork } from '../doubles/background-wake';
import { snapshot } from '../unit/continuation-fixtures';
import { ALLOW_DENY, approvalItem, change, fileChange, harnessApproval, marker, toolCall, turn } from '../doubles/thread-rows';
import type { MessageNav } from '@/features/thread/messageMoves';
import type { WorkDetail } from '@/features/timeline/rows';
import type { ComponentProps } from 'react';
import { me, memberAccess, userRef } from '../doubles/fixtures';

const calls = vi.hoisted(() => ({ approval: vi.fn<(id: string) => Promise<Approval>>(), settings: vi.fn(), review: vi.fn(), share: vi.fn(), update: vi.fn(), apply: vi.fn(), fork: vi.fn(), change: vi.fn(), ask: vi.fn(), find: vi.fn(), goal: vi.fn(), starred: false, items: new Map<string, Item>(), segments: [] as Segment[], background: null as ThreadBackground | null, backgroundClosed: false }));
const headerRows = vi.hoisted(() => ({ order: [] as string[], turns: new Map<string, Turn>(), hasOlder: false, hasNewer: false }));
const reading = vi.hoisted((): { detail: WorkDetail; setDetail: (detail: WorkDetail) => void; folded: Set<string> } => ({ detail: 'normal', setDetail: vi.fn(), folded: new Set<string>() }));
vi.mock('@/features/thread/rowFit', () => ({ useRowFit: () => reading.folded }));
vi.mock('@/api/approvals', async (original) => {
  const actual = await original<typeof ApprovalsApi>();
  return { ...actual, approvalQuery: (id: string) => ({ ...actual.approvalQuery(id), queryFn: () => calls.approval(id) }) };
});
const changesView = vi.hoisted(() => vi.fn());
vi.mock('@/features/inspector/reviewTarget', () => ({ openThreadChanges: changesView }));
vi.mock('@/features/settings/Editor', () => ({ SettingsEditor: (props: { target: unknown; only?: readonly string[]; scope: string }) => { calls.settings(props); return <div>One scoped setting</div>; } }));
vi.mock('@/features/goal/editor', () => ({ openGoalEditor: calls.goal }));
vi.mock('@/api/nodes', async (importOriginal) => ({ ...await importOriginal<typeof NodesApi>(), updateNode: calls.update }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(), resumeAutomatic: vi.fn() }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: calls.share }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => <button>{calls.starred ? 'Unstar' : 'Star'}</button> }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => <button>Notifications</button> }));
vi.mock('@/features/navigation/NodeMenu', () => ({ useNodeMenu: () => [calls.starred ? 'Unstar' : 'Star', 'Export', 'Archive', 'Delete'].map((label) => ({ id: label.toLowerCase(), label, onSelect: vi.fn() })) }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: calls.fork }) }));
vi.mock('@/api/forks', async (original) => ({ ...await original<object>(), planFork: () => Promise.resolve({
  access: { summary: { custom: false, from: null, interact: [], manage: [], view: [] } },
  agent_access: { preset: { where: 'nothing', parts: {} }, choices: [
    { where: 'nothing', parts: {} }, { where: 'thread', parts: {} },
    { where: 'topic', parts: {} }, { where: 'project', parts: {} },
  ] },
}) }));
vi.mock('@/features/change/target', () => ({ openChange: calls.change }));
vi.mock('@/features/side-threads/ask', () => ({ openSideAsk: calls.ask }));
vi.mock('@/features/side-threads/SideChips', () => ({ SideThreadList: () => <p>Open side threads</p> }));
vi.mock('@/features/thread-move/MoveToTopic', () => ({ MoveToTopic: () => null }));
vi.mock('@/features/start/SetUpThread', () => ({ SetUpThread: ({ thread, onClose }: { thread: Thread; onClose: () => void }) => <div role="dialog" aria-label={`Setup ${thread.name}`}><button onClick={onClose}>Close setup</button></div> }));
vi.mock('@/features/timeline/FindInConversation', () => ({ openFind: calls.find }));
vi.mock('@/features/inbox/away/Catchup', () => ({ CatchupRequest: () => null }));
vi.mock('@/features/running/Running', async (importOriginal) => ({ ...await importOriginal<typeof RunningUi>(), RunningDialog: ({ thread, close, endTarget }: { thread: string; close: () => void; endTarget?: unknown }) => { calls.review(endTarget); return <div role="dialog" aria-label={i18n.t('running.title')} data-running-thread={thread}><button onClick={close}>{i18n.t('common.close')}</button></div>; } }));
vi.mock('@/features/running/live', () => ({ useRunning: () => ({ roster: calls.background, closed: calls.backgroundClosed, error: null }) }));
vi.mock('@/features/timeline/preferences', () => ({ useTimelinePreferences: () => ({ detail: reading.detail, setDetail: reading.setDetail, hideHistory: false, setHideHistory: vi.fn() }) }));
vi.mock('@/realtime/stores/thread', async (importOriginal) => {
  const actual = await importOriginal<typeof ThreadStore>();
  return { ...actual, runningTurn: () => null,
    useThreadPart: (_id: string, select: (view: typeof actual.EMPTY_VIEW) => unknown) => select({ ...actual.EMPTY_VIEW, items: calls.items, segments: calls.segments, ...headerRows }),
    threadWriter: { ...actual.threadWriter, apply: calls.apply },
  };
});

// A thread carries its root-to-node path, including for the header's real place/menu projection.
const path: Thread['path'] = [
  { id: 'org-1', kind: 'org', name: 'Acme', redacted: false },
  { id: 'project-1', kind: 'project', name: 'Web', redacted: false },
  { id: 'topic-1', kind: 'topic', name: 'Login', redacted: false },
  { id: 'thread-1', kind: 'thread', name: 'Repair login', redacted: false },
];
const approvalRequest: HarnessRequest = { request_kind: 'command', options: ALLOW_DENY, detail: { command: 'check redirect', cwd: '/workspace', reason: null } };

const background = (more: Partial<ThreadBackground> = {}): ThreadBackground => ({ supported: true, state: 'live', total: 0, items: [], next_cursor: null, list_ref: null, expires_at: null, as_of: null, can_end: true, reason: null, ...more });
beforeEach(() => { vi.clearAllMocks(); calls.approval.mockReset().mockRejectedValue(new Error('No approval fixture was supplied')); calls.starred = false; calls.items.clear(); calls.segments = []; calls.background = background(); calls.backgroundClosed = false; });
const hosts: HTMLElement[] = [];
beforeEach(() => { reading.detail = 'normal'; reading.folded.clear(); });
beforeEach(() => { headerRows.order = []; headerRows.turns.clear(); headerRows.hasOlder = false; headerRows.hasNewer = false; changesView.mockClear(); });
afterEach(async () => { cleanup(); for (const host of hosts.splice(0)) host.remove(); await setLocale('en'); });

function draw(more: Partial<Thread> = {}, cache?: QueryClient, props: Omit<ComponentProps<typeof ThreadHeader>, 'thread' | 'notes'> = {}) {
  const native = snapshot().thread;
  const thread: Thread = { ...native, id: 'thread-1', kind: 'thread', name: 'Repair login', revision: 4, description: '', title_source: 'user', auto_updated_at: null,
    org_id: 'org-1', parent_id: 'topic-1', topic_id: 'topic-1', path,
    display: executionDisplay(more.state ?? 'idle'), state: 'idle', lock: null, blocked: null, archived: false, side_of: null, sides: { open: 0, allowed: true, max: 2, mode: 'read_only', reason: null }, pending_approvals: 0,
    owner: { ...native.owner, id: 'ada', display_name: 'Ada' }, access: { ...native.access, permissions: ['thread.view', 'thread.interact', 'thread.manage'] }, can: { ...native.can, fork: true, place: true, archive: true },
    ...more };
  // The lines under the title are drawn by the header into the place the screen gives them, at the top of the conversation.
  const notes = document.body.appendChild(document.createElement('div'));
  hosts.push(notes);
  const content = <ThreadHeader thread={thread} notes={notes} {...props} />;
  const drawn = show(cache === undefined ? content : <QueryClientProvider client={cache}>{content}</QueryClientProvider>);
  return { thread, header: screen.getByRole('banner'), notes, ...drawn };
}

async function confirmFork(user: ReturnType<typeof userEvent.setup>) {
  const dialog = await screen.findByRole('dialog', { name: i18n.t('agentAccess.forkTitle', { name: 'Repair login' }) });
  expect(calls.fork).not.toHaveBeenCalled();
  const confirm = () => within(dialog).getByRole('button', { name: i18n.t('change.menu.forkNow') });
  await waitFor(() => expect(confirm()).toBeEnabled());
  await user.click(confirm());
  expect(calls.fork).toHaveBeenCalledExactlyOnceWith(undefined, { where: 'nothing', parts: {} });
}

it.each([
  ['en', 'Move to another org', 'Moves with its main thread · Keep it to move it alone'],
  ['zh-CN', '移到其他组织', '随主会话一起移动 · 保留后可单独移动'],
] as const)('keeps an open side Move disabled with only its short tooltip in %s', async (locale, label, reason) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const { thread, header } = draw({ side_of: { id: 'main-thread', kind: 'thread', name: 'Main thread', redacted: false } });
  await user.click(within(header).getByRole('button', { name: actionName('actions', thread.name) }));
  const move = screen.getByRole('menuitem', { name: label });
  expect(move).toHaveAttribute('aria-disabled', 'true');
  expect(move).toHaveTextContent(label);
  expect(within(move).queryByText(reason)).not.toBeInTheDocument();
  await user.hover(move);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(reason);
  await user.click(move);
  act(() => move.focus());
  await user.keyboard('{Enter}');
  expect(calls.change).not.toHaveBeenCalled();
  expect(calls.fork).not.toHaveBeenCalled();
  expect(calls.ask).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('shows native work from an earlier turn beside Idle, in the notes and in the Running entry (%s)', async (locale) => {
  await setLocale(locale);
  calls.background = background({ total: 2, items: ['one', 'two'].map((id) => ({ id, segment_id: 'earlier-segment', turn_id: 'earlier-turn', item_id: `source-${id}`, command: 'sleep 300', command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment', since: null, cpu_percent: null, memory_bytes: null })) });
  const { header, notes, thread } = draw({ state: 'idle' });
  expect(within(header).getByText(i18n.t('state.idle'), { exact: true })).toBeVisible();
  expect(within(notes).getAllByText('sleep 300')).toHaveLength(2);
  expect(header).not.toContainElement(notes);
  await userEvent.click(within(header).getByRole('button', { name: actionName('actions', thread.name) }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('thread.header.view') }));
  expect(await screen.findByRole('menuitem', { name: i18n.t('running.title') })).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('opens the exact thread Changes from the header View menu in %s', async (locale) => {
  await setLocale(locale);
  const thread = { ...snapshot().thread, environment_permissions: ['files.view'] };
  show(<ThreadHeader thread={thread} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: actionName('actions', thread.name) }));
  const view = screen.getByRole('menuitem', { name: i18n.t('thread.header.view') });
  act(() => view.focus()); await user.keyboard('{ArrowRight}');
  await user.click(await screen.findByRole('menuitem', { name: i18n.t('views.changes.title') }));
  expect(changesView).toHaveBeenCalledExactlyOnceWith(thread.id, thread.current_turn_id ?? undefined);
});

async function openRunningFromView(user: ReturnType<typeof userEvent.setup>, thread: Thread) {
  await user.click(within(screen.getByRole('banner')).getByRole('button', { name: actionName('actions', thread.name) }));
  const view = screen.getByRole('menuitem', { name: i18n.t('thread.header.view') });
  act(() => view.focus());
  await user.keyboard('{ArrowRight}');
  await user.click(await screen.findByRole('menuitem', { name: i18n.t('running.title') }));
}

for (const change of ['view opening', 'reader authority', 'thread target', 'forbidden source'] as const) it(`retires the background review owner after ${change}`, async () => {
  const user = userEvent.setup(), thread = snapshot().thread;
  const content = (opening: string, target = thread) => <ViewOpeningContext value={opening}><ThreadHeader thread={target} /></ViewOpeningContext>;
  const view = show(content('first-opening'));
  await openRunningFromView(user, thread);
  expect(screen.getAllByRole('dialog', { name: i18n.t('running.title') })).toHaveLength(1);
  const before = client.session.getState();
  try {
    if (change === 'reader authority') act(() => client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1 }));
    if (change === 'forbidden source') calls.backgroundClosed = true;
    const other = change === 'thread target' ? { ...thread, id: 'other-thread', path: thread.path.map((part) => part.id === thread.id ? { ...part, id: 'other-thread' } : part) } : thread;
    view.rerender(content(change === 'view opening' ? 'second-opening' : 'first-opening', other));
    expect(screen.queryByRole('dialog', { name: i18n.t('running.title') })).not.toBeInTheDocument();
  } finally { act(() => client.session.setState(before)); }
});

it('registers one current background action for the existing finder and View menu', async () => {
  const user = userEvent.setup(), { thread } = draw();
  const command = commandsNow().find((entry) => entry.id === 'thread.running');
  expect(command).toBeDefined();
  if (command === undefined) throw new Error('The current header must register its running action');
  act(() => command.run('here'));
  expect(screen.getAllByRole('dialog', { name: i18n.t('running.title') })).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: i18n.t('common.close') }));
  await openRunningFromView(user, thread);
  expect(screen.getAllByRole('dialog', { name: i18n.t('running.title') })).toHaveLength(1);
});

it('returns a command opening without an element opener to the current header actions', async () => {
  const user = userEvent.setup(), { header, thread } = draw();
  expect(document.body).toHaveFocus();
  const command = commandsNow().find((entry) => entry.id === 'thread.running');
  if (command === undefined) throw new Error('The current header must register its running action');
  act(() => command.run('here'));
  await user.click(screen.getByRole('button', { name: i18n.t('common.close') }));
  await waitFor(() => expect(within(header).getByRole('button', { name: actionName('actions', thread.name) })).toHaveFocus());
});

describe('a thread header', () => {
  const importedSegment: Segment = {
    id: 'imported-history', ordinal: 1, agent: null, provider: null, environment: null,
    arrangement: 'separated', credential_path: 'forwarded', executor: null, tool_set: null,
    env_generation: 0, import_fidelity: 'structured', import_report: null, seq_updated: 1,
    started_at: '2026-10-28T12:00:00Z', started_by: null, ended_at: '2026-10-28T12:01:00Z', end_reason: 'imported',
  };

  it.each([['en', 'pointer'], ['en', 'keyboard'], ['zh-CN', 'pointer'], ['zh-CN', 'keyboard']] as const)('names imported history and opens the native setup chooser in %s by %s', async (locale, via) => {
    await setLocale(locale);
    calls.segments = [importedSegment];
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const { header, notes } = draw({ binding: null, state: 'blocked', display: executionDisplay('blocked', { blocked_reason: 'unbound' }) });
    expect(within(notes).getByText(t('thread.header.importedHistory'))).toBeVisible();
    expect(within(header).getByText(t('thread.header.needsSetup'))).toBeVisible();
    expect(within(header).queryByText(t('state.blocked'), { exact: true })).not.toBeInTheDocument();
    const setup = within(notes).getByRole('button', { name: t('thread.header.chooseAgentProvider') });
    if (via === 'pointer') await user.click(setup);
    else { act(() => setup.focus()); await user.keyboard('{Enter}'); }
    expect(screen.getByRole('dialog', { name: 'Setup Repair login' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Close setup' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('does not claim an unbound local thread was imported', () => {
    const { header, notes } = draw({ binding: null, state: 'blocked', display: executionDisplay('blocked', { blocked_reason: 'unbound' }) });
    expect(within(header).getByText('Needs setup')).toBeVisible();
    expect(within(notes).queryByText('Imported history')).not.toBeInTheDocument();
    expect(within(notes).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('keeps imported history visible after local setup without offering initial setup again', () => {
    calls.segments = [importedSegment];
    const { header, notes } = draw({ binding: snapshot().thread.binding });
    expect(within(notes).getByText('Imported history')).toBeVisible();
    expect(within(header).getByText('Idle', { exact: true })).toBeVisible();
    expect(within(notes).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('shows archived imported history without a setup action', () => {
    calls.segments = [importedSegment];
    const { header, notes } = draw({ binding: null, archived: true, display: executionDisplay('idle', { state: 'archived' }) });
    expect(within(notes).getByText('Imported history')).toBeVisible();
    expect(within(header).getByText('Archived', { exact: true })).toBeVisible();
    expect(within(notes).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('keeps initial setup unavailable to someone who only reads the imported thread', async () => {
    calls.segments = [importedSegment];
    const { notes } = draw({ binding: null, access: { ...memberAccess, permissions: ['thread.view'] } });
    const setup = within(notes).getByRole('button', { name: 'Choose agent and provider…' });
    expect(setup).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(setup);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it.each(['en', 'zh-CN'] as const)('gives the shared tree and tab glyph the same setup state in %s', async (locale) => {
    await setLocale(locale);
    show(<ThreadStateIcon display={executionDisplay('blocked', { blocked_reason: 'unbound' })} />);
    expect(screen.getByRole('img', { name: i18n.getFixedT(locale)('thread.header.needsSetup') })).toBeVisible();
  });

  it.each(['active', 'paused', 'stopped'] as const)('opens the existing %s goal from the session menu', async (status) => {
    draw({ goal: { status } as ThreadGoal });
    await userEvent.click(screen.getByRole('button', { name: 'Actions for Repair login' }));
    const edit = screen.getByRole('menuitem', { name: 'Edit goal…' });
    expect(edit).not.toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('menuitem', { name: 'Set a goal…' })).not.toBeInTheDocument();
    await userEvent.click(edit);
    expect(calls.goal).toHaveBeenCalledExactlyOnceWith('thread-1');
  });

  it.each(['en', 'zh-CN'] as const)('retains archived and side-thread goal actions with the slash command reason in %s', async (locale) => {
    await setLocale(locale);
    for (const state of ['archived', 'side'] as const) {
      const view = draw({ archived: state === 'archived', side_of: state === 'side' ? { id: 'main-thread', kind: 'thread', name: 'Main thread', redacted: false } : null, goal: null });
      await userEvent.click(within(view.header).getByRole('button', { name: actionName('actions', view.thread.name) }));
      const goal = screen.getByRole('menuitem', { name: i18n.t('goals.menu.set') });
      expect(goal).toHaveAttribute('aria-disabled', 'true');
      expect(goal).toHaveTextContent(i18n.t(state === 'archived' ? 'goals.act.archived' : 'goals.act.sideThread'));
      await userEvent.click(goal); expect(calls.goal).not.toHaveBeenCalled();
      view.unmount();
    }
  });

  it('keeps goal editing disabled for a reader who cannot drive', async () => {
    draw({ goal: { status: 'paused' } as ThreadGoal, access: { ...memberAccess, permissions: ['thread.view'] } });
    await userEvent.click(screen.getByRole('button', { name: 'Actions for Repair login' }));
    const edit = screen.getByRole('menuitem', { name: 'Edit goal…' });
    expect(edit).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(edit);
    expect(calls.goal).not.toHaveBeenCalled();
  });

  it('shows title, state, Find and the way of reading in one row, and nothing else of the old head', () => {
    const { header, thread } = draw();
    const head = within(header);
    expect(head.getByRole('heading', { name: thread.name })).toBeVisible();
    expect(head.getByText('Idle', { exact: true })).toBeVisible();
    const find = head.getByRole('button', { name: 'Find in the conversation' });
    expect(find).toBeVisible();
    expect(find.textContent).toBe('');
    expect(find).toHaveClass('size-(--control-sm)');
    // One row of the control height that never wraps: nothing in it may wrap or stack.
    expect(header).toHaveClass('h-(--control-md)');
    expect(header.className).not.toMatch(/flex-wrap|flex-col/);
    for (const name of ['Star', 'Notifications', 'Duplicate', 'Duplicate to…', 'Switch provider or agent…', 'Side thread…', i18n.t('thread.header.shareThread')]) {
      expect(head.queryByRole('button', { name })).not.toBeInTheDocument();
    }
    expect(head.queryByRole('group', { name: 'Thread actions' })).not.toBeInTheDocument();
    expect(head.queryByText('Add a description')).not.toBeInTheDocument();
    expect(head.queryByRole('button', { name: /Model and effort|Approval mode|Context|Edit links|Details/ })).not.toBeInTheDocument();
    expect(head.queryByText(/Run with provider|Separated|not_a_repository/)).not.toBeInTheDocument();
  });

  it('puts every other act of the head, and the description, in More', async () => {
    const user = userEvent.setup();
    const { header, thread } = draw();
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    for (const name of ['Duplicate', 'Duplicate to…', 'Switch provider or agent…', 'Side thread…', 'Find in the conversation', 'Star', i18n.t('thread.bell.label'), i18n.t('thread.header.shareThread'), 'Export', 'Archive', 'Delete']) expect(screen.getByRole('menuitem', { name })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'Organize' })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'View' })).toBeVisible();
    await user.click(screen.getByRole('menuitem', { name: i18n.t('thread.header.shareThread') }));
    expect(calls.share).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name });
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    await user.click(screen.getByRole('menuitem', { name: 'Add a description' }));
    expect(within(screen.getByRole('dialog')).getByRole('textbox')).toBeVisible();
    expect(within(header).queryByText('Add a description')).not.toBeInTheDocument();
  });

  it.each(['en', 'zh-CN'] as const)('runs the same fork, fork-to, switch, side-ask and find producers from the menu and the row in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const { header, thread } = draw();
    const choose = async (name: string) => {
      await user.click(within(header).getByRole('button', { name: actionName('actions', thread.name) }));
      await user.click(screen.getByRole('menuitem', { name }));
    };
    await choose(t('duplicate.now'));
    await confirmFork(user);
    expect(calls.fork).toHaveBeenCalledOnce();
    await choose(t('duplicate.to'));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'fork' });
    await choose(t('change.menu.switch'));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
    await choose(t('side.menu.ask'));
    expect(calls.ask).toHaveBeenCalledWith({ threadId: thread.id, threadName: thread.name, toTurn: null, text: '', read: defaultThreadRead(thread.id) });
    await user.click(within(header).getByRole('button', { name: t('search.find.command') }));
    expect(calls.find).toHaveBeenCalledWith(thread.id, undefined);
    await choose(t('search.find.command'));
    expect(calls.find).toHaveBeenCalledTimes(2);
  });

  it.each([false, true])('offers Star by its state in More, once (starred=%s)', async (starred) => {
    calls.starred = starred;
    const { header } = draw();
    expect(within(header).queryByRole('button', { name: starred ? 'Unstar' : 'Star' })).not.toBeInTheDocument();
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    expect(screen.getAllByRole('menuitem', { name: starred ? 'Unstar' : 'Star' })).toHaveLength(1);
  });

  it('keeps Duplicate in the menu with the one handler, outside the placement submenu', async () => {
    const { header } = draw();
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    const fork = screen.getByRole('menuitem', { name: /^Duplicate$/ });
    const submenu = screen.getByRole('menuitem', { name: 'Agent and environment' });
    act(() => submenu.focus());
    await userEvent.keyboard('{ArrowRight}');
    expect(screen.getAllByRole('menuitem', { name: /^Duplicate$/ })).toHaveLength(1);
    act(() => fork.focus()); await userEvent.keyboard('{Enter}');
    await confirmFork(userEvent.setup());
    expect(calls.fork).toHaveBeenCalledOnce();
  });

  it('does not offer a mutation the viewer is not offered, and keeps Find available', async () => {
    const { header } = draw({ access: { permissions: ['thread.view'] } as Thread['access'], can: { fork: false, place: false, archive: false, export: false } });
    expect(within(header).getByRole('button', { name: 'Find in the conversation' })).toBeVisible();
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    expect(screen.queryByRole('menuitem', { name: 'Duplicate' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Switch provider or agent…' })).not.toBeInTheDocument();
    expect(calls.fork).not.toHaveBeenCalled();
    expect(calls.change).not.toHaveBeenCalled();
    expect(calls.ask).not.toHaveBeenCalled();
  });

  it('keeps the page and Find readable when a cached row has no capabilities, without offering a guessed fork', async () => {
    const { header, notes, thread, rerender } = draw();
    Reflect.deleteProperty(thread, 'can');
    rerender(<ThreadHeader thread={thread} notes={notes} />);
    expect(within(header).getByRole('heading', { name: 'Repair login' })).toBeVisible();
    expect(within(header).getByRole('button', { name: 'Find in the conversation' })).toBeVisible();
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    expect(screen.queryByRole('menuitem', { name: 'Duplicate' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Duplicate to…' })).not.toBeInTheDocument();
    expect(calls.fork).not.toHaveBeenCalled();
  });

  it('shows only a present description’s first line in the notes and opens Edit directly from More', async () => {
    const user = userEvent.setup();
    const { header, notes } = draw({ description: 'Track the login repair\nKeep the tests green' });
    expect(within(notes).getByText('Track the login repair')).toBeVisible();
    expect(within(notes).queryByText('Keep the tests green')).not.toBeInTheDocument();
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    await user.click(screen.getByRole('menuitem', { name: 'Edit the description' }));
    expect(within(screen.getByRole('dialog')).getByRole('textbox')).toHaveValue('Track the login repair\nKeep the tests green');
  });

  it('does not assume an unknown question is answerable and leaves renaming on the title', async () => {
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 });
    expect(within(header).getByText('Waiting on an approval', { exact: true })).toBeVisible();
    await userEvent.click(within(header).getByRole('button', { name: /Rename/ }));
    expect(within(header).getByRole('textbox')).toHaveValue('Repair login');
  });

  it.each([['en', 'pointer'], ['en', 'keyboard'], ['zh-CN', 'pointer'], ['zh-CN', 'keyboard']] as const)('omits an unsupported aside while keeping both Fork actions in the menu in %s via %s', async (locale, via) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const { header } = draw({ sides: { open: 0, allowed: true, max: 2, mode: null, reason: 'The adapter cannot restrict this aside safely.' } });
    const open = () => user.click(within(header).getByRole('button', { name: /Actions|操作/ }));
    await open();
    const forkNow = screen.getByRole('menuitem', { name: t('duplicate.now') });
    expect(screen.getByRole('menuitem', { name: t('duplicate.to') })).toBeVisible();
    expect(screen.queryByRole('menuitem', { name: t('side.menu.ask') })).not.toBeInTheDocument();
    expect(calls.ask).not.toHaveBeenCalled();
    if (via === 'pointer') await user.click(forkNow);
    else { act(() => forkNow.focus()); await user.keyboard('{Enter}'); }
    await confirmFork(user);
    expect(calls.fork).toHaveBeenCalledOnce();
    await open();
    const forkTo = screen.getByRole('menuitem', { name: t('duplicate.to') });
    if (via === 'pointer') await user.click(forkTo);
    else { act(() => forkTo.focus()); await user.keyboard('{Enter}'); }
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread-1', name: 'Repair login' }, { kind: 'fork' });
  });

  it.each([true, false])('addresses the current approval’s actual decider (can decide: %s)', (canDecide) => {
    const request = harnessApproval('question', { threadId: 'thread-1', itemId: 'question-item', title: 'Continue?', request: approvalRequest, requester: userRef(me(1, 'ada', 'Ada')), canDecide });
    const base = approvalItem('question-item', request.id);
    if (base.kind !== 'approval') throw new Error('An approval item is required');
    calls.items.set(base.id, { ...base, payload: { ...base.payload, approval: request } });
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 });
    expect(within(header).getByText(canDecide ? 'Waiting for you' : 'Waiting for an answer', { exact: true })).toBeVisible();
    expect(within(header).queryByText(canDecide ? 'Waiting for an answer' : 'Waiting for you', { exact: true })).not.toBeInTheDocument();
  });

  for (const locale of ['en', 'zh-CN'] as const) it.each([true, false])(`reads a missing approval before naming its decider in ${locale} (can decide: %s)`, async (canDecide) => {
    await setLocale(locale);
    const request = harnessApproval('unloaded-question', { threadId: 'thread-1', itemId: 'unloaded-item', title: 'Continue?', request: approvalRequest, requester: userRef(me(1, 'ada', 'Ada')), canDecide });
    const item = approvalItem('unloaded-item', request.id);
    calls.items.set(item.id, item);
    let answer: (approval: Approval) => void = () => { throw new Error('The approval read must start first'); };
    calls.approval.mockImplementation(() => new Promise((resolve) => { answer = resolve; }));
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 });
    expect(within(header).getByText(i18n.t('thread.header.waitingOnApproval'), { exact: true })).toBeVisible();
    expect(within(header).queryByText(i18n.t('composer.row.waitingForYou'), { exact: true })).not.toBeInTheDocument();
    expect(within(header).queryByText(i18n.t('thread.header.waitingForAnswer'), { exact: true })).not.toBeInTheDocument();
    await waitFor(() => expect(calls.approval).toHaveBeenCalledExactlyOnceWith(request.id));
    await act(async () => { answer(request); await Promise.resolve(); });
    expect(await within(header).findByText(i18n.t(canDecide ? 'composer.row.waitingForYou' : 'thread.header.waitingForAnswer'), { exact: true })).toBeVisible();
    expect(within(header).queryByText(i18n.t('thread.header.waitingOnApproval'), { exact: true })).not.toBeInTheDocument();
  });

  it.each(['en', 'zh-CN'] as const)('keeps the neutral approval label after a failed read in %s', async (locale) => {
    await setLocale(locale);
    const item = approvalItem('unavailable-item', 'unavailable-question');
    calls.items.set(item.id, item);
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 });
    await waitFor(() => expect(calls.approval).toHaveBeenCalledExactlyOnceWith('unavailable-question'));
    expect(within(header).getByText(i18n.t('thread.header.waitingOnApproval'), { exact: true })).toBeVisible();
    expect(within(header).queryByText(i18n.t('thread.header.waitingForAnswer'), { exact: true })).not.toBeInTheDocument();
  });

  it('refreshes an older card’s loaded authority and follows its confirmed changes', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const request = harnessApproval('cached-question', { threadId: 'thread-1', itemId: 'cached-item', title: 'Continue?', request: approvalRequest, requester: userRef(me(1, 'ada', 'Ada')) });
    const base = approvalItem('cached-item', request.id);
    calls.items.set(base.id, base);
    cache.setQueryData(approvalKey(request.id), request);
    calls.approval.mockResolvedValue(request);
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 }, cache);
    expect(within(header).getByText('Waiting for you', { exact: true })).toBeVisible();
    await waitFor(() => expect(calls.approval).toHaveBeenCalledExactlyOnceWith(request.id));
    await waitFor(() => expect(cache.isFetching({ queryKey: approvalKey(request.id) })).toBe(0));
    act(() => { cache.setQueryData(approvalKey(request.id), { ...request, can_decide: false }); });
    await waitFor(() => expect(within(header).getByText('Waiting for an answer', { exact: true })).toBeVisible());
    expect(cache.isFetching({ queryKey: approvalKey(request.id) })).toBe(0);
  });

  it('stops asking the same person after a confirmed decision while the committed thread frame is on its way', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const request = harnessApproval('inline-question', { threadId: 'thread-1', itemId: 'inline-item', title: 'Continue?', request: approvalRequest, requester: userRef(me(1, 'ada', 'Ada')) });
    const base = approvalItem('inline-item', request.id);
    if (base.kind !== 'approval') throw new Error('The fixture is a request');
    calls.items.set(base.id, { ...base, payload: { ...base.payload, approval: request } });
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 }, cache);
    expect(within(header).getByText('Waiting for you', { exact: true })).toBeVisible();
    act(() => { cache.setQueryData(approvalKey(request.id), { ...request, state: 'approved', can_decide: false }); });
    await waitFor(() => expect(within(header).queryByText('Waiting for you', { exact: true })).not.toBeInTheDocument());
    expect(within(header).getByText('Waiting for an answer', { exact: true })).toBeVisible();
    expect(header.querySelector('[data-state]')).toHaveAttribute('data-state', 'waiting_approval');
    expect(cache.isFetching({ queryKey: approvalKey(request.id) })).toBe(0);
  });

  it('keeps all view settings under one submenu and retains their original callbacks', async () => {
    const user = userEvent.setup();
    const { header } = draw();
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    expect(screen.queryByRole('menuitemcheckbox')).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: 'Resume the automatic title' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('menuitem', { name: 'View' }));
    expect(screen.getByRole('menuitemcheckbox')).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'Resume the automatic title' })).toBeVisible();
    expect(screen.queryByRole('menuitem', { name: 'Resume the automatic description' })).not.toBeInTheDocument();
  });

  it('offers reading a present description to a viewer without an edit action', async () => {
    const { header } = draw({ description: 'A description to read', access: { permissions: ['thread.view'] } as Thread['access'] });
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    expect(screen.queryByRole('menuitem', { name: 'Edit the description' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('menuitem', { name: 'Show the whole description' }));
    const dialog = screen.getByRole('dialog', { name: 'Description' });
    expect(within(dialog).getByText('A description to read')).toBeVisible();
    expect(within(dialog).queryByRole('textbox')).not.toBeInTheDocument();
  });
});

it.each(['en', 'zh-CN'] as const)('binds recorded tasks, one scoped wake setting and reviewed End in %s', async (locale) => {
  await setLocale(locale);
  const task = backgroundWork();
  const { notes } = draw({ background: backgroundWake({ tasks: [task] }) });
  expect(within(notes).getByText('Build web')).toBeVisible();
  await userEvent.click(within(notes).getByRole('button', { name: i18n.t('backgroundWake.settingOn') }));
  expect(calls.settings.mock.lastCall?.[0]).toMatchObject({ target: { kind: 'node', id: 'thread-1' }, only: ['threads.background_wake'], scope: 'thread' });
  await userEvent.click(within(screen.getByRole('dialog', { name: i18n.t('backgroundWake.settingTitle') })).getByRole('button', { name: i18n.t('common.close') }));
  await userEvent.click(within(notes).getByRole('button', { name: i18n.t('backgroundWake.endNamed', { name: task.label }) }));
  expect(calls.review).toHaveBeenLastCalledWith({ task: { id: task.id, item_id: task.item_id, turn_id: task.turn_id } });
});

it('drops a forbidden live task observation without reusing its recorded End target', () => {
  calls.background = background({ wake: backgroundWake({ tasks: [backgroundWork({ label: 'Current private work' })] }) });
  const view = draw();
  expect(within(view.notes).getByText('Current private work')).toBeVisible();
  calls.backgroundClosed = true;
  view.rerender(<ThreadHeader thread={view.thread} notes={view.notes} />);
  expect(within(view.notes).queryByText('Current private work')).not.toBeInTheDocument();
});

it('retires the wake-setting opening across a target change and does not reopen it on return', async () => {
  const task = backgroundWork();
  const view = draw({ background: backgroundWake({ tasks: [task] }) });
  await userEvent.click(within(view.notes).getByRole('button', { name: i18n.t('backgroundWake.settingOn') }));
  expect(screen.getByRole('dialog', { name: i18n.t('backgroundWake.settingTitle') })).toBeVisible();
  view.rerender(<ThreadHeader thread={{ ...view.thread, id: 'other-thread' }} notes={view.notes} />);
  expect(screen.queryByRole('dialog', { name: i18n.t('backgroundWake.settingTitle') })).not.toBeInTheDocument();
  view.rerender(<ThreadHeader thread={view.thread} notes={view.notes} />);
  expect(screen.queryByRole('dialog', { name: i18n.t('backgroundWake.settingTitle') })).not.toBeInTheDocument();
});

const moves = (more: Partial<MessageNav> = {}): MessageNav => ({ messages: [], pinned: null, place: null, whole: true, canPrevious: true, canNext: false,
  previous: vi.fn(() => true), next: vi.fn(() => true), go: vi.fn(), moving: false, error: null, retry: vi.fn(), ...more });

it.each(['en', 'zh-CN'] as const)('draws the arrows and the Messages list beside the state, and the same moves in the View menu in %s', async (locale) => {
  await setLocale(locale);
  const nav = moves();
  const { header, thread } = draw({}, undefined, { nav });
  const head = within(header);
  const previous = head.getByRole('button', { name: i18n.t('thread.messages.previous') });
  expect(previous).toBeEnabled();
  expect(head.getByRole('button', { name: i18n.t('thread.messages.next') })).toBeDisabled();
  await userEvent.click(previous);
  expect(nav.previous).toHaveBeenCalledOnce();
  await userEvent.click(head.getByRole('button', { name: i18n.t('thread.messages.title') }));
  expect(await screen.findByText(i18n.t('thread.messages.none'))).toBeVisible();
  await userEvent.keyboard('{Escape}');
  await userEvent.click(head.getByRole('button', { name: actionName('actions', thread.name) }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('thread.header.view') }));
  expect(await screen.findByRole('menuitem', { name: i18n.t('thread.messages.previous') })).toBeVisible();
  expect(screen.getByRole('menuitem', { name: i18n.t('thread.messages.next') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.getByRole('menuitem', { name: i18n.t('thread.messages.title') })).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('shows how many agents the thread has or had with a dot, lists a finished one, and opens one or the main conversation in %s', async (locale) => {
  await setLocale(locale);
  const subagent = (id: string, name: string, state: 'running' | 'completed') => ({ ...toolCall(id, { kind: 'subagent', child_stream: `sub:${id}`, name, role: null, prompt: null, state }, { title: name, status: state === 'running' ? 'in_progress' : 'completed' }), thread_id: 'thread-1' });
  calls.items.set('sub-run', subagent('sub-run', 'runner', 'running'));
  calls.items.set('sub-done', subagent('sub-done', 'reviewer', 'completed'));
  const onAgent = vi.fn();
  const { header } = draw({}, undefined, { onAgent });
  const button = within(header).getByRole('button', { name: i18n.t('thread.agents.count', { count: 2 }) });
  expect(within(button).getByRole('img', { name: i18n.t('thread.agents.dot.working') })).toBeVisible();
  await userEvent.click(button);
  const active = await screen.findByRole('region', { name: i18n.t('thread.agents.active') });
  const done = screen.getByRole('region', { name: i18n.t('thread.agents.done') });
  expect(within(active).getByText('runner')).toBeVisible();
  // Finished ones stay listed.
  expect(within(done).getByText('reviewer')).toBeVisible();
  await userEvent.click(within(done).getByRole('button', { name: /reviewer/ }));
  expect(onAgent).toHaveBeenLastCalledWith('sub-done');
  await userEvent.click(button);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('thread.agents.main') }));
  expect(onAgent).toHaveBeenLastCalledWith(null);
});

it('shows no agents where the thread never had one', () => {
  const { header } = draw();
  expect(within(header).queryByRole('button', { name: /agents?$/ })).not.toBeInTheDocument();
});

it('reads the thread’s title, then the sub-agent’s name, in place of the title, with the title leading back and no moves between messages', async () => {
  const nav = moves();
  const onAgent = vi.fn();
  const { header, thread } = draw({}, undefined, { nav, agent: { id: 'sub-run', name: 'runner' }, onAgent });
  expect(within(header).getByRole('heading', { name: 'runner', level: 1 })).toBeVisible();
  expect(within(header).queryByRole('button', { name: i18n.t('thread.messages.previous') })).not.toBeInTheDocument();
  expect(within(header).queryByRole('button', { name: i18n.t('thread.messages.title') })).not.toBeInTheDocument();
  await userEvent.click(within(header).getByRole('button', { name: i18n.t('thread.agents.backTo', { name: thread.name }) }));
  expect(onAgent).toHaveBeenCalledExactlyOnceWith(null);
  // The title is no longer a field to rename while the sub-agent is shown.
  expect(within(header).queryByRole('button', { name: /Rename/ })).not.toBeInTheDocument();
});

it.each(['en', 'zh-CN'] as const)('keeps all reading modes operable through More when the narrow header folds their control in %s', async (locale) => {
  await setLocale(locale);
  reading.folded.add('view');
  const { header, thread } = draw();
  expect(header.querySelector('[data-fold="view"]')).toHaveAttribute('hidden');
  await userEvent.click(within(header).getByRole('button', { name: actionName('actions', thread.name) }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('thread.header.view') }));
  const normal = screen.getByRole('menuitemradio', { name: i18n.t('thread.viewModes.normal') });
  expect(normal).toHaveAttribute('aria-checked', 'true');
  expect(screen.getByRole('menuitemradio', { name: i18n.t('thread.viewModes.diagnostics') })).toBeVisible();
  await userEvent.click(screen.getByRole('menuitemradio', { name: i18n.t('thread.viewModes.verbose') }));
  expect(reading.setDetail).toHaveBeenCalledExactlyOnceWith('verbose');
});

it.each(['en', 'zh-CN'] as const)('opens the native move to another org from the existing compact menu in %s', async (locale) => {
  await setLocale(locale);
  const { header, thread } = draw();
  await userEvent.click(within(header).getByRole('button', { name: actionName('actions', thread.name) }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('thread.header.placement') }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('thread.moveOut.action') }));
  expect(calls.change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'move-out' });
});

it.each(['en', 'zh-CN'] as const)('shows only the counts of the turn that header Changes opens in %s', async (locale) => {
  await setLocale(locale);
  for (const [id, added] of [['one', 100], ['two', 1]] as const) {
    const item = fileChange(`edit-${id}`, { turn: id, changes: [change(`${id}.ts`, '+line', { added, removed: 0 })] });
    calls.items.set(item.id, item); headerRows.order.push(item.id);
    headerRows.turns.set(id, turn(id, { ordinal: id === 'one' ? 1 : 2, state: 'completed', inputId: `input-${id}` }));
  }
  const { header } = draw({ current_turn_id: null });
  await userEvent.click(within(header).getByRole('button', { name: i18n.t('thread.frame.changesCounts', { added: 1, removed: 0 }) }));
  expect(changesView).toHaveBeenCalledExactlyOnceWith('thread-1', 'two');
  expect(header).not.toHaveTextContent('+101');
});

it('marks header change counts as unknown when only the end of its target turn is held', () => {
  const item = fileChange('partial-edit', { turn: 'two', changes: [change('tail.ts', '+line')] });
  calls.items.set(item.id, item); headerRows.order.push(item.id); headerRows.hasOlder = true;
  headerRows.turns.set('two', turn('two', { ordinal: 2, state: 'completed', inputId: 'not-held' }));
  const { header } = draw({ current_turn_id: null });
  expect(within(header).getByRole('button', { name: i18n.t('thread.frame.changesUnknown') })).toHaveTextContent('–');
});

it('uses the main turn net diff for header counts without borrowing a child stream', () => {
  const raw = fileChange('component-edit', { turn: 'two', changes: [change('component.ts', '+old', { added: 20, removed: 0 })] });
  const child = fileChange('child-edit', { turn: 'two', stream: 'child:review', changes: [change('child.ts', '+child', { added: 900, removed: 0 })] });
  const snapshot = marker('net-diff', { marker: 'harness_changed', data: {
    program: 'Codex', change: 'turn_diff', subject: null, previous: { state: 'unsupported' }, expected: { state: 'unsupported' },
    value: { changes: [change('net.ts', '+net', { added: 4, removed: 1 })] }, changed: true, diverged: false,
    next_turn: false, turn_id: 'two', reported_at: '2026-11-14T10:00:00Z',
  } }, { turn: 'two' });
  for (const item of [raw, child, snapshot]) calls.items.set(item.id, item);
  headerRows.order.push(raw.id);
  headerRows.turns.set('two', turn('two', { ordinal: 2, state: 'completed', inputId: 'input-two' }));
  const { header } = draw({ current_turn_id: null });
  expect(within(header).getByRole('button', { name: i18n.t('thread.frame.changesCounts', { added: 4, removed: 1 }) })).toBeVisible();
  expect(header).not.toHaveTextContent('+920');
});
