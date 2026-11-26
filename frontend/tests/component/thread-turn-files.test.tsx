/** A turn's admitted changed files remain one lazy card, including native snapshots. */
import { act, cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnFiles, turnChanges } from '@/features/timeline/TurnFiles';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { RecordingNotice } from '@/features/source/RecordingNotice';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { captureThreadRead } from '@/api/threadRead';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { i18n, setLocale } from '@/i18n';
import { AT, change, command, fileChange, marker, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { show, skipOverflowFallback } from './support';
import { A, snapshot } from '../unit/continuation-fixtures';

const wire = vi.hoisted(() => ({ open: vi.fn<(path: string) => void>(), mount: vi.fn<(path: string) => void>(), review: vi.fn<(thread: string, turn: string) => void>() }));
skipOverflowFallback();
vi.mock('@/features/inspector/reviewTarget', async (loadOriginal) => ({ ...await loadOriginal<object>(), openTurnReview: wire.review }));
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => <div className={props.className}><ol aria-label={props.label}>
  {Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}
</ol></div> }));
vi.mock('@/features/files/FileLink', async (load) => ({ ...await load<object>(), FileLink: ({ path, children }: { path: string; children: ReactNode }) => { wire.mount(path); return <button onClick={() => wire.open(path)}>{children}</button>; } }));
vi.mock('@/features/source/Restore', async (load) => ({ ...await load<object>(), RestoreFromTurn: ({ turn }: { turn: { id: string } }) => <button data-restore-turn={turn.id}>Restore this turn</button> }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });
const first = fileChange('first-edit', { turn: 'one', changes: [change('src/a.ts', '@@ -1 +1 @@\n-old\n+new'), change('gone.txt', '-removed', { change: 'delete' })] });
const second = fileChange('second-edit', { turn: 'one', changes: [change('src/a.ts', '@@ -1 +1 @@\n-new\n+final')] });
const other = fileChange('other-turn', { turn: 'two', changes: [change('private-other.ts', '+unrelated')] });
const items = new Map([first, second, other].map((item) => [item.id, item]));
const changes = turnChanges(items, ['first-edit', 'second-edit', 'other-turn'], 'one');

it('shows no file changes for a long run of commands without ordering their output by timestamp', () => {
  const commands = Array.from({ length: 42 }, (_, index) => command(`step-${index}`, { turn: 'one', command: 'pwd' }));
  const history = new Map(commands.map((item) => [item.id, item]));
  const parse = vi.spyOn(Date, 'parse');
  try {
    expect(turnChanges(history, [...history.keys()], 'one').files).toEqual([]);
    expect(parse).not.toHaveBeenCalled();
  } finally { parse.mockRestore(); }
});

it('counts distinct paths and keeps the last native item for each without losing edit totals', () => {
  expect(changes).toMatchObject({ added: 2, removed: 3 });
  expect(changes.files.map((file) => [file.change.path, file.itemId])).toEqual([['src/a.ts', 'second-edit'], ['gone.txt', 'first-edit']]);
});

function snapshotFile(id: string, files: ReturnType<typeof change>[]) {
  const item = marker(id, { marker: 'harness_changed', data: {
    program: 'Codex', change: 'turn_diff', subject: null, previous: { state: 'unsupported' }, expected: { state: 'unsupported' },
    value: { changes: files }, changed: true, diverged: false, next_turn: false, turn_id: 'one', reported_at: '2026-11-14T10:00:00Z',
  } }, { turn: 'one' });
  if (item.kind !== 'marker') throw new Error('A snapshot is a native observation');
  return item;
}

it('replaces and clears a turns file list and figures from its authoritative snapshot', () => {
  const snap = snapshotFile('native-diff', [change('aggregate.ts', '@@ -1 +1,2 @@\n-old\n+new\n+next')]);
  const held = new Map([...items, [snap.id, snap]]);
  const order = ['first-edit', 'second-edit', snap.id, 'other-turn'];
  expect(turnChanges(held, order, 'one')).toMatchObject({ added: 2, removed: 1,
    files: [{ itemId: snap.id, change: { path: 'aggregate.ts' } }] });
  held.set(snap.id, snapshotFile(snap.id, [change('replacement.ts', '@@ -1 +1 @@\n-old\n+final')]));
  expect(turnChanges(held, order, 'one')).toMatchObject({ added: 1, removed: 1,
    files: [{ itemId: snap.id, change: { path: 'replacement.ts' } }] });
  held.set(snap.id, snapshotFile(snap.id, []));
  expect(turnChanges(held, order, 'one')).toEqual({ files: [], added: 0, removed: 0, partial: true, coverage: 'tools', reported: 2 });
  expect(first.kind).toBe('tool_call');
});

it('applies a child streams snapshot without clearing another streams original edits', () => {
  const child = fileChange('child-edit', { turn: 'one', stream: 'child', changes: [change('child-old.ts', '+old')] });
  const snap = { ...snapshotFile('child-diff', [change('child-net.ts', '+new')]), stream: 'child' };
  const held = new Map([...items, [child.id, child], [snap.id, snap]]);
  const result = turnChanges(held, ['first-edit', 'second-edit', 'other-turn'], 'one');
  expect(result.files.map(({ change }) => change.path)).toEqual(['src/a.ts', 'gone.txt', 'child-net.ts']);
  held.set(snap.id, { ...snapshotFile(snap.id, []), stream: 'child' });
  expect(turnChanges(held, ['first-edit', 'second-edit'], 'one')).toEqual({ ...changes, reported: 3 });
});

it('keeps observed shell paths alongside the native snapshot without counting superseded tool paths', () => {
  const snap = snapshotFile('observed-diff', [change('aggregate.ts', '+new')]);
  const held = new Map([...items, [snap.id, snap]]);
  const observed = turnChanges(held, [first.id, second.id, snap.id], 'one', { observed: ['aggregate.ts', 'shell.txt'] });
  expect(observed).toMatchObject({ coverage: 'observed', partial: true, added: 1, removed: 0, reported: 3 });
  expect(observed.files.map(({ change, itemId }) => [change.path, itemId])).toEqual([
    ['aggregate.ts', snap.id], ['shell.txt', null],
  ]);
});

it('leaves Orch\u2019s own folder out of what the workspace shows changed, unless a tool of the turn changed it', () => {
  const notes = fileChange('notes-edit', { turn: 'one', changes: [change('NOTES.md', '+one\n+two\n+three', { change: 'add' })] });
  const memory = fileChange('memory-edit', { turn: 'one', changes: [change('.orch/memory/MEMORY.md', '+- remember')] });
  const seen = turnChanges(new Map([[notes.id, notes]]), [notes.id], 'one', { observed: ['.orch', '.orch/memory', 'NOTES.md'] });
  expect(seen.files.map(({ change }) => change.path)).toEqual(['NOTES.md']);
  expect(seen).toMatchObject({ coverage: 'observed', partial: false });
  const written = turnChanges(new Map([[notes.id, notes], [memory.id, memory]]), [notes.id, memory.id], 'one', { observed: ['.orch/memory/MEMORY.md', 'NOTES.md'] });
  expect(written.files.map(({ change }) => change.path)).toEqual(['.orch/memory/MEMORY.md', 'NOTES.md']);
});

it('keeps review of reported edits when the observed end and native snapshot are unchanged', async () => {
  const source = snapshot(), threadId = source.thread.id, read = captureThreadRead(threadId);
  const edit = { ...fileChange('reverted-edit', { turn: 'one', changes: [change('reverted.ts', '+temporary')] }),
    thread_id: threadId, continuation_id: A, seq_created: 1, seq_updated: 1 };
  const snap = { ...snapshotFile('reverted-diff', []), thread_id: threadId, continuation_id: A, seq_created: 2, seq_updated: 2 };
  const done = { ...turn('one', { ordinal: 1, state: 'completed', inputId: 'input' }), files_changed_paths: [] };
  const opened = threadWriter.reset(threadId, { ...source, items: [edit, snap], turns: [done] }, false, read);
  if (opened === null) throw new Error('The observed end is held in its original history');
  const context: TimelineContext = { threadId, threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={done.id} /></TimelineContextValue.Provider>);
  try {
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('thread.turnFiles.reviewReported') }));
    expect(wire.review).toHaveBeenCalledExactlyOnceWith(threadId, done.id);
    expect(view.container.querySelector('[data-turn-files]')).toBeNull();
  } finally { view.unmount(); threadWriter.evict(threadId); }
});

for (const locale of ['en', 'zh-CN'] as const) it(`finds a path in a long aggregate file list without opening another workspace in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  const snap = snapshotFile('many-files', Array.from({ length: 12 }, (_, index) => change(`src/path-${index}.ts`, '+new')));
  const reported = turnChanges(new Map([[snap.id, snap]]), [snap.id], 'one');
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const draw = (held: typeof reported) => <TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={held} /></TimelineContextValue.Provider>;
  const view = show(draw(reported));
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(wire.mount).not.toHaveBeenCalled();
  const disclosure = view.container.querySelector<HTMLElement>('[data-turn-changes="one"]');
  if (disclosure === null) throw new Error('The long list begins collapsed');
  await userEvent.click(disclosure);
  await user.type(await screen.findByRole('searchbox', { name: i18n.t('thread.turnFiles.find') }), 'path-11');
  expect(screen.getByText('src/path-11.ts')).toBeVisible();
  expect(screen.queryByText('src/path-0.ts')).not.toBeInTheDocument();
  const scroll = view.container.querySelector('[data-turn-files] .overflow-y-auto');
  expect(scroll).not.toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('inspector.changes.openFile') }));
  expect(wire.open).toHaveBeenCalledExactlyOnceWith('src/path-11.ts');
  view.rerender(draw({ ...reported, files: reported.files.slice(0, 2), added: 2, removed: 0 }));
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(screen.getByText('src/path-0.ts')).toBeVisible();
  view.rerender(draw({ ...reported, files: [], added: 0, removed: 0 }));
  wire.mount.mockClear();
  view.rerender(draw(reported));
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(wire.mount).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it(`offers Review for an aggregate-only turn and removes it when the snapshot is empty in ${locale}`, async () => {
  await setLocale(locale);
  const source = snapshot(), threadId = source.thread.id, read = captureThreadRead(threadId);
  const snap = { ...snapshotFile('footer-diff', [change('aggregate-footer.ts', '@@ -1 +1,2 @@\n-old\n+new\n+next')]),
    thread_id: threadId, continuation_id: A, seq_created: 1, seq_updated: 1 };
  const done = turn('one', { ordinal: 1, state: 'completed', inputId: 'input' });
  const opened = threadWriter.reset(threadId, { ...source, items: [snap], turns: [done] }, false, read);
  if (opened === null) throw new Error('The captured thread opens in its current history');
  const context: TimelineContext = { threadId, threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={done.id} /></TimelineContextValue.Provider>);
  const footer = view.container.querySelector('[data-turn-changes="one"]');
  expect(footer).toBeVisible();
  expect(footer).toHaveTextContent(i18n.t('thread.turnFiles.reported', { count: 1 }));
  expect(footer).toHaveTextContent(i18n.t('thread.turnFiles.toolLines', { added: 2, removed: 1 }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.turnFiles.review') }));
  expect(wire.review).toHaveBeenCalledExactlyOnceWith(threadId, done.id);
  act(() => { threadWriter.apply(opened.ref, { items: [{ ...snap, payload: snapshotFile(snap.id, []).payload, seq_updated: 2 }] }, opened); });
  expect(screen.queryByRole('button', { name: i18n.t('thread.turnFiles.review') })).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-turn-files]')).toBeNull();
  view.unmount(); threadWriter.evict(threadId);
});

for (const locale of ['en', 'zh-CN'] as const) it(`includes aggregate-only paths in the ordinary turn file disclosure in ${locale}`, async () => {
  await setLocale(locale);
  const snap = snapshotFile('only-diff', [change('only-in-snapshot.ts', '@@ -1 +1 @@\n-old\n+new')]);
  const reported = turnChanges(new Map([[snap.id, snap]]), [snap.id], 'one');
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={reported} /></TimelineContextValue.Provider>);
  const disclosure = view.container.querySelector<HTMLElement>('[data-turn-changes="one"]');
  if (disclosure === null) throw new Error('The reported file needs its ordinary disclosure');
  await userEvent.click(disclosure);
  expect(await screen.findByText('only-in-snapshot.ts')).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) for (const detail of ['normal', 'verbose', 'diagnostics'] as const) it(`draws one card with the counts, lazy paths with exact Open, and Review in ${locale}/${detail}`, async () => {
  await setLocale(locale);
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail, layout: 'one', setLayout: () => undefined, meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={changes} /></TimelineContextValue.Provider>);
  const card = view.container.querySelector<HTMLElement>('[data-turn-files="one"]');
  if (card === null) throw new Error('A turn that changed files has its card');
  expect(card).toHaveTextContent(i18n.t('thread.turnFiles.reported', { count: 2 }));
  expect(card).toHaveTextContent(i18n.t('thread.turnFiles.toolLines', { added: 2, removed: 3 }));
  expect(view.container.querySelectorAll('[data-turn-files]')).toHaveLength(1);
  expect(wire.mount).not.toHaveBeenCalled();
  expect(screen.queryByText('src/a.ts')).not.toBeInTheDocument();
  const toggle = view.container.querySelector<HTMLElement>('[data-turn-changes="one"]');
  if (toggle === null) throw new Error('The card opens its list');
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
  await userEvent.click(toggle);
  await screen.findByText('src/a.ts');
  expect(toggle).toHaveAttribute('aria-expanded', 'true');
  expect(screen.queryByText('private-other.ts')).not.toBeInTheDocument();
  await userEvent.click(within(card).getByRole('button', { name: i18n.t('inspector.changes.openFile') }));
  expect(wire.open).toHaveBeenCalledExactlyOnceWith('src/a.ts');
  await userEvent.click(within(card).getByRole('button', { name: i18n.t('thread.turnFiles.review') }));
  expect(wire.review).toHaveBeenCalledExactlyOnceWith('thread', 'one');
  expect(screen.queryByRole('button', { name: 'Restore this turn' })).not.toBeInTheDocument();
});

it('draws no card for a turn that changed nothing, and no Review where the conversation is read from a record', () => {
  const none = show(<TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={{ files: [], added: 0, removed: 0, partial: false, coverage: 'observed', reported: 0 }} />);
  expect(none.container.querySelector('[data-turn-files]')).toBeNull();
  none.unmount();
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', inspection: { threadId: 'thread' }, detail: 'normal', layout: 'one', setLayout: () => undefined, meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={changes} /></TimelineContextValue.Provider>);
  expect(screen.queryByRole('button', { name: i18n.t('thread.turnFiles.review') })).not.toBeInTheDocument();
});

it('finds a path in a long changed-files list without losing its Review destination', async () => {
  const edit = fileChange('many-files', { turn: 'many', changes: Array.from({ length: 12 }, (_, index) => change(`src/module-${index}.ts`, '+updated')) });
  const admitted = turnChanges(new Map([[edit.id, edit]]), [edit.id], 'many');
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined, meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<div style={{ width: 400 }}><TimelineContextValue.Provider value={context}><TurnFiles turn={turn('many', { ordinal: 2, state: 'completed', inputId: 'input' })} changes={admitted} /></TimelineContextValue.Provider></div>);
  const toggle = view.container.querySelector<HTMLElement>('[data-turn-changes="many"]');
  if (toggle === null) throw new Error('The files have their card');
  await userEvent.click(toggle);
  const list = screen.getByRole('list', { name: i18n.t('thread.turnFiles.list') });
  expect(list.closest('.overflow-y-auto')).toHaveClass('h-64');
  await userEvent.type(screen.getByRole('searchbox', { name: i18n.t('thread.turnFiles.find') }), 'module-11');
  expect(within(list).getByText('src/module-11.ts')).toBeVisible();
  expect(within(list).queryByText('src/module-0.ts')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('thread.turnFiles.review') }));
  expect(wire.review).toHaveBeenCalledExactlyOnceWith('thread', 'many');
});

it('keeps scoped native observations and tool edits without promising a complete changed-file count', () => {
  const known = turnChanges(items, ['first-edit', 'second-edit', 'other-turn'], 'one', { observed: ['shell.txt'], complete: false });
  expect(known.coverage).toBe('scoped');
  expect(known.partial).toBe(true);
  expect(known.files.map((file) => file.change.path)).toContain('shell.txt');
  for (const file of changes.files) expect(known.files.map((row) => row.change.path)).toContain(file.change.path);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const)
for (const width of [400, 1440]) for (const sourceKind of ['user', 'system'] as const)
it(`keeps a completed initial index in message actions without standing advice in ${locale}/${theme}/${width}/${sourceKind}`, async () => {
  await setLocale(locale);
  const previousTheme = document.documentElement.getAttribute('data-theme');
  document.documentElement.setAttribute('data-theme', theme);
  const source = snapshot(), threadId = source.thread.id, read = captureThreadRead(threadId);
  const author = userRef(me(41, 'recording-reader', 'Ada'));
  const first = userMessage('recording-first', 'First question', { author, turn: 'recording-one', inputId: 'recording-input', source: sourceKind });
  const second = userMessage('recording-second', 'Next question', { author, turn: 'recording-two', inputId: 'next-input', source: sourceKind });
  if (first.kind !== 'user_message' || second.kind !== 'user_message') throw new Error('Recording belongs to its original input');
  const ended = { ...turn('recording-one', { ordinal: 1, state: 'completed', inputId: 'recording-input', completedAt: AT }),
    thread_id: threadId, segment_id: 'recording-segment', recording_notice: 'indexing', recording_indexing: { done: 2, total: 2 } };
  const later = { ...turn('recording-two', { ordinal: 2, state: 'completed', inputId: 'next-input', completedAt: AT }),
    thread_id: threadId, segment_id: 'recording-segment' };
  const environment = source.thread.environment;
  if (environment === null) throw new Error('The native fixture has an environment');
  const opened = threadWriter.reset(threadId, { ...source, items: [first, second], turns: [ended, later], segments: [{
    id: 'recording-segment', ordinal: 1, agent: source.thread.binding?.agent ?? null, provider: source.thread.binding?.provider ?? null,
    environment, arrangement: 'separated', credential_path: 'native', env_generation: 1, executor: null,
    import_fidelity: null, import_report: null, ended_at: null, end_reason: null, seq_updated: 1, started_at: AT, started_by: null, tool_set: null,
  }] }, false, read);
  if (opened === null) throw new Error('The input opens in its own captured history');
  const context: TimelineContext = { threadId, threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined,
    meId: author.id, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  const request = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('A captured fact needs no read'));
  const view = show(<div style={{ width }}><ViewCompactContext.Provider value={width === 400}><TimelineContextValue.Provider value={context}>
    <UserMessage item={first} /><UserMessage item={second} />
  </TimelineContextValue.Provider></ViewCompactContext.Provider></div>);
  try {
    expect(view.container.querySelector('[data-item-body] [data-recording-notice]')).toBeNull();
    expect(screen.queryByRole('link', { name: i18n.t('largeTree.recordingWait') })).not.toBeInTheDocument();
    const details = screen.getByRole('button', { name: i18n.t('largeTree.recordingDetails') });
    const actions = details.closest('[data-hover-actions]');
    expect(actions).not.toBeNull();
    if (width === 400) expect(actions).toHaveAttribute('data-always-visible', 'true');
    expect(screen.getAllByRole('button', { name: i18n.t('largeTree.recordingDetails') })).toHaveLength(1);
    const user = userEvent.setup();
    act(() => details.focus());
    await user.keyboard('{Enter}');
    const facts = await screen.findByRole('dialog', { name: i18n.t('largeTree.recordingDetails') });
    expect(facts).toHaveTextContent(locale === 'en' ? 'Preparing recording · 2 files indexed' : '正在准备记录 · 已索引 2 个文件');
    expect(facts).toHaveTextContent(i18n.t('largeTree.recordingFileTools'));
    expect(within(facts).getByRole('link', { name: i18n.t('largeTree.recordingWait') })).toHaveAttribute('href',
      `/resources/environments/${encodeURIComponent(environment.id)}?tab=settings&setting=workspace.record_wait_secs`);
    act(() => { threadWriter.apply(opened.ref, { turns: [{ ...later, recording_indexing: { done: 200_000, total: 200_000 }, seq_updated: source.seq + 1 }] }, opened); });
    expect(facts).toHaveTextContent(locale === 'en' ? 'Preparing recording · 2 files indexed' : '正在准备记录 · 已索引 2 个文件');
    expect(facts).not.toHaveTextContent('200,000');
    await user.keyboard('{Escape}');
    expect(details).toHaveFocus();
    expect(request).not.toHaveBeenCalled();
  } finally {
    view.unmount(); threadWriter.evict(threadId); request.mockRestore();
    if (previousTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', previousTheme);
  }
});

for (const locale of ['en', 'zh-CN'] as const) for (const reason of ['too_large', 'live_tree'] as const)
it(`keeps a real recording exception compact and its explanation on intent in ${locale}/${reason}`, async () => {
  await setLocale(locale);
  const view = show(<RecordingNotice environmentId="recording-environment" reason={reason} />);
  expect(view.container).toHaveTextContent(i18n.t('largeTree.recordingUnavailable'));
  const explanation = i18n.t(reason === 'too_large' ? 'largeTree.recordingTooLarge' : 'largeTree.recordingLive');
  expect(screen.queryByText(explanation)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('largeTree.recordingDetails') }));
  const facts = await screen.findByRole('dialog', { name: i18n.t('largeTree.recordingDetails') });
  expect(facts).toHaveTextContent(explanation);
  expect(facts).toHaveTextContent(i18n.t('largeTree.recordingFileTools'));
  expect(within(facts).getByRole('link', { name: i18n.t('largeTree.recordingSetting') })).toHaveAttribute('href',
    '/resources/environments/recording-environment?tab=settings&setting=workspace.record_max_bytes');
});

for (const locale of ['en', 'zh-CN'] as const) for (const known of [true, false])
it(`keeps ${known ? 'captured' : 'unknown'} preparation facts without inventing an environment in ${locale}`, async () => {
  await setLocale(locale);
  show(<RecordingNotice environmentId={null} reason="indexing" indexing={known ? { done: 80_000, total: 200_000 } : null} placement="actions" />);
  expect(screen.queryByText(i18n.t('largeTree.recordingFileTools'))).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('largeTree.recordingDetails') }));
  const facts = await screen.findByRole('dialog', { name: i18n.t('largeTree.recordingDetails') });
  expect(facts).toHaveTextContent(i18n.t('largeTree.recordingFileTools'));
  expect(facts).toHaveTextContent(known ? i18n.t('largeTree.recordingIndexing', { files: '200,000', percent: '40' }) : i18n.t('largeTree.recordingIndexingUnknown'));
  expect(within(facts).queryByRole('link')).not.toBeInTheDocument();
});
