import { executionDisplay } from '../doubles/thread-display';
/** Frequent thread acts stay visible and unique; the session menu keeps the remaining actions. */
import { defaultThreadRead } from '@/realtime/threadScope';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ThreadHeader } from '@/features/thread/Header';
import type * as NodesApi from '@/api/nodes';
import type * as ThreadsApi from '@/api/threads';
import type * as ThreadStore from '@/realtime/stores/thread';
import { approvalKey } from '@/api/approvals';
import type { HarnessRequest, Item, Segment, Thread, ThreadGoal } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { show } from './support';
import { snapshot } from '../unit/continuation-fixtures';
import { ALLOW_DENY, approvalItem, harnessApproval } from '../doubles/thread-rows';
import { me, memberAccess, userRef } from '../doubles/fixtures';

const calls = vi.hoisted(() => ({ share: vi.fn(), update: vi.fn(), apply: vi.fn(), fork: vi.fn(), change: vi.fn(), ask: vi.fn(), find: vi.fn(), goal: vi.fn(), starred: false, items: new Map<string, Item>(), segments: [] as Segment[] }));
vi.mock('@/features/goal/editor', () => ({ openGoalEditor: calls.goal }));
vi.mock('@/api/nodes', async (importOriginal) => ({ ...await importOriginal<typeof NodesApi>(), updateNode: calls.update }));
vi.mock('@/api/threads', async (importOriginal) => ({ ...await importOriginal<typeof ThreadsApi>(), resumeAutomatic: vi.fn() }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: calls.share }));
vi.mock('@/features/navigation/favorites', () => ({ StarButton: () => <button>{calls.starred ? 'Unstar' : 'Star'}</button> }));
vi.mock('@/features/thread/SubscriptionBell', () => ({ SubscriptionBell: () => <button>Notifications</button> }));
vi.mock('@/features/navigation/NodeMenu', () => ({ useNodeMenu: () => [calls.starred ? 'Unstar' : 'Star', 'Export', 'Archive', 'Delete'].map((label) => ({ id: label.toLowerCase(), label, onSelect: vi.fn() })) }));
vi.mock('@/features/thread/forkNow', () => ({ useForkNow: () => ({ pending: false, start: calls.fork }) }));
vi.mock('@/features/change/target', () => ({ openChange: calls.change }));
vi.mock('@/features/side-threads/ask', () => ({ openSideAsk: calls.ask }));
vi.mock('@/features/side-threads/SideChips', () => ({ SideThreadList: () => <p>Open side threads</p> }));
vi.mock('@/features/thread-move/MoveToTopic', () => ({ MoveToTopic: () => null }));
vi.mock('@/features/start/SetUpThread', () => ({ SetUpThread: ({ thread, onClose }: { thread: Thread; onClose: () => void }) => <div role="dialog" aria-label={`Setup ${thread.name}`}><button onClick={onClose}>Close setup</button></div> }));
vi.mock('@/features/timeline/FindInConversation', () => ({ openFind: calls.find }));
vi.mock('@/features/inbox/away/Catchup', () => ({ CatchupRequest: () => null }));
vi.mock('@/features/running/Running', () => ({ RunningDialog: () => <div role="dialog">Running commands</div> }));
vi.mock('@/features/timeline/preferences', () => ({ useTimelinePreferences: () => ({ detail: 'steps', hideHistory: false, setDetail: vi.fn(), setHideHistory: vi.fn() }) }));
vi.mock('@/realtime/stores/thread', async (importOriginal) => {
  const actual = await importOriginal<typeof ThreadStore>();
  return { ...actual, runningTurn: () => null,
    useThreadPart: (_id: string, select: (view: typeof actual.EMPTY_VIEW) => unknown) => select({ ...actual.EMPTY_VIEW, items: calls.items, segments: calls.segments }),
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

beforeEach(() => { vi.clearAllMocks(); calls.starred = false; calls.items.clear(); calls.segments = []; });
afterEach(async () => { cleanup(); await setLocale('en'); });

function draw(more: Partial<Thread> = {}, cache?: QueryClient) {
  const thread = { id: 'thread-1', kind: 'thread', name: 'Repair login', revision: 4, description: '', title_source: 'user', description_source: 'user', auto_updated_at: null,
    org_id: 'org-1', parent_id: 'topic-1', topic_id: 'topic-1', path,
    display: executionDisplay(more.state ?? 'idle'), state: 'idle', lock: null, blocked: null, archived: false, page: 'default', side_of: null, sides: { open: 0, allowed: true, max: 2, mode: 'read_only', reason: null }, pending_approvals: 0,
    owner: { id: 'ada', display_name: 'Ada' }, access: { permissions: ['thread.view', 'thread.interact', 'thread.manage'] }, can: { fork: true, place: true, archive: true },
    ...more } as unknown as Thread;
  const content = <ThreadHeader thread={thread} />;
  const drawn = show(cache === undefined ? content : <QueryClientProvider client={cache}>{content}</QueryClientProvider>);
  return { thread, header: screen.getByRole('banner'), ...drawn };
}

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
    const { header } = draw({ binding: null, state: 'blocked', display: executionDisplay('blocked', { blocked_reason: 'unbound' }) });
    expect(within(header).getByText(t('thread.header.importedHistory'))).toBeVisible();
    expect(within(header).getByText(t('thread.header.needsSetup'))).toBeVisible();
    expect(within(header).queryByText(t('state.blocked'), { exact: true })).not.toBeInTheDocument();
    const setup = within(header).getByRole('button', { name: t('thread.header.chooseAgentProvider') });
    if (via === 'pointer') await user.click(setup);
    else { act(() => setup.focus()); await user.keyboard('{Enter}'); }
    expect(screen.getByRole('dialog', { name: 'Setup Repair login' })).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Close setup' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('does not claim an unbound local thread was imported', () => {
    const { header } = draw({ binding: null, state: 'blocked', display: executionDisplay('blocked', { blocked_reason: 'unbound' }) });
    expect(within(header).getByText('Needs setup')).toBeVisible();
    expect(within(header).queryByText('Imported history')).not.toBeInTheDocument();
    expect(within(header).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('keeps imported history visible after local setup without offering initial setup again', () => {
    calls.segments = [importedSegment];
    const { header } = draw({ binding: snapshot().thread.binding });
    expect(within(header).getByText('Imported history')).toBeVisible();
    expect(within(header).getByText('Idle', { exact: true })).toBeVisible();
    expect(within(header).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('shows archived imported history without a setup action', () => {
    calls.segments = [importedSegment];
    const { header } = draw({ binding: null, archived: true, display: executionDisplay('idle', { state: 'archived' }) });
    expect(within(header).getByText('Imported history')).toBeVisible();
    expect(within(header).getByText('Archived', { exact: true })).toBeVisible();
    expect(within(header).queryByRole('button', { name: 'Choose agent and provider…' })).not.toBeInTheDocument();
  });

  it('keeps initial setup unavailable to someone who only reads the imported thread', async () => {
    calls.segments = [importedSegment];
    const { header } = draw({ binding: null, access: { ...memberAccess, permissions: ['thread.view'] } });
    const setup = within(header).getByRole('button', { name: 'Choose agent and provider…' });
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

  it('keeps goal editing disabled for a reader who cannot drive', async () => {
    draw({ goal: { status: 'paused' } as ThreadGoal, access: { ...memberAccess, permissions: ['thread.view'] } });
    await userEvent.click(screen.getByRole('button', { name: 'Actions for Repair login' }));
    const edit = screen.getByRole('menuitem', { name: 'Edit goal…' });
    expect(edit).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(edit);
    expect(calls.goal).not.toHaveBeenCalled();
  });

  it('shows title, state and the frequent acts without runtime or next-turn chip rows', async () => {
    const { header, thread } = draw();
    const head = within(header);
    expect(head.getByRole('heading', { name: thread.name })).toBeVisible();
    expect(head.getByText('Idle', { exact: true })).toBeVisible();
    expect(head.getByRole('button', { name: 'Star' })).toBeVisible();
    expect(head.getByRole('button', { name: 'Notifications' })).toBeVisible();
    const frequent = head.getByRole('group', { name: 'Thread actions' });
    for (const name of ['Fork', 'Fork to…', 'Switch provider or agent…', 'Ask on the side…', 'Find in the conversation']) {
      const button = within(frequent).getByRole('button', { name });
      expect(button).toBeVisible();
      expect(button.textContent).toBe('');
      expect(button.querySelector('svg')).not.toBeNull();
      expect(button).toHaveClass('size-(--control-sm)');
    }
    expect(within(frequent).getAllByRole('button')).toHaveLength(5);
    expect(frequent.closest('[data-thread-actions-row]')).toContainElement(head.getByRole('button', { name: 'Share' }));
    expect(head.queryByText('Add a description')).not.toBeInTheDocument();
    expect(head.queryByRole('button', { name: /Model and effort|Approval mode|Context|Edit links|Details/ })).not.toBeInTheDocument();
    expect(head.queryByText(/Run with provider|Separated|not_a_repository/)).not.toBeInTheDocument();
    await userEvent.click(head.getByRole('button', { name: 'Share' }));
    expect(calls.share).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name });
  });

  it('puts adding a description and the existing session actions in More', async () => {
    const user = userEvent.setup();
    const { header } = draw();
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    for (const name of ['Export', 'Archive', 'Delete']) expect(screen.getByRole('menuitem', { name })).toBeVisible();
    for (const name of ['Fork', 'Fork to…', 'Switch provider or agent…', 'Ask on the side…', 'Find in the conversation', 'Star']) expect(screen.queryByRole('menuitem', { name })).not.toBeInTheDocument();
    expect(within(screen.getByRole('menu')).getAllByRole('menuitem').length).toBeLessThanOrEqual(10);
    expect(screen.getByRole('menuitem', { name: 'Organize' })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'View' })).toBeVisible();
    await user.click(screen.getByRole('menuitem', { name: 'Add a description' }));
    expect(within(screen.getByRole('dialog')).getByRole('textbox')).toBeVisible();
    expect(within(header).queryByText('Add a description')).not.toBeInTheDocument();
  });

  it.each(['en', 'zh-CN'] as const)('runs the same fork, fork-to, switch, side-ask and find producers directly from the header in %s', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const user = userEvent.setup();
    const { header, thread } = draw();
    const actions = within(header).getByRole('group', { name: t('thread.header.quickActions') });
    await user.click(within(actions).getByRole('button', { name: t('change.menu.forkNow') }));
    expect(calls.fork).toHaveBeenCalledOnce();
    await user.click(within(actions).getByRole('button', { name: t('change.menu.fork') }));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'fork' });
    await user.click(within(actions).getByRole('button', { name: t('change.menu.switch') }));
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
    await user.click(within(actions).getByRole('button', { name: t('side.menu.ask') }));
    expect(calls.ask).toHaveBeenCalledWith({ threadId: thread.id, threadName: thread.name, toTurn: null, text: '', read: defaultThreadRead(thread.id) });
    await user.click(within(actions).getByRole('button', { name: t('search.find.command') }));
    expect(calls.find).toHaveBeenCalledWith(thread.id, undefined);
  });

  it.each([false, true])('keeps the compact Star button and repeats its name in Organize (starred=%s)', async (starred) => {
    calls.starred = starred;
    const { header } = draw();
    expect(within(header).getAllByRole('button', { name: starred ? 'Unstar' : 'Star' })).toHaveLength(1);
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    const organize = screen.getByRole('menuitem', { name: 'Organize' });
    act(() => organize.focus());
    await userEvent.keyboard('{ArrowRight}');
    expect(await screen.findByRole('menuitem', { name: starred ? 'Unstar' : 'Star' })).toBeVisible();
  });

  it('repeats the visible Fork action by name with the same handler in the menu', async () => {
    const { header } = draw();
    expect(within(header).getByRole('button', { name: /^Fork$/ })).toBeDefined();
    await userEvent.click(within(header).getByRole('button', { name: /Actions/ }));
    const submenu = screen.getByRole('menuitem', { name: 'Agent and environment' });
    act(() => submenu.focus());
    await userEvent.keyboard('{ArrowRight}');
    const fork = await screen.findByRole('menuitem', { name: /^Fork$/ });
    act(() => fork.focus()); await userEvent.keyboard('{Enter}');
    expect(calls.fork).toHaveBeenCalledOnce();
  });

  it('does not promote a mutation the viewer is not offered, and keeps Find available', () => {
    const { header } = draw({ access: { permissions: ['thread.view'] } as Thread['access'], can: { fork: false, place: false, archive: false, export: false } });
    const actions = within(header).getByRole('group', { name: 'Thread actions' });
    expect(within(actions).getAllByRole('button')).toHaveLength(1);
    expect(within(actions).getByRole('button', { name: 'Find in the conversation' })).toBeVisible();
    expect(calls.fork).not.toHaveBeenCalled();
    expect(calls.change).not.toHaveBeenCalled();
    expect(calls.ask).not.toHaveBeenCalled();
  });

  it('keeps the page and Find readable when a cached row has no capabilities, without offering a guessed fork', () => {
    const { header, thread, rerender } = draw();
    Reflect.deleteProperty(thread, 'can');
    rerender(<ThreadHeader thread={thread} />);
    expect(within(header).getByRole('heading', { name: 'Repair login' })).toBeVisible();
    expect(within(header).getByRole('button', { name: 'Find in the conversation' })).toBeVisible();
    expect(within(header).queryByRole('button', { name: 'Fork' })).not.toBeInTheDocument();
    expect(within(header).queryByRole('button', { name: 'Fork to…' })).not.toBeInTheDocument();
    expect(calls.fork).not.toHaveBeenCalled();
  });

  it('shows only a present description’s first line below the title and opens Edit directly from More', async () => {
    const user = userEvent.setup();
    const { header } = draw({ description: 'Track the login repair\nKeep the tests green' });
    expect(within(header).getByText('Track the login repair')).toBeVisible();
    expect(within(header).queryByText('Keep the tests green')).not.toBeInTheDocument();
    await user.click(within(header).getByRole('button', { name: /Actions/ }));
    await user.click(screen.getByRole('menuitem', { name: 'Edit the description' }));
    expect(within(screen.getByRole('dialog')).getByRole('textbox')).toHaveValue('Track the login repair\nKeep the tests green');
  });

  it('does not assume an unknown question is answerable and leaves renaming on the title', async () => {
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 });
    expect(within(header).getByText('Waiting for an answer', { exact: true })).toBeVisible();
    await userEvent.click(within(header).getByRole('button', { name: /Rename/ }));
    expect(within(header).getByRole('textbox')).toHaveValue('Repair login');
  });

  it.each([['en', 'pointer'], ['en', 'keyboard'], ['zh-CN', 'pointer'], ['zh-CN', 'keyboard']] as const)('omits an unsupported aside while keeping both Fork actions in %s via %s', async (locale, via) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale), user = userEvent.setup();
    const { header } = draw({ sides: { open: 0, allowed: true, max: 2, mode: null, reason: 'The adapter cannot restrict this aside safely.' } });
    const actions = within(header).getByRole('group', { name: t('thread.header.quickActions') });
    const buttons = within(actions).getAllByRole('button');
    expect(buttons[0]).toHaveAccessibleName(t('change.menu.forkNow'));
    expect(buttons[1]).toHaveAccessibleName(t('change.menu.fork'));
    expect(within(actions).queryByRole('button', { name: t('side.menu.ask') })).not.toBeInTheDocument();
    expect(calls.ask).not.toHaveBeenCalled();
    if (via === 'pointer') await user.click(buttons[0]!);
    else { act(() => buttons[0]!.focus()); await user.keyboard('{Enter}'); }
    expect(calls.fork).toHaveBeenCalledOnce();
    if (via === 'pointer') await user.click(buttons[1]!);
    else { act(() => buttons[1]!.focus()); await user.keyboard('{Enter}'); }
    expect(calls.change).toHaveBeenCalledWith({ kind: 'thread', id: 'thread-1', name: 'Repair login' }, { kind: 'fork' });
    await user.click(within(header).getByRole('button', { name: /Actions|操作/ }));
    expect(screen.queryByRole('menuitem', { name: t('change.menu.fork') })).not.toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: t('side.menu.ask') })).not.toBeInTheDocument();
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

  it('follows an older card’s already loaded authority without another approval request', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const request = harnessApproval('cached-question', { threadId: 'thread-1', itemId: 'cached-item', title: 'Continue?', request: approvalRequest, requester: userRef(me(1, 'ada', 'Ada')) });
    const base = approvalItem('cached-item', request.id);
    calls.items.set(base.id, base);
    cache.setQueryData(approvalKey(request.id), request);
    const { header } = draw({ state: 'waiting_approval', pending_approvals: 1 }, cache);
    expect(within(header).getByText('Waiting for you', { exact: true })).toBeVisible();
    expect(cache.isFetching({ queryKey: approvalKey(request.id) })).toBe(0);
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
    expect(screen.queryByRole('menuitem', { name: 'Work detail' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('menuitem', { name: 'View' }));
    expect(screen.getByRole('menuitemradio', { name: 'Steps' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByRole('menuitemcheckbox')).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'Resume the automatic title' })).toBeVisible();
    expect(screen.getByRole('menuitem', { name: 'Resume the automatic description' })).toBeVisible();
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
