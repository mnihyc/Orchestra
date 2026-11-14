/** A person finds their own messages: the sticky prompt names the turn in view, previous and next move between the people's messages by key and by button, and the list of Messages goes to one. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useImperativeHandle } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import type * as Threads from '@/api/threads';
import type { OutlineOptions } from '@/api/threads';
import { ThreadScreen } from '@/features/thread/Thread';
import { MessagesPanel, MessageArrows } from '@/features/thread/MessageParts';
import { useMessageNav } from '@/features/thread/messageMoves';
import type { TimelineHandle, TimelineProps } from '@/features/timeline/Timeline';
import type { Item, OutlineEntry, Thread, ThreadOutlinePage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { keyBook } from '@/shell/keys';
import { keyRows } from '@/lib/shortcuts';
import { ACME, me, userRef } from '../doubles/fixtures';
import { agentMessage, change, command, fileChange, turn, userMessage } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ reveal: vi.fn(), neighbors: vi.fn<typeof Threads.fetchMessageNeighbors>(), outline: vi.fn<(options: OutlineOptions, before: number | null) => Promise<ThreadOutlinePage>>(), props: { current: undefined as TimelineProps | undefined } }));
vi.mock('@/api/threads', async (load) => {
  const actual = await load<typeof Threads>();
  return { ...actual, fetchMessageNeighbors: calls.neighbors, outlineQuery: (target: Parameters<typeof actual.outlineQuery>[0], options: OutlineOptions = {}) => ({
    ...actual.outlineQuery(target, options), enabled: true,
    queryFn: ({ pageParam }: { pageParam: number | null }) => calls.outline(options, pageParam),
  }) };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: () => undefined, useOrgTreeFeed: () => undefined, useSocketStatus: () => ({ phase: 'ready' }) }));
vi.mock('@/shell/screen', async (load) => ({ ...await load<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
vi.mock('@/features/thread/Header', () => ({ ThreadHeader: ({ thread }: { thread: Thread }) => <header aria-label="Thread"><h1>{thread.name}</h1></header> }));
vi.mock('@/features/composer/Composer', () => ({ Composer: () => <textarea aria-label="Thread draft" /> }));
vi.mock('@/features/composer/QueueTray', () => ({ QueueTray: () => null }));
vi.mock('@/features/schedules/Schedules', () => ({ PendingAgentSchedules: () => null }));
vi.mock('@/features/thread/ConnectionBanner', () => ({ ConnectionBanner: () => null }));
vi.mock('@/features/thread/BlockedBanner', () => ({ BlockedBanner: () => null }));
vi.mock('@/features/thread/LockBanner', () => ({ LockBanner: () => null }));
vi.mock('@/features/thread/ContinuationNotice', () => ({ ContinuationNotice: () => null }));
vi.mock('@/features/source/Preparation', () => ({ SourcePreparation: () => null }));
vi.mock('@/features/source/Review', () => ({ ReviewTray: () => null }));
vi.mock('@/features/gateways/ThreadGatewayStatus', () => ({ ThreadGatewayStatus: () => null }));
vi.mock('@/features/timeline/FindInConversation', () => ({ FindInConversation: () => null }));
vi.mock('@/features/timeline/StatusLine', () => ({ StatusLine: () => null }));
vi.mock('@/features/ports/LocalAddresses', () => ({ LocalAddresses: ({ children }: { children: ReactNode }) => children }));
// The list is what tells the screen where the reader is; the case says it, as the browser would.
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: (props: TimelineProps) => {
  calls.props.current = props;
  useImperativeHandle(props.ref, (): TimelineHandle => ({ scrollToItem: calls.reveal, scrollToEnd: () => undefined, visibleItemIds: () => new Set() }));
  return <p>The conversation</p>;
} }));

const ADA = userRef(me(1, 'ada', 'Ada'));
const VERA = userRef(me(2, 'vera', 'Vera'));
let native: Thread;
const stamp = (item: Item, seq: number): Item => ({ ...item, thread_id: native.id, seq_created: seq, seq_updated: seq });
beforeEach(() => {
  calls.reveal.mockReset();
  calls.outline.mockReset().mockResolvedValue({ entries: [], next_before: null, seq: 12 });
  calls.neighbors.mockReset();
  const world = new World(); const person = world.people.get('ada');
  const node = world.places.rowOf(ACME.mainThread);
  if (person === undefined || node === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, node), state: 'idle', current_turn_id: null };
  threadWriter.reset(native.id, { thread: native, turns: [turn('t1', { ordinal: 1, state: 'completed', inputId: 'one-input' }), turn('t2', { ordinal: 2, state: 'completed', inputId: 'two-input' })], items: [
    stamp(userMessage('one', 'Work in a new directory', { turn: 't1', author: ADA }), 2),
    stamp(agentMessage('says', 'I will start.', { turn: 't1', phase: 'commentary' }), 3),
    stamp(fileChange('edit', { turn: 't1', changes: [change('a.py', null, { added: 10, removed: 3 })] }), 4),
    stamp(agentMessage('answer-one', 'Done.', { turn: 't1' }), 5),
    stamp(userMessage('two', 'Now run the tests and fix what fails', { turn: 't2', author: VERA }), 10),
    stamp(command('run', { turn: 't2', command: 'npm test', exit: 0 }), 11),
    stamp(agentMessage('answer-two', 'All pass.', { turn: 't2' }), 12),
  ] }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(native.id); await setLocale('en'); });

const where = (top: string | null, visible: string[]) => act(() => { calls.props.current?.onTop?.(top); calls.props.current?.onVisible?.(new Set(visible)); });
const ctrlAlt = (key: 'ArrowUp' | 'ArrowDown') => new KeyboardEvent('keydown', { key, ctrlKey: true, altKey: true, cancelable: true });

it.each(['en', 'zh-CN'] as const)('names the turn in view in a dim line once its message has scrolled above the view, with who said it, where it stands, and arrows in %s', async (locale) => {
  await setLocale(locale);
  show(<ThreadScreen threadId={native.id} item={null} />);
  expect(document.querySelector('[data-sticky-prompt]')).toBeNull();
  where('run', ['run', 'answer-two']);
  const prompt = document.querySelector<HTMLElement>('[data-sticky-prompt]');
  expect(prompt).not.toBeNull();
  expect(prompt).toHaveTextContent('Vera');
  expect(prompt).toHaveTextContent('Now run the tests and fix what fails');
  expect(prompt).toHaveTextContent(i18n.t('thread.messages.place', { at: 2, total: 2 }));
  expect(within(prompt!).getByRole('button', { name: i18n.t('thread.messages.previous') })).toBeEnabled();
  expect(within(prompt!).getByRole('button', { name: i18n.t('thread.messages.next') })).toBeDisabled();
  // The message in view is its own prompt: nothing stands above it.
  where('two', ['two', 'run']);
  expect(document.querySelector('[data-sticky-prompt]')).toBeNull();
  // The turn before it: the first message names it.
  where('edit', ['edit', 'answer-one']);
  expect(document.querySelector('[data-sticky-prompt]')).toHaveTextContent('Work in a new directory');
});

it('goes to the message when the prompt is pressed, without lighting it', async () => {
  show(<ThreadScreen threadId={native.id} item={null} />);
  where('run', ['run']);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.messages.goTo', { line: 'Now run the tests and fix what fails' }) }));
  expect(calls.reveal).toHaveBeenCalledExactlyOnceWith('two', { align: 'start', highlight: false });
});

it('moves to the previous and the next message of a person with Ctrl+Alt+Up and Ctrl+Alt+Down, and lists the keys', () => {
  show(<ThreadScreen threadId={native.id} item={null} />);
  where('run', ['run']);
  const press = (key: 'ArrowUp' | 'ArrowDown') => {
    let handled = false;
    act(() => { handled = keyBook.handle(ctrlAlt(key)); });
    return handled;
  };
  expect(press('ArrowUp')).toBe(true);
  expect(calls.reveal).toHaveBeenLastCalledWith('two', { align: 'start', highlight: false });
  // Keys that repeat do not wait for the list to say where the view went: the next press counts from the message just moved to.
  expect(press('ArrowUp')).toBe(true);
  expect(calls.reveal).toHaveBeenLastCalledWith('one', { align: 'start', highlight: false });
  // Nothing is earlier than the first message: the key goes on to the page.
  expect(press('ArrowUp')).toBe(false);
  // Once the list says that the view went elsewhere, the keys count from the top of the view.
  where('answer-one', ['answer-one']);
  expect(press('ArrowDown')).toBe(true);
  expect(calls.reveal).toHaveBeenLastCalledWith('two', { align: 'start', highlight: false });
  const row = keyRows(keyBook.list()).find((entry) => entry.what === 'thread.messages.keys');
  expect(row).toMatchObject({ first: 'Mod+Alt+ArrowUp', last: 'Mod+Alt+ArrowDown' });
});

it('leaves the keys to the thread that has the keyboard, and takes them away with the screen', () => {
  const view = show(<ThreadScreen threadId={native.id} item={null} />);
  where('run', ['run']);
  expect(keyBook.list().map((binding) => binding.id)).toEqual(expect.arrayContaining(['thread-previous-message', 'thread-next-message']));
  view.unmount();
  expect(keyBook.list().map((binding) => binding.id)).not.toContain('thread-previous-message');
});

function Harness({ onGo }: { onGo: (id: string) => void }) {
  const nav = useMessageNav(native.id, { top: 'run', visible: new Set() }, onGo);
  return <><MessageArrows nav={nav} /><MessagesPanel threadId={native.id} nav={nav} onGo={onGo} /></>;
}

it.each(['en', 'zh-CN'] as const)('lists only the people’s messages, with who said them, when, the first line and what their turn changed, searchable, and goes to the one chosen in %s', async (locale) => {
  await setLocale(locale);
  const go = vi.fn();
  show(<Harness onGo={go} />);
  const list = screen.getByRole('list', { name: i18n.t('thread.messages.list') });
  expect(within(list).getAllByRole('listitem')).toHaveLength(2);
  expect(within(list).getByText('Work in a new directory')).toBeVisible();
  expect(within(list).queryByText('I will start.')).not.toBeInTheDocument();
  // Two people wrote: the sender is named; the lines the first turn changed stand with its message.
  expect(within(list).getByText('Ada')).toBeVisible();
  expect(within(list).getByText('Vera')).toBeVisible();
  expect(within(list).getByText('+10')).toBeVisible();
  await userEvent.type(screen.getByRole('textbox', { name: i18n.t('thread.messages.search') }), 'tests');
  expect(within(screen.getByRole('list', { name: i18n.t('thread.messages.list') })).getAllByRole('listitem')).toHaveLength(1);
  await userEvent.click(within(screen.getByRole('list', { name: i18n.t('thread.messages.list') })).getByRole('button', { name: /Now run the tests/ }));
  expect(go).toHaveBeenCalledExactlyOnceWith('two');
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('thread.messages.search') }), { target: { value: 'nothing of the kind' } });
  expect(screen.getByText(i18n.t('thread.messages.noMatch'))).toBeVisible();
});

const earlier = (): OutlineEntry => ({ item_id: 'earlier', seq: 1, at: '2026-10-15T00:00:00Z', title: 'An earlier question', kind: 'user_message',
  message: { author: ADA, source: 'user', turn_id: 'old-turn', added: 7, removed: 2, phase: null, last_agent_seq: null, last_final_seq: null, last_work_seq: null, ended: true } });

it('reads every page of older messages on demand and keeps the current list when another page fails', async () => {
  threadWriter.window(native.id, [], true, true);
  calls.outline.mockResolvedValueOnce({ entries: [], next_before: 100, seq: 12 })
    .mockRejectedValueOnce(new Error('The next page is unavailable'))
    .mockResolvedValueOnce({ entries: [earlier()], next_before: null, seq: 12 });
  show(<Harness onGo={calls.reveal} />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByText(i18n.t('thread.messages.none'))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByText('An earlier question')).toBeVisible();
  expect(screen.getByText('Ada')).toBeVisible();
  expect(screen.getByText('+7')).toBeVisible();
  expect(screen.getByText('−2')).toBeVisible();
  expect(calls.outline.mock.calls.map(([, before]) => before)).toEqual([null, 100, 100]);
  expect(screen.getByRole('list', { name: i18n.t('thread.messages.list') }).parentElement).toHaveClass('overflow-y-auto');
});

it.each(['a distinctive second line', 'ada'])('finds an older prompt by its full words or sender through the history projection: %s', async (words) => {
  threadWriter.window(native.id, [], true, false);
  calls.outline.mockImplementation((options) => Promise.resolve({ entries: options.q === words ? [earlier()] : [], next_before: null, seq: 12 }));
  show(<Harness onGo={calls.reveal} />);
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('thread.messages.search') }), { target: { value: words } });
  expect(await screen.findByText('An earlier question')).toBeVisible();
  await waitFor(() => expect(calls.outline).toHaveBeenCalledWith(expect.objectContaining({ selection: 'messages', q: words }), null));
  expect(screen.getByText('Ada')).toBeVisible();
  expect(screen.getByText('+7')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: /An earlier question/ }));
  expect(calls.reveal).toHaveBeenCalledWith('earlier');
});

it('does not call an early window the whole chat and moves to the actual next prompt beyond it', async () => {
  const held = threadViewOf(native.id);
  threadWriter.window(native.id, [...held.items.values()].filter((item) => item.seq_created < 6), false, true);
  calls.neighbors.mockResolvedValue({ entries: [{ ...earlier(), item_id: 'beyond', seq: 20 }], next_before: null, seq: 30 });
  show(<ThreadScreen threadId={native.id} item={null} />);
  where('edit', ['edit']);
  expect(document.querySelector('[data-sticky-prompt]')).not.toHaveTextContent(i18n.t('thread.messages.place', { at: 1, total: 1 }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.messages.next') }));
  await waitFor(() => expect(calls.reveal).toHaveBeenCalledWith('beyond', { align: 'start', highlight: false }));
  expect(calls.neighbors).toHaveBeenCalledWith(expect.objectContaining({ threadId: native.id }), 'edit', expect.any(AbortSignal));
});

it('goes to the actual previous prompt outside the tail rather than scrolling to an arbitrary page edge', async () => {
  const held = threadViewOf(native.id);
  threadWriter.window(native.id, [...held.items.values()].filter((item) => item.seq_created >= 10), true, false);
  calls.neighbors.mockResolvedValue({ entries: [earlier()], next_before: null, seq: 12 });
  show(<Harness onGo={calls.reveal} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.messages.previous') }));
  expect(calls.reveal).toHaveBeenLastCalledWith('two');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.messages.previous') }));
  await waitFor(() => expect(calls.reveal).toHaveBeenLastCalledWith('earlier'));
  expect(calls.neighbors).toHaveBeenCalledWith(expect.objectContaining({ threadId: native.id }), 'two', expect.any(AbortSignal));
});
