/** A fork's historical steps never look live, and moving a single file's acts to its row keeps its real file reader. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Files from '@/features/files/FileTabs';
import type * as ThreadsApi from '@/api/threads';
import type * as NodesApi from '@/api/nodes';
import { ApiError } from '@/api/client';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { useBrowserRequests } from '@/features/browser/store';
import { OrchDisplay } from '@/features/timeline/tools/Orch';
import { AnnouncementCard } from '@/features/timeline/items/Announcement';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { WebFetchDisplay } from '@/features/timeline/tools/WebFetch';
import { AgentsPanel } from '@/features/thread/AgentsPanel';
import { listAgents } from '@/features/thread/agents';
import { timelineUi } from '@/features/timeline/state';
import type { Item, Node, Segment, Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { change, command, fileChange, LOGIN_DIFF, rawOf, toolCall } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn<typeof Files.openFile>(), raw: vi.fn<typeof ThreadsApi.fetchRaw>(), copy: vi.fn(), source: vi.fn<(id: string) => Promise<Node>>() }));
vi.mock('@/features/files/FileTabs', async (load) => ({ ...await load<typeof Files>(), openFile: calls.open }));
vi.mock('@/api/threads', async (load) => ({ ...await load<typeof ThreadsApi>(), fetchRaw: calls.raw }));
vi.mock('@/api/nodes', async (load) => {
  const original = await load<typeof NodesApi>();
  return { ...original, nodeQuery: (...args: Parameters<typeof NodesApi.nodeQuery>) => ({ ...original.nodeQuery(...args),
    ...(args[0] === 'recorded-policy-source' ? { queryFn: () => calls.source(args[0]) } : {}),
  }) };
});
vi.mock('@/ui/clipboard', () => ({ copyText: calls.copy }));
let native: Thread;
let ended: Segment;
let context: TimelineContext;
beforeEach(() => {
  calls.open.mockReset(); calls.raw.mockReset(); calls.copy.mockReset(); calls.source.mockReset();
  const world = new World(), person = world.people.get('ada');
  const row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A native thread is required');
  native = { ...world.threads.thread(person, row), state: 'idle', environment_permissions: ['files.view'] };
  if (native.binding === null || native.environment === null) throw new Error('The historical test needs the original binding facts');
  ended = { id: 'copied-segment', ordinal: 1, agent: native.binding.agent, provider: native.binding.provider, environment: native.environment,
    arrangement: native.binding.arrangement, credential_path: native.binding.isolation.credential_path, executor: null, tool_set: null,
    env_generation: 1, started_by: native.owner, started_at: '2026-10-15T10:00:00Z', ended_at: '2026-10-15T10:01:00Z',
    end_reason: null, import_fidelity: null, import_report: null, seq_updated: 1 };
  context = { threadId: native.id, threadName: native.name, detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
  threadWriter.reset(native.id, { thread: native, segments: [ended] }, false);
});
afterEach(async () => { cleanup(); threadWriter.evict(native.id); timelineUi.forget(native.id); vi.useRealTimers(); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

it.each([
  { tool: 'use_browser', field: 'browser', request: 'front', label: 'browser.tool.open' },
  { tool: 'use_display', field: 'display', request: 'display', label: 'vnc.tool.open' },
] as const)('opens the target named in the recorded $tool result', async ({ tool, field, request, label }) => {
  const item = toolCall('recorded-target', { kind: 'orch', tool, arguments: {}, result: { [field]: 'chosen-target' } }, { title: tool });
  if (item.kind !== 'tool_call') throw new Error('A recorded tool step is required');
  show(<OrchDisplay item={item} />);
  try {
    await userEvent.click(screen.getByRole('button', { name: i18n.t(label) }));
    expect(useBrowserRequests.getState()[request]).toBe('chosen-target');
  } finally {
    useBrowserRequests.getState().takeFront();
    useBrowserRequests.getState().takeDisplay();
  }
});

it.each(['en', 'zh-CN'] as const)('keeps the applied announcement fallback and an older missing decision apart in %s', async (locale) => {
  await setLocale(locale);
  const payload = Object.assign({ announcement_id: 'notice', category: 'notice' as const, to_thread: false, title: 'Keep this notice', body: 'Original words', severity: 'info' as const, from: { user: native.owner } }, {
    delivered_as: 'queued', delivery_resolution: { requested: 'steer' as const, applied: 'queue' as const, reason: 'steer_unavailable' as const, source: { kind: 'request' as const, value: 'next_turn' as const }, resolved_at: '2026-11-15T00:00:00Z' },
  });
  const item: Extract<Item, { kind: 'announcement' }> = { ...toolCall('notice', { kind: 'orch', tool: 'announce', arguments: {}, result: null }, { title: 'Notice' }), kind: 'announcement', payload };
  const draw = (value: typeof item) => <TimelineContextValue.Provider value={context}><AnnouncementCard item={value} /></TimelineContextValue.Provider>;
  const view = show(draw(item));
  expect(screen.getByText(i18n.t('deliveryResolutionFacts.queued'))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const details = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  expect(details).toHaveTextContent(i18n.t('composer.queue.delivery.steer'));
  expect(details).toHaveTextContent(i18n.t('composer.queue.delivery.queue'));
  view.rerender(draw({ ...item, payload: { ...payload, delivery_resolution: undefined } }));
  expect(screen.queryByText(i18n.t('deliveryResolutionFacts.queued'))).not.toBeInTheDocument();
  expect(details).toHaveTextContent(i18n.t('composer.queue.delivery.queue'));
  expect(within(details).getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThanOrEqual(2);
  expect(screen.getByText('Original words')).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('keeps the recorded tool allowance and reported wait after time advances, with absent older facts explicit in %s', async (locale) => {
  await setLocale(locale);
  const item = toolCall('recorded-rate', { kind: 'orch', tool: 'announce', arguments: { title: 'Held' }, result: { error: { code: 'rate_limited' } } }, { status: 'failed', title: 'Recorded refusal' });
  if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'orch') throw new Error('A recorded Orch refusal is required');
  const tool = Object.assign(item.payload.tool, { rate_limit: { allowed_calls: 2, period_secs: 60,
    retry_at: '2026-11-15T00:01:00Z', observed_at: '2026-11-15T00:00:00Z', source: { scope: 'default', node: null } } });
  const view = show(<OrchDisplay item={{ ...item, payload: { ...item.payload, tool } }} />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const dialog = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  const wait = within(dialog).getByText(i18n.t('composer.toolRate.wait')).parentElement;
  const allowance = within(dialog).getByText(i18n.t('composer.toolRate.allowance')).parentElement;
  const recordedWait = wait?.textContent;
  expect(allowance).toHaveTextContent('2');
  const clock = vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2027-01-01T00:00:00Z'));
  try {
    view.rerender(<OrchDisplay item={{ ...item, payload: { ...item.payload, tool } }} />);
    expect(wait?.textContent).toBe(recordedWait);
    view.rerender(<OrchDisplay item={{ ...item, payload: { ...item.payload, tool: { ...tool, rate_limit: undefined } } }} />);
    expect(within(dialog).getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThanOrEqual(3);
  } finally { clock.mockRestore(); }
});

it('reads a recorded setting source only on intent and withholds its name when current access refuses it', async () => {
  calls.source.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', title: 'Not allowed', status: 403, code: 'authz.denied', detail: 'Not allowed', request_id: 'source' }, null));
  const item = toolCall('private-rate-source', { kind: 'orch', tool: 'announce', arguments: {}, result: { error: { code: 'rate_limited' } } }, { status: 'failed', title: 'Recorded refusal' });
  if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'orch') throw new Error('A recorded Orch refusal is required');
  const tool = Object.assign(item.payload.tool, { rate_limit: { allowed_calls: 2, period_secs: 60,
    retry_at: '2026-11-15T00:01:00Z', observed_at: '2026-11-15T00:00:00Z',
    source: { scope: 'topic', node: { id: 'recorded-policy-source', kind: 'topic', name: null, redacted: true } } } });
  show(<OrchDisplay item={{ ...item, payload: { ...item.payload, tool } }} />);
  expect(calls.source).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const dialog = await screen.findByRole('dialog', { name: i18n.t('common.details') });
  await within(dialog).findByText(i18n.t('settings.compact.hiddenSource'));
  expect(calls.source).toHaveBeenCalledExactlyOnceWith('recorded-policy-source');
  expect(dialog).not.toHaveTextContent('recorded-policy-source');
});

it.each(['en', 'zh-CN'] as const)('keeps a masked native address readable with its captured page and no misleading open link in %s', async (locale) => {
  await setLocale(locale);
  const item = toolCall('masked-page', {kind:'web_fetch',url:'https://example.invalid/?key=********************',title:'Fetched page',status:200,content:{inline:'Captured safe page',total_len:18,blob:null,truncated:false}}, {title:'Fetched page'});
  if (item.kind !== 'tool_call') throw new Error('The captured page is a tool step');
  item.payload.masked_fields = ['/tool/url'];
  show(<WebFetchDisplay item={item} />);
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(screen.getByText('Captured safe page')).toBeVisible();
  expect(screen.getByText(i18n.t('timeline.masking.address'))).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('keeps a masked edited path and its diff without opening a different file in %s', async (locale) => {
  await setLocale(locale);
  const item = {...fileChange('hidden-edit', {changes:[change('src/********************.rs', LOGIN_DIFF)]}),thread_id:native.id,segment_id:ended.id,seq_created:2,seq_updated:2};
  if (item.kind !== 'tool_call') throw new Error('The captured edit is a tool step');
  item.payload.masked_fields = ['/tool/changes/0/path'];
  act(() => threadWriter.apply(native.id, {items:[item]}));
  show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  await userEvent.click(screen.getByRole('button', {expanded:false,name:/src\//}));
  const open = screen.getByRole('button', {name:i18n.t('files.link.open',{path:'src/********************.rs'})});
  expect(open).toHaveAttribute('aria-disabled','true');
  await userEvent.click(open);
  expect(calls.open).not.toHaveBeenCalled();
  expect(screen.getByRole('table', {name:/src\//})).toHaveTextContent('return redirect(next ?? "/");');
});

it.each(['input', 'output'] as const)('shows a failed unopened raw %s copy and retries that same act locally', async (part) => {
  const row = command('copy-native', { command: 'echo complete', output: 'complete', exit: 0 });
  if (row.kind !== 'tool_call') throw new Error('The native copy needs a tool call');
  const body = rawOf(row);
  const item = { ...row, thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2, payload: { ...row.payload, raw: undefined } };
  calls.raw.mockRejectedValueOnce(new Error('The raw copy read failed')).mockResolvedValue(body);
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('timeline.actions.more') }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t(part === 'input' ? 'timeline.step.copyRawInput' : 'timeline.step.copyRawOutput') }));
  await waitFor(() => expect(calls.raw).toHaveBeenCalledOnce());
  expect(calls.copy).not.toHaveBeenCalled();
  const failure = await screen.findByRole('alert');
  await userEvent.click(within(failure).getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.raw).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(calls.copy).toHaveBeenCalledWith(part === 'input' ? body.input.inline : body.output?.inline ?? ''));
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(document.querySelector('[data-tool-row] [aria-expanded="true"]')).not.toBeInTheDocument();
});

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

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps an ended session’s reported running step factual, without a live clock or an active agent, in %s/%s', async (locale, look) => {
  await setLocale(locale); document.documentElement.setAttribute('data-theme', look); vi.useFakeTimers();
  const item = { ...toolCall('old-helper', { kind: 'subagent', child_stream: 'sub:old-helper', name: 'reviewer', role: 'review', prompt: null, state: 'running' },
    { title: 'Review', status: 'in_progress' }), thread_id: native.id, segment_id: ended.id, seq_created: 2, seq_updated: 2 };
  act(() => threadWriter.apply(native.id, { items: [item] }));
  show(<TimelineContextValue.Provider value={context}><AgentsPanel entries={listAgents(threadViewOf(native.id), [], i18n.t)} current={null} onMain={vi.fn()} onOpen={vi.fn()} onEnd={null} /><ToolCallRow threadId={native.id} id={item.id} /></TimelineContextValue.Provider>);
  const row = document.querySelector('[data-tool-row]');
  if (!(row instanceof HTMLElement)) throw new Error('The historical step has its own tool row');
  expect(within(row).getByRole('button', { expanded: false })).toHaveTextContent(i18n.t('timeline.step.recordedRunning'));
  expect(screen.queryByRole('region', { name: i18n.t('thread.agents.active') })).not.toBeInTheDocument();
  expect(within(screen.getByRole('region', { name: i18n.t('thread.agents.done') })).getByText(i18n.t('timeline.step.recordedRunning'))).toBeVisible();
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
  const step = commandRow.closest('[data-step]');
  if (!(step instanceof HTMLElement)) throw new Error('The command disclosure owns its output');
  await userEvent.click(within(commandRow).getByRole('button', { expanded: false }));
  expect(within(step).queryByRole('button', { name: /Show all|显示全部/ })).not.toBeInTheDocument();
  expect(await within(step).findByText(i18n.t('timeline.command.outputMissing', { from: '0', to: '999996' }))).toBeVisible();
});

it.each(['en', 'zh-CN'] as const)('says whether a thread message requested wake, including an omitted request (%s)', async (locale) => {
  await setLocale(locale);
  const row = toolCall('wake-intent', { kind: 'orch', tool: 'send_message', arguments: { target: { kind: 'thread', path: '/Review' }, text: 'Check this' }, result: null }, { title: 'Send a message' });
  if (row.kind !== 'tool_call' || row.payload.tool.kind !== 'orch') throw new Error('The native call supplies its own wake argument');
  const view = show(<OrchDisplay item={row} />);
  expect(screen.getByText(locale === 'en' ? 'No wake requested' : '未请求唤醒', { exact: false })).toBeVisible();
  view.rerender(<OrchDisplay item={{ ...row, payload: { ...row.payload, tool: { ...row.payload.tool, arguments: { target: { kind: 'thread', path: '/Review' }, text: 'Check this', wake_idle: true } } } }} />);
  expect(screen.getByText(locale === 'en' ? 'Wake requested' : '已请求唤醒', { exact: false })).toBeVisible();
  expect(screen.queryByText(locale === 'en' ? 'No wake requested' : '未请求唤醒', { exact: false })).not.toBeInTheDocument();
});
