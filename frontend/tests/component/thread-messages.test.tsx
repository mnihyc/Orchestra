/** A person finds their own messages: the sticky prompt names the turn in view, previous and next move between the people's messages by key and by button, and the list of Messages goes to one. */
import { act, cleanup, fireEvent, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useImperativeHandle } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { ThreadScreen } from '@/features/thread/Thread';
import { MessagesPanel, MessageArrows } from '@/features/thread/MessageParts';
import { useMessageNav } from '@/features/thread/messageMoves';
import type { TimelineHandle, TimelineProps } from '@/features/timeline/Timeline';
import type { Item, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { keyBook } from '@/shell/keys';
import { keyRows } from '@/lib/shortcuts';
import { ACME, me, userRef } from '../doubles/fixtures';
import { agentMessage, change, command, fileChange, turn, userMessage } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ reveal: vi.fn(), props: { current: undefined as TimelineProps | undefined } }));
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
  const nav = useMessageNav(native.id, { top: 'run', visible: new Set() }, onGo, () => undefined);
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
