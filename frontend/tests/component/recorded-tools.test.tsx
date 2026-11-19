/** A fork's historical steps never look live, and moving a single file's acts to its row keeps its real file reader. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Files from '@/features/files/FileTabs';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { SubagentChips } from '@/features/timeline/SubagentChips';
import { timelineUi } from '@/features/timeline/state';
import type { Segment, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { change, fileChange, LOGIN_DIFF, toolCall } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn<typeof Files.openFile>() }));
vi.mock('@/features/files/FileTabs', async (load) => ({ ...await load<typeof Files>(), openFile: calls.open }));
let native: Thread;
let ended: Segment;
let context: TimelineContext;
beforeEach(() => {
  calls.open.mockReset();
  const world = new World(), person = world.people.get('ada');
  const row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, row), state: 'idle', environment_permissions: ['files.view'] };
  if (native.binding === null || native.environment === null) throw new Error('The historical test needs the original binding facts');
  ended = { id: 'copied-segment', ordinal: 1, agent: native.binding.agent, provider: native.binding.provider, environment: native.environment,
    arrangement: native.binding.arrangement, credential_path: native.binding.isolation.credential_path, executor: null, tool_set: null,
    env_generation: 1, started_by: native.owner, started_at: '2026-10-15T10:00:00Z', ended_at: '2026-10-15T10:01:00Z',
    end_reason: null, import_fidelity: null, import_report: null, seq_updated: 1 };
  context = { threadId: native.id, threadName: native.name, detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
  threadWriter.reset(native.id, { thread: native, segments: [ended] }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(native.id); timelineUi.forget(native.id); vi.useRealTimers(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

it.each(['en', 'zh-CN'] as const)('names a message step once for either target in %s', async (locale) => {
  await setLocale(locale);
  const name = i18n.t('choiceCatalog.sharedTool.send_message.name');
  for (const kind of ['chat', 'thread'] as const) {
    const item = { ...toolCall(`message-${kind}`, { kind: 'orch', tool: 'send_message',
      arguments: { target: { kind, path: kind === 'chat' ? '/Team/chat.md' : '/Team/Review/' }, text: 'The report.' },
      result: { status: kind === 'chat' ? 'posted' : 'queued' } }, { title: 'Send a message' }),
      thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2 };
    act(() => threadWriter.apply(native.id, { items: [item] }));
    const view = show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
    expect(screen.getAllByText(name)).toHaveLength(1);
    expect(screen.getByRole('button', { expanded: false, name: new RegExp(name) })).toBeInTheDocument();
    expect(screen.queryByText('Send a message')).not.toBeInTheDocument();
    view.unmount();
  }
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps an ended session’s reported running step factual, without a live clock or header chip, in %s/%s', async (locale, look) => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', look); vi.useFakeTimers();
  const item = { ...toolCall('old-helper', { kind: 'subagent', child_stream: 'sub:old-helper', name: 'reviewer', role: 'review', prompt: null, state: 'running' },
    { title: 'Review', status: 'in_progress' }), thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2 };
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><SubagentChips threadId={native.id} /><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  const row = document.querySelector('[data-tool-row]');
  if (!(row instanceof HTMLElement)) throw new Error('The historical step has its own tool row');
  expect(within(row).getByRole('button', { expanded: false })).toHaveTextContent(i18n.t('timeline.step.recordedRunning'));
  expect(screen.queryByRole('group', { name: i18n.t('timeline.subagent.running') })).not.toBeInTheDocument();
  expect(document.querySelector('.animate-spin')).toBeNull(); expect(document.querySelector('[data-tool-duration]')).toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
  expect(document.querySelector('[data-tool-duration]')).toBeNull();
  expect(threadViewOf(native.id).items.get(item.id)?.status).toBe('in_progress');
});

it.each(['en', 'zh-CN'] as const)('names a single edited file once and opens that real file from the same row in %s', async (locale) => {
  await setLocale(locale);
  const item = { ...fileChange('single-edit', { changes: [change('src/session.rs', LOGIN_DIFF)] }), thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2 };
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  const row = screen.getByRole('button', { expanded: false, name: /src\/session.rs/ }); await userEvent.click(row);
  expect(screen.getAllByText(/src\/session.rs/)).toHaveLength(1);
  const open = screen.getByRole('button', { name: i18n.t('files.link.open', { path: 'src/session.rs' }) });
  expect(open.closest('[data-tool-row]')).toContainElement(row); expect(open.textContent).toBe('');
  await userEvent.click(open);
  expect(calls.open).toHaveBeenCalledExactlyOnceWith({ environmentId: native.environment!.id, threadId: native.id, path: 'src/session.rs', line: null, permanent: false });
  expect(row).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('table', { name: /src\/session.rs/ })).toHaveTextContent('return redirect(next ?? "/");');
});
