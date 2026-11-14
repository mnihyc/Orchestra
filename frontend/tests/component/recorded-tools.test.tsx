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
import { change, command, fileChange, LOGIN_DIFF, toolCall } from '../doubles/thread-rows';
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

// EN: Background command → hover/focus its disclosure → read the complete command, including the prompt mark.
// 中文：后台命令 → 悬停或聚焦展开控件 → 读取完整命令及提示符。
function showClippedCommand(words: string) {
  const item = { ...command('long-background', { command: words, output: '', exit: 0 }), thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2 };
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  const control = screen.getByRole('button', { expanded: false, name: new RegExp(words) });
  const label = control.querySelector('[data-overflow-text]');
  if (!(label instanceof HTMLElement)) throw new Error('A command has its full value on its visible label');
  label.style.cssText = 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap';
  Object.defineProperties(label, {
    clientWidth: { configurable: true, value: 160 }, scrollWidth: { configurable: true, value: 480 },
    clientHeight: { configurable: true, value: 20 }, scrollHeight: { configurable: true, value: 20 },
  });
  return control;
}

it.each(['en', 'zh-CN'] as const)('reveals a clipped command through the full-value tooltip (%s)', async (locale) => {
  await setLocale(locale);
  const words = 'Build web and check the authentication routes';
  const control = showClippedCommand(words);
  await userEvent.hover(control);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(`$${words}`);
  expect(control).toHaveAttribute('aria-expanded', 'false');
});

it.each(['en', 'zh-CN'] as const)('keeps the whole command visible on keyboard focus after the pointer leaves (%s)', async (locale) => {
  await setLocale(locale);
  const words = 'Build web and check the authentication routes';
  const control = showClippedCommand(words);
  const user = userEvent.setup();
  await user.hover(control);
  expect(await screen.findByRole('tooltip')).toHaveTextContent(`$${words}`);
  act(() => control.focus());
  await user.unhover(control);
  expect(control).toHaveFocus();
  expect(await screen.findByRole('tooltip')).toHaveTextContent(`$${words}`);
  expect(control).toHaveAttribute('aria-expanded', 'false');
});

it.each(['en', 'zh-CN'] as const)('never offers Show all when only the inline command tail survived (%s)', async (locale) => {
  await setLocale(locale);
  const row = command('lost-output', { command: 'long-running-build', output: 'tail', exit: 0 });
  if (row.kind !== 'tool_call' || row.payload.tool.kind !== 'command') throw new Error('A command fixture is required');
  const item = { ...row, thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2,
    payload: { ...row.payload, tool: { ...row.payload.tool, output: { inline: 'tail', total_len: 1_000_000, truncated: true, blob: null } } } };
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  const commandRow = document.querySelector('[data-tool-row]');
  if (!(commandRow instanceof HTMLElement)) throw new Error('A retained command row is required');
  await userEvent.click(within(commandRow).getByRole('button', { expanded: false }));
  expect(within(commandRow).queryByRole('button', { name: /Show all|显示全部/ })).not.toBeInTheDocument();
  expect(commandRow.textContent).toContain(i18n.t('timeline.command.outputMissing', { from: '0', to: '999996' }));
});
