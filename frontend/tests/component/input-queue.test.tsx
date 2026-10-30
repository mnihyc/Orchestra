/** All waiting inputs share one editable queue; every action preserves the input and its holds. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ApiError, client } from '@/api/client';
import type * as ThreadsApi from '@/api/threads';
import { useComposerStore } from '@/features/composer/Draft';
import { QueueTray } from '@/features/composer/QueueTray';
import { QUEUE_HELP } from '@/help/uploads';
import { queuedInputs } from '@/features/timeline/rows';
import type { ContentPart, Thread, ThreadInput } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ShortcutBook } from '@/lib/shortcuts';
import { comparePositions, threadViewOf, threadWriter, useThreadView } from '@/realtime/stores/thread';
import { defaultThreadRead } from '@/realtime/threadScope';
import { KEY_ACTIONS } from '@/shell/keyActions';
import { me, userRef } from '../doubles/fixtures';
import { threadInput, userMessage } from '../doubles/thread-rows';
import { show } from './support';
import { snapshot } from '../unit/continuation-fixtures';

const calls = vi.hoisted(() => ({ update: vi.fn<typeof ThreadsApi.updateInput>(), remove: vi.fn<typeof ThreadsApi.cancelInput>(), resume: vi.fn<typeof ThreadsApi.resumeQueue>() }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof ThreadsApi>(), updateInput: calls.update, cancelInput: calls.remove, resumeQueue: calls.resume }));
const queueHelp = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => queueHelp }));
const THREAD = 'queue-thread';
const READ = defaultThreadRead(THREAD);
const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
const GRACE = userRef(me(2, 'grace', 'Grace Hopper'));
let sequence = 10;
const image: ContentPart = { type: 'image', blob: 'kept-image', name: 'screenshot.png', media_type: 'image/png' };

function input(id: string, text: string, position: string, more: Partial<ThreadInput> = {}): ThreadInput {
  return { ...threadInput(id, text, { author: ADA, position }), thread_id: THREAD, seq_updated: ++sequence, ...more };
}

function mount(inputs: ThreadInput[], more: Partial<Thread> = {}, readOnly = false) {
  const thread = { id: THREAD, kind: 'thread', name: 'Repair login', seq: 10, state: 'idle', archived: false, side_of: null, lock: null,
    environment: null, binding: null, owner: ADA, blocked: null, queue: { held: false, by: null, at: null, held_inputs: 0, revision: 1 },
    access: { permissions: ['thread.view', 'thread.interact'] }, ...more } as Thread;
  threadWriter.reset(THREAD, { thread, inputs }, false);
  const Harness = () => { const view = useThreadView(THREAD); return <QueueTray threadId={THREAD} queued={queuedInputs(view)} sending={[]} readOnly={readOnly} />; };
  return show(<Harness />);
}

function row(number: number) { return screen.getByRole('listitem', { name: i18n.t('composer.queue.row', { number }) }); }
function button(key: 'editRow' | 'removeRow' | 'moveRowUp' | 'moveRowDown' | 'sendNowLabel' | 'interruptNowLabel', number: number) {
  return screen.getByRole('button', { name: i18n.t(`composer.queue.${key}`, { number }) });
}

beforeEach(() => {
  vi.clearAllMocks();
  calls.update.mockReset(); calls.remove.mockReset(); calls.resume.mockReset();
  sequence = 10;
  client.session.setState({ userId: ADA.id });
  useComposerStore.setState({ cannotSteer: {} });
  threadWriter.evict(THREAD);
  calls.update.mockImplementation((_thread, id, body) => {
    // The response is the server's row, not a speculative local edit.
    const view = heldInputs.get(id);
    if (view === undefined) return Promise.reject(new Error('No such waiting input'));
    const next = { ...view, ...(body.content === undefined ? {} : { content: body.content }), ...(body.delivery === undefined ? {} : { delivery: body.delivery }) };
    let changed = [next];
    if (body.afterInputId !== undefined || body.delivery !== undefined) {
      const others = [...heldInputs.values()].filter((input) => input.id !== id && input.state === 'pending').sort((one, other) => comparePositions(one.position, other.position));
      const predecessor = body.afterInputId;
      const after = predecessor === undefined || predecessor === null ? -1 : others.findIndex((input) => input.id === predecessor);
      if (predecessor !== undefined && predecessor !== null && after < 0) return Promise.reject(new Error('The preceding input is no longer pending'));
      const index = body.delivery === 'queue' ? others.length : after + 1;
      others.splice(index, 0, next);
      changed = others.map((input, at) => ({ ...input, position: String(at + 1) }))
        .filter((input) => input.id === id || input.position !== heldInputs.get(input.id)?.position);
    }
    const stamp = ++sequence;
    changed = changed.map((input) => ({ ...input, seq_updated: stamp }));
    for (const input of changed) heldInputs.set(input.id, input);
    const input = heldInputs.get(id);
    if (input === undefined) return Promise.reject(new Error('The changed input is still present'));
    return Promise.resolve({ input, inputs: changed });
  });
  calls.remove.mockImplementation((_thread, id) => { const value = heldInputs.get(id); if (value === undefined) return Promise.reject(new Error('Missing input')); return Promise.resolve({ ...value, state: 'cancelled', seq_updated: ++sequence }); });
});
const heldInputs = new Map<string, ThreadInput>();
function draw(inputs: ThreadInput[], more: Partial<Thread> = {}, readOnly = false) { heldInputs.clear(); for (const one of inputs) heldInputs.set(one.id, one); return mount(inputs, more, readOnly); }
afterEach(async () => { cleanup(); threadWriter.evict(THREAD); client.session.setState({ userId: null }); useComposerStore.setState({ cannotSteer: {} }); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

describe('the waiting-input queue', () => {
  it('offers Back to queue while a steer has not entered native dispatch', async () => {
    draw([input('steering', 'Take this or let it wait', '2', { state: 'delivering', delivery: 'steer', turn_id: 'running-turn' })], { state: 'running' });
    expect(screen.getByText('Steering…')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Back to queue' }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledWith(READ, 'steering', { delivery: 'queue' }));
  });
  it('refuses recall after dispatch has crossed the take boundary', () => {
    draw([input('taken', 'Already being taken', '2', { state: 'delivering', delivery: 'steer', state_detail: 'steering', turn_id: 'running-turn' })], { state: 'running' });
    expect(screen.getByRole('button', { name: 'Back to queue' })).toBeDisabled();
    expect(calls.update).not.toHaveBeenCalled();
  });
  it.each(['en', 'zh-CN'] as const)('opens one queue guide without changing waiting messages (%s)', async (locale) => {
    await setLocale(locale);
    draw([input('guide', 'Keep this pending input', '1')]);
    const label = i18n.t('taskGuides.open', { title: i18n.t('taskGuides.messageQueue') });
    expect(screen.getAllByRole('button', { name: label })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: label }));
    expect(queueHelp).toHaveBeenCalledExactlyOnceWith(QUEUE_HELP.article, QUEUE_HELP.heading);
    expect(screen.getByText('Keep this pending input')).toBeVisible();
    expect(calls.update).not.toHaveBeenCalled();
    expect(calls.remove).not.toHaveBeenCalled();
    expect(calls.resume).not.toHaveBeenCalled();
  });

  it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps the first held row editable in %s / %s', async (locale, theme) => {
    await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
    const t = i18n.getFixedT(locale);
    const first = input('first', 'First, held by Stop', '1', { queue_held: true });
    draw([first], { state: 'blocked', blocked: { reason: 'unbound', detail: null } });
    expect(screen.getAllByText('First, held by Stop')).toHaveLength(1);
    expect(within(row(1)).getByText(t('composer.queue.heldStop'))).toBeVisible();
    expect(button('editRow', 1)).toBeEnabled();
    expect(button('removeRow', 1)).toBeEnabled();
    await userEvent.click(button('editRow', 1));
    await userEvent.clear(screen.getByRole('textbox', { name: t('composer.queue.editLabel') }));
    await userEvent.type(screen.getByRole('textbox', { name: t('composer.queue.editLabel') }), 'Edited while held');
    await userEvent.click(screen.getByRole('button', { name: t('composer.queue.save') }));
    await waitFor(() => expect(screen.getByText('Edited while held')).toBeVisible());
    expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'first', { content: [{ type: 'text', text: 'Edited while held' }] });
    expect(heldInputs.get('first')?.queue_held).toBe(true);
  });

  it.each(['en', 'zh-CN'] as const)('says why an input waits on its session command and whom it waits for (%s)', async (locale) => {
    await setLocale(locale);
    const t = i18n.getFixedT(locale);
    const native = { action_id: 'native-skill', revision: 'earlier-mapping', arguments: {} };
    const checks = [['stale', 'heldCommandStale'], ['permission', 'heldCommandPermission'], ['arguments', 'heldCommandArguments'], ['unavailable', 'heldCommandUnavailable']] as const;
    draw(checks.map(([check], at) => input(check, `Command ${check}`, String(at + 1), { author: GRACE, state_detail: `native_action.${check}`, native_action: native })));
    checks.forEach(([, words], at) => expect(within(row(at + 1)).getByText(t(`composer.queue.${words}`, { name: GRACE.display_name }))).toBeVisible());
    expect(document.body).not.toHaveTextContent('native_action');
  });

  it('edits only its author’s plain text, retaining the exact nontext parts, native selection and a refused draft', async () => {
    const native = { action_id: 'native-skill', revision: 'current-mapping', arguments: { topic: 'login' } };
    const first = input('files', 'Original text', '1', { content: [image, { type: 'text', text: 'Original text' }], state_detail: 'attachments', native_action: native });
    draw([first, input('other', 'Grace’s message', '2', { author: GRACE })]);
    expect(button('editRow', 1)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Edit queued message 2' })).not.toBeInTheDocument();
    calls.update.mockRejectedValueOnce(new Error('The queue changed; your draft is kept.'));
    await userEvent.click(button('editRow', 1));
    const editor = screen.getByRole('textbox', { name: 'Words of the queued message' });
    await userEvent.clear(editor); await userEvent.type(editor, 'New words');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
    expect(editor).toHaveValue('New words');
    expect(heldInputs.get('files')).toEqual(first);
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    expect(calls.update).toHaveBeenLastCalledWith(READ, 'files', { content: [image, { type: 'text', text: 'New words' }] });
    expect(heldInputs.get('files')?.state_detail).toBe('attachments');
    expect(heldInputs.get('files')?.native_action).toEqual(native);
  });

  it('sends the same idle row first and keeps its Stop hold, without making another input', async () => {
    draw([input('one', 'First', '1'), input('two', 'Second', '2', { queue_held: true })]);
    await userEvent.click(button('sendNowLabel', 2));
    await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'two', { afterInputId: null }));
    await waitFor(() => expect(row(1)).toHaveAttribute('data-queued', 'two'));
    expect(heldInputs.size).toBe(2);
    expect(heldInputs.get('two')?.queue_held).toBe(true);
  });

  it('offers interruption only after a refused steer and waits for that explicit press', async () => {
    const first = input('one', 'Guide the running turn', '1');
    draw([first], { state: 'running' });
    calls.update.mockRejectedValueOnce(new ApiError({ type: 'about:blank', title: 'Unavailable', status: 409, code: 'capability.unsupported', detail: 'This agent cannot be steered.', request_id: 'queue-refusal' }, null));
    await userEvent.click(button('sendNowLabel', 1));
    await waitFor(() => expect(button('interruptNowLabel', 1)).toBeVisible());
    expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'one', { delivery: 'steer', afterInputId: null });
    expect(heldInputs.get('one')).toEqual(first);
    await userEvent.click(button('interruptNowLabel', 1));
    await waitFor(() => expect(calls.update).toHaveBeenLastCalledWith(READ, 'one', { delivery: 'interrupt', afterInputId: null }));
    expect(calls.update).toHaveBeenCalledTimes(2);
  });

  it('changes delivery deliberately and removes the same held row', async () => {
    draw([input('one', 'A held message', '1', { queue_held: true })], { state: 'running' });
    await userEvent.click(screen.getByRole('combobox', { name: 'Delivery of queued message 1' }));
    await userEvent.click(screen.getByRole('option', { name: 'Interrupt' }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'one', { delivery: 'interrupt' }));
    expect(heldInputs.get('one')?.queue_held).toBe(true);
    await userEvent.click(button('removeRow', 1));
    await waitFor(() => expect(screen.queryByRole('region', { name: i18n.t('composer.queue.title', { count: 1 }) })).not.toBeInTheDocument());
    expect(calls.remove).toHaveBeenCalledExactlyOnceWith(READ, 'one');
  });

  it('reorders by arrows and Alt keys without losing keyboard focus', async () => {
    draw([input('one', 'First', '1'), input('two', 'Second', '2'), input('three', 'Third', '3')]);
    await userEvent.click(button('moveRowUp', 2));
    await waitFor(() => expect(row(1)).toHaveAttribute('data-queued', 'two'));
    expect(row(1)).toHaveFocus();
    await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
    await waitFor(() => expect(calls.update).toHaveBeenLastCalledWith(READ, 'two', { afterInputId: 'one' }));
    expect(row(2)).toHaveFocus();
    expect(button('moveRowUp', 1)).toBeDisabled();
    expect(button('moveRowDown', 3)).toBeDisabled();
  });

  it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('shows native integer-position moves from the answer and live frame without reloading in %s / %s', async (locale, theme) => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const content: ContentPart[] = [{ type: 'text', text: 'Second' }, image];
    draw([input('one', 'First', '1'), input('two', 'Second', '2', { content, queue_held: true }), input('three', 'Third', '3')]);
    const queue = screen.getByRole('region', { name: i18n.t('composer.queue.title', { count: 3 }) });
    const order = () => within(queue).getAllByRole('listitem').map((element) => element.getAttribute('data-queued'));
    const firstElement = row(1);
    expect(order()).toEqual(['one', 'two', 'three']);
    await userEvent.click(button('moveRowUp', 2));
    await waitFor(() => expect(order()).toEqual(['two', 'one', 'three']));
    expect(row(2)).toBe(firstElement);
    expect(row(1)).toHaveFocus();
    expect(threadViewOf(THREAD).inputs.get('one')?.position).toBe('2');
    expect(threadViewOf(THREAD).inputs.get('two')?.position).toBe('1');
    const earlier = [...threadViewOf(THREAD).inputs.values()];

    await userEvent.click(button('moveRowDown', 1));
    await waitFor(() => expect(order()).toEqual(['one', 'two', 'three']));
    expect(row(2)).toHaveFocus();
    await userEvent.keyboard('{Alt>}{ArrowUp}{/Alt}');
    await waitFor(() => expect(order()).toEqual(['two', 'one', 'three']));
    expect(row(1)).toHaveFocus();
    await userEvent.keyboard('{Alt>}{ArrowDown}{/Alt}');
    await waitFor(() => expect(order()).toEqual(['one', 'two', 'three']));
    expect(row(2)).toHaveFocus();
    expect(calls.update.mock.calls).toEqual([
      [READ, 'two', { afterInputId: null }], [READ, 'two', { afterInputId: 'one' }],
      [READ, 'two', { afterInputId: null }], [READ, 'two', { afterInputId: 'one' }],
    ]);
    expect(threadViewOf(THREAD).inputs.get('two')?.content).toEqual(content);
    expect(threadViewOf(THREAD).inputs.get('two')?.queue_held).toBe(true);
    expect(threadViewOf(THREAD).inputs.get('two')?.delivery).toBe('queue');

    // Another person's move arrives as one ordinary frame, with the same per-row guards.
    const stamp = ++sequence;
    const live = ['three', 'one', 'two'].map((id, index) => {
      const held = threadViewOf(THREAD).inputs.get(id);
      if (held === undefined) throw new Error('Every waiting input is still present');
      return { ...held, position: String(index + 1), seq_updated: stamp };
    });
    act(() => threadWriter.apply(THREAD, { inputs: live }));
    expect(order()).toEqual(['three', 'one', 'two']);
    // The live frame's echo, or an older HTTP answer arriving late, cannot undo it.
    act(() => threadWriter.apply(THREAD, { inputs: live }));
    act(() => threadWriter.apply(THREAD, { inputs: earlier }));
    expect(order()).toEqual(['three', 'one', 'two']);
    expect(screen.getByRole('region', { name: i18n.t('composer.queue.title', { count: 3 }) })).toBe(queue);
    expect(calls.update).toHaveBeenCalledTimes(4);
    expect(calls.remove).not.toHaveBeenCalled(); expect(calls.resume).not.toHaveBeenCalled();
  });

  it.each(['en', 'zh-CN'] as const)('focuses an edit at the end, keeps real text and saves with Ctrl+Enter in %s', async (locale) => {
    await setLocale(locale);
    const text = '#!fake\nsay Check the redirect';
    draw([input('one', text, '1')]);
    expect(within(row(1)).getByText('say Check the redirect', { exact: true })).toBeVisible();
    expect(within(row(1)).queryByText('#!fake', { exact: true })).not.toBeInTheDocument();
    expect(button('moveRowUp', 1).querySelector('svg.lucide-arrow-up')).not.toBeNull();
    expect(button('moveRowDown', 1).querySelector('svg.lucide-arrow-down')).not.toBeNull();
    const user = userEvent.setup();
    await user.click(button('editRow', 1));
    const editor = screen.getByRole<HTMLTextAreaElement>('textbox', { name: i18n.t('composer.queue.editLabel') });
    expect(editor).toHaveFocus();
    expect(editor.selectionStart).toBe(text.length);
    expect(editor.selectionEnd).toBe(text.length);
    // No click into the editor: the first keystroke must go into the opened box.
    await user.keyboard(' carefully');
    fireEvent.keyDown(editor, { key: 'Enter', ctrlKey: true, isComposing: true });
    expect(calls.update).not.toHaveBeenCalled();
    await user.keyboard('{Control>}{Enter}{/Control}');
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'one', { content: [{ type: 'text', text: `${text} carefully` }] });
    expect(row(1)).toHaveFocus();
  });

  it('owns only a focused row’s Alt arrows, ahead of the real workbench shortcut matcher', async () => {
    draw([input('one', 'First', '1'), input('two', 'Second', '2')]);
    const book = new ShortcutBook(false);
    const previous = vi.fn(); const next = vi.fn(); const unread = vi.fn();
    book.add('previous-thread', KEY_ACTIONS['previous-thread'].shipped, previous);
    book.add('next-thread', KEY_ACTIONS['next-thread'].shipped, next);
    book.add('previous-unread', KEY_ACTIONS['previous-unread'].shipped, unread);
    const navigation = (event: KeyboardEvent) => { book.handle(event); };
    window.addEventListener('keydown', navigation);
    try {
      row(2).focus();
      fireEvent.keyDown(row(2), { key: 'ArrowUp', altKey: true });
      await waitFor(() => expect(row(1)).toHaveAttribute('data-queued', 'two'));
      expect(previous).not.toHaveBeenCalled();
      expect(row(1)).toHaveFocus();
      // The boundary is still the queue; pressing up again cannot open a thread.
      fireEvent.keyDown(row(1), { key: 'ArrowUp', altKey: true });
      expect(previous).not.toHaveBeenCalled();
      expect(calls.update).toHaveBeenCalledTimes(1);
      fireEvent.keyDown(row(1), { key: 'ArrowUp', altKey: true, shiftKey: true });
      expect(unread).toHaveBeenCalledTimes(1);
      fireEvent.keyDown(document.body, { key: 'ArrowUp', altKey: true });
      fireEvent.keyDown(document.body, { key: 'ArrowDown', altKey: true });
      expect(previous).toHaveBeenCalledTimes(1); expect(next).toHaveBeenCalledTimes(1);
      // A focused action belongs to its row, just like its drag handle.
      button('editRow', 1).focus();
      fireEvent.keyDown(button('editRow', 1), { key: 'ArrowDown', altKey: true });
      await waitFor(() => expect(row(2)).toHaveAttribute('data-queued', 'two'));
      expect(row(2)).toHaveFocus();
      expect(previous).toHaveBeenCalledTimes(1);
      const handle = screen.getByRole('button', { name: i18n.t('composer.queue.drag', { number: 2 }) });
      handle.focus();
      fireEvent.keyDown(handle, { key: 'ArrowUp', altKey: true });
      await waitFor(() => expect(row(1)).toHaveAttribute('data-queued', 'two'));
      expect(calls.update).toHaveBeenCalledTimes(3);
      // The editor retains its key; it cannot mutate the queue's ordering.
      await userEvent.click(button('editRow', 1));
      fireEvent.keyDown(screen.getByRole('textbox'), { key: 'ArrowUp', altKey: true });
      expect(previous).toHaveBeenCalledTimes(2);
      expect(calls.update).toHaveBeenCalledTimes(3);
      // A control that has consumed its own event cannot trigger a second workbench act.
      const consumed = new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, cancelable: true });
      consumed.preventDefault();
      expect(book.handle(consumed)).toBe(false);
      expect(next).toHaveBeenCalledTimes(1);
    } finally { window.removeEventListener('keydown', navigation); }
  });

  it.each(['en', 'zh-CN'] as const)('saves with Enter and cancels with Escape, keeping line breaks and composition inside the editor in %s', async (locale) => {
    await setLocale(locale);
    draw([input('one', 'Keep this', '1', { content: [image, { type: 'text', text: 'Keep this' }], queue_held: true })]);
    const user = userEvent.setup();
    await user.click(button('editRow', 1));
    const editor = screen.getByRole('textbox', { name: i18n.t('composer.queue.editLabel') });
    expect(editor).toHaveClass('resize-none');
    await user.keyboard('{Shift>}{Enter}{/Shift}More');
    expect(editor).toHaveValue('Keep this\nMore');
    fireEvent.keyDown(editor, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(editor, { key: 'Enter', altKey: true });
    expect(calls.update).not.toHaveBeenCalled();
    await user.keyboard('{Enter}');
    await waitFor(() => expect(screen.queryByRole('textbox')).not.toBeInTheDocument());
    expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'one', { content: [image, { type: 'text', text: 'Keep this\nMore' }] });
    expect(heldInputs.get('one')?.queue_held).toBe(true);
    expect(row(1)).toHaveFocus();
    await user.click(button('editRow', 1));
    await user.keyboard(' discard this{Escape}');
    expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    expect(row(1)).toHaveFocus();
    expect(calls.update).toHaveBeenCalledTimes(1);
    await user.click(button('editRow', 1));
    expect(screen.getByRole('textbox')).toHaveValue('Keep this\nMore');
  });

  it.each(['pending', 'delivering'] as const)('removes a stale %s steer as soon as its own message arrives, without cancelling or guessing another input’s state', (state) => {
    const steer = input('steer', 'Guide the running turn', '1', { delivery: 'steer', state });
    const other = input('other', 'Still waiting', '2');
    draw([steer, other], { state: 'running' });
    expect(row(1)).toHaveAttribute('data-queued', steer.id);
    const message = userMessage('spoken', 'Guide the running turn', { author: ADA, inputId: steer.id, delivery: 'steer' });
    act(() => threadWriter.apply(THREAD, { items: [{ ...message, thread_id: THREAD, seq_created: ++sequence, seq_updated: sequence }] }));
    expect(screen.queryByText('Guide the running turn')).not.toBeInTheDocument();
    expect(row(1)).toHaveAttribute('data-queued', other.id);
    act(() => threadWriter.apply(THREAD, { inputs: [{ ...steer, seq_updated: ++sequence }] }));
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
    expect(row(1)).toHaveAttribute('data-queued', other.id);
    expect(calls.update).not.toHaveBeenCalled(); expect(calls.remove).not.toHaveBeenCalled();
  });

  it.each(['en', 'zh-CN'] as const)('keeps steer controls until delivery, while only pending messages can still change in %s', async (locale) => {
    await setLocale(locale);
    const steered = input('one', 'Guide the running turn', '1', { delivery: 'steer' });
    draw([steered], { state: 'running' });
    expect(button('editRow', 1)).toBeEnabled();
    expect(button('removeRow', 1)).toBeEnabled();
    expect(button('sendNowLabel', 1)).toBeEnabled();
    await userEvent.click(button('editRow', 1));
    expect(screen.getByRole('textbox')).toHaveFocus();
    await userEvent.keyboard(' carefully{Control>}{Enter}{/Control}');
    await waitFor(() => expect(within(row(1)).getByText('Guide the running turn carefully')).toBeVisible());
    expect(button('removeRow', 1)).toBeEnabled();
    const current = heldInputs.get('one');
    expect(current).toBeDefined();
    act(() => { threadWriter.apply(THREAD, { inputs: [{ ...steered, ...current, state: 'delivering', seq_updated: ++sequence }] }); });
    expect(button('editRow', 1)).toBeDisabled();
    expect(button('removeRow', 1)).toBeDisabled();
    expect(button('sendNowLabel', 1)).toBeDisabled();
    expect(screen.getByRole('combobox', { name: i18n.t('composer.queue.deliveryLabel', { number: 1 }) })).toBeDisabled();
    expect(row(1)).toHaveAccessibleDescription(i18n.t('composer.queue.deliveryStarted'));
    act(() => { threadWriter.apply(THREAD, { inputs: [{ ...steered, ...current, state: 'delivered', seq_updated: ++sequence }] }); });
    expect(screen.queryByRole('region', { name: i18n.t('composer.queue.title', { count: 1 }) })).not.toBeInTheDocument();
    expect(calls.remove).not.toHaveBeenCalled();
  });

  it('takes back a pending steer through the ordinary cancellation before delivery starts', async () => {
    draw([input('steer', 'Wait for me to check this', '1', { delivery: 'steer' })], { state: 'running' });
    await userEvent.click(button('removeRow', 1));
    await waitFor(() => expect(screen.queryByRole('region', { name: i18n.t('composer.queue.title', { count: 1 }) })).not.toBeInTheDocument());
    expect(calls.remove).toHaveBeenCalledExactlyOnceWith(READ, 'steer');
    expect(calls.update).not.toHaveBeenCalled();
  });

  it('ignores native file and other-thread drops rather than moving an unrelated input', () => {
    draw([input('one', 'First', '1'), input('two', 'Second', '2')]);
    const data = new Map<string, string>();
    const transfer = { types: ['application/x-orch-queue'], effectAllowed: '', dropEffect: '', setData: (kind: string, value: string) => data.set(kind, value), getData: (kind: string) => data.get(kind) ?? '' };
    data.set('application/x-orch-queue', 'another-thread/two');
    fireEvent.drop(row(1), { dataTransfer: transfer, clientY: 0 });
    fireEvent.drop(row(1), { dataTransfer: { ...transfer, types: ['Files'] }, clientY: 0 });
    expect(calls.update).not.toHaveBeenCalled();
    expect(row(1)).toHaveAttribute('data-queued', 'one');
    expect(row(2)).toHaveAttribute('data-queued', 'two');
  });

  it.each(['en', 'zh-CN'] as const)('describes the pointer handle and keeps its Alt-arrow ordering available in %s', async (locale) => {
    await setLocale(locale);
    draw([input('one', 'First', '1'), input('two', 'Second', '2')]);
    const handle = screen.getByRole('button', { name: i18n.t('composer.queue.drag', { number: 2 }) });
    expect(handle).toHaveAccessibleDescription(i18n.t('composer.queue.dragInstructions'));
    act(() => handle.focus());
    fireEvent.keyDown(handle, { key: 'ArrowUp', altKey: true });
    await waitFor(() => expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'two', { afterInputId: null }));
    expect(row(1)).toHaveAttribute('data-queued', 'two');
    expect(row(1)).toHaveFocus();
  });

  it('does not turn a delivery picker’s Alt key into an unrelated queue reorder', () => {
    draw([input('one', 'First', '1'), input('two', 'Second', '2')]);
    fireEvent.keyDown(screen.getByRole('combobox', { name: 'Delivery of queued message 2' }), { key: 'ArrowUp', altKey: true });
    expect(calls.update).not.toHaveBeenCalled();
  });

  it('keeps a delivering row visible and gives a viewer no queue mutation controls', () => {
    draw([input('one', 'Already delivering', '1', { state: 'delivering' }), input('two', 'Still held', '2', { queue_held: true })], { access: { permissions: ['thread.view'] } as Thread['access'] });
    expect(screen.getByText('Already delivering')).toBeVisible();
    expect(within(row(1)).getByRole('status')).toHaveTextContent('Sending…');
    for (const number of [1, 2]) {
      const queueRow = within(row(number));
      expect(queueRow.getAllByRole('button')).toEqual([queueRow.getByRole('button', { name: 'Ada Lovelace' })]);
      expect(queueRow.queryByRole('combobox')).not.toBeInTheDocument();
    }
    expect(calls.update).not.toHaveBeenCalled(); expect(calls.remove).not.toHaveBeenCalled();
  });

  it('disables every row during a write and does not speculatively reorder on a refusal', async () => {
    let reject: (error: Error) => void = () => undefined;
    calls.update.mockImplementationOnce(() => new Promise((_resolve, no) => { reject = no; }));
    draw([input('one', 'First', '1'), input('two', 'Second', '2')]);
    await userEvent.click(button('moveRowUp', 2));
    expect(button('removeRow', 1)).toBeDisabled();
    expect(row(1)).toHaveAttribute('data-queued', 'one');
    act(() => { reject(new Error('The input is no longer waiting.')); });
    expect(row(1)).toHaveAttribute('data-queued', 'one');
    await waitFor(() => expect(button('removeRow', 1)).toBeEnabled());
    expect(calls.update).toHaveBeenCalledTimes(1);
  });
});


it('keeps inactive pending words editable/removable without advertising delivery or queue execution', async () => {
  const user = userEvent.setup();
  draw([input('saved', 'Unsent retained words', '1', { queue_held: true }), input('other', 'Another author', '2', { author: GRACE })], {}, true);
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Send now|Move.*earlier|Move.*later|Drag|Resume/i })).not.toBeInTheDocument();
  expect(button('editRow', 1)).toBeEnabled();
  expect(screen.queryByRole('button', { name: i18n.t('composer.queue.editRow', { number: 2 }) })).not.toBeInTheDocument();
  await user.click(button('editRow', 1));
  const editor = screen.getByRole('textbox', { name: i18n.t('composer.queue.editLabel') });
  await user.clear(editor); await user.type(editor, 'Saved without sending');
  await user.click(screen.getByRole('button', { name: i18n.t('composer.queue.save') }));
  expect(calls.update).toHaveBeenCalledExactlyOnceWith(READ, 'saved', { content: [{ type: 'text', text: 'Saved without sending' }] });
  expect(await screen.findByText('Saved without sending')).toBeVisible();
  await user.click(button('removeRow', 2));
  expect(calls.remove).toHaveBeenCalledExactlyOnceWith(READ, 'other');
  expect(calls.resume).not.toHaveBeenCalled();
});

it('does not offer inactive pending mutation to a viewer', () => {
  draw([input('saved', 'Still visible', '1')], { access: { ...snapshot().thread.access, permissions: ['thread.view'] } }, true);
  expect(screen.getByText('Still visible')).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('composer.queue.editRow', { number: 1 }) })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('composer.queue.removeRow', { number: 1 }) })).not.toBeInTheDocument();
  expect(calls.update).not.toHaveBeenCalled(); expect(calls.remove).not.toHaveBeenCalled();
});


it('preserves an open pending editor but cannot save after interaction access is revoked', async () => {
  const user = userEvent.setup(); draw([input('saved', 'Keep these words', '1')], {}, true);
  await user.click(button('editRow', 1));
  const editor = screen.getByRole('textbox', { name: i18n.t('composer.queue.editLabel') });
  await user.clear(editor); await user.type(editor, 'Unsent edited words');
  act(() => threadWriter.patch(READ, thread => ({ ...thread, access: { ...thread.access, permissions: ['thread.view'] } })));
  expect(editor).toHaveValue('Unsent edited words'); expect(editor).toBeDisabled();
  const save = screen.getByRole('button', { name: i18n.t('composer.queue.save') });
  await user.click(save); expect(calls.update).not.toHaveBeenCalled();
});
