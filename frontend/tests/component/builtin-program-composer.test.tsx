/** Permanent model choices keep the native thread identity, reader and draft. */
import { act, cleanup, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AskApi from '@/api/ask';
import { client } from '@/api/client';
import type { AskComposerProps } from '@/features/composer/AskComposer';
import { ProgramComposer } from '@/features/composer/ProgramComposer';
import type { Thread } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ send: vi.fn<typeof AskApi.sendAsk>(), sync: vi.fn(), stop: vi.fn(), props: null as AskComposerProps | null }));
vi.mock('@/api/ask', async (load) => ({ ...await load<typeof AskApi>(), sendAsk: calls.send }));
vi.mock('@/api/threads', async (load) => ({ ...await load<object>(), fetchThreadSync: calls.sync, interruptTurn: calls.stop }));
vi.mock('@/api/threadRead', async (load) => ({ ...await load<object>(), captureThreadExecution: vi.fn(), threadAct: (_ref: unknown, execute: () => Promise<unknown>) => execute() }));
vi.mock('@/realtime/threadReadContext', () => ({ useThreadReadRef: (id: string) => id }));
vi.mock('@/realtime/stores/thread', async (load) => ({ ...await load<object>(), useThreadPart: () => null }));
vi.mock('@/features/composer/AskComposer', () => ({ AskComposer: (props: AskComposerProps) => { calls.props = props; return <div data-question-thread={props.conversationId} />; } }));

let thread: Thread;
function question(): AskComposerProps { if (calls.props === null) throw new Error('The genuine program composer must be mounted'); return calls.props; }
beforeEach(() => {
  vi.clearAllMocks(); calls.props = null; calls.sync.mockResolvedValue(undefined);
  calls.send.mockResolvedValue({ conversation_id: ACME.mainThread, turn_id: null, seq: 101 });
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A genuine thread is required');
  thread = { ...world.threads.thread(person, row), binding: null, environment: null, lock: null, state: 'idle', current_turn_id: null,
    program: { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false },
    program_model: askModels().default_model };
  client.session.setState({ status: 'signed_in', userId: person.me.id, readerVersion: 0 });
});
afterEach(async () => { cleanup(); act(() => client.session.setState({ status: 'signed_out', userId: null })); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`uses the saved choice, then an explicit choice and reset for the actual thread (${locale})`, async () => {
  // EN: kept thread > choose an API model > Send > current default > Send.
  // 中文：保留的会话 > 选择 API 模型 > 发送 > 当前默认模型 > 发送。
  await setLocale(locale); show(<ProgramComposer thread={thread} />);
  expect(question()).toMatchObject({ root: ACME.org, conversationId: thread.id, draftKey: thread.id, model: thread.program_model });
  await act(() => question().send([{ type: 'text', text: 'Keep the saved choice' }], 'saved'));
  expect(calls.send.mock.calls[0]).toEqual([thread.id, { client_id: 'saved', content: [{ type: 'text', text: 'Keep the saved choice' }], context: null, model: thread.program_model }, true]);
  const selected = { provider_id: thread.program_model?.provider_id ?? '', model: 'other-api-model' };
  act(() => question().onModel(selected));
  await act(() => question().send([{ type: 'text', text: 'Use this choice' }], 'explicit'));
  expect(calls.send.mock.calls[1]?.[1].model).toEqual(selected);
  expect(calls.send.mock.calls[1]?.[2]).toBe(false);
  act(() => question().onModel(null));
  await act(() => question().send([{ type: 'text', text: 'Follow the current default' }], 'reset'));
  expect(calls.send.mock.calls[2]?.[1].model).toBeNull();
  expect(calls.send.mock.calls[2]?.[2]).toBe(false);
  expect(calls.sync).toHaveBeenCalledTimes(3);
});

it('keeps a viewer read-only and stops through the native interrupt operation', async () => {
  const view = show(<ProgramComposer thread={{ ...thread, access: { ...thread.access, permissions: ['thread.view'] } }} />);
  expect(question().disabled).toBe(true);
  view.rerender(<ProgramComposer thread={{ ...thread, state: 'running', current_turn_id: ACME.repro }} />);
  expect(question().active).toBe(true); expect(question().canStop).toBe(true);
  await act(() => question().stop());
  expect(calls.stop).toHaveBeenCalledExactlyOnceWith(thread.id, ACME.repro);
  expect(calls.send).not.toHaveBeenCalled();
});

it('does not refresh or clear a replacement readers draft after the question returns', async () => {
  let resolve: (() => void) | undefined;
  calls.send.mockImplementationOnce(() => new Promise((done) => { resolve = () => done({ conversation_id: thread.id, turn_id: null, seq: 101 }); }));
  const onSent = vi.fn(); show(<ProgramComposer thread={thread} onSent={onSent} />);
  const pending = question().send([{ type: 'text', text: 'Original reader question' }], 'original');
  const refused = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  await waitFor(() => expect(resolve).toBeTypeOf('function'));
  act(() => client.session.setState({ userId: 'replacement-reader', readerVersion: 1 }));
  resolve?.(); await refused;
  expect(calls.sync).not.toHaveBeenCalled(); expect(onSent).not.toHaveBeenCalled();
});
