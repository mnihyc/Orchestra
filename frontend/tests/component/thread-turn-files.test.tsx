/** Per-turn paths remain collapsed until wanted and never borrow another turn's changes. */
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnFiles, turnChanges } from '@/features/timeline/TurnFiles';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { captureThreadRead } from '@/api/threadRead';
import { threadWriter } from '@/realtime/stores/thread';
import type { VirtualListProps } from '@/ui/VirtualList';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { i18n, setLocale } from '@/i18n';
import { change, fileChange, marker, turn } from '../doubles/thread-rows';
import { show } from './support';
import { A, snapshot } from '../unit/continuation-fixtures';

const wire = vi.hoisted(() => ({ open: vi.fn<(path: string) => void>(), mount: vi.fn<(path: string) => void>(), review: vi.fn() }));
vi.mock('@/features/inspector/reviewTarget', () => ({ openTurnReview: wire.review }));
vi.mock('@/ui/VirtualList', () => ({ VirtualList: (props: VirtualListProps) => <div className={props.className}><ol aria-label={props.label}>
  {Array.from({ length: props.count }, (_, index) => <li key={props.keyOf(index)}>{props.render(index)}</li>)}
</ol></div> }));
vi.mock('@/features/files/FileLink', () => ({ FileLink: ({ path, children }: { path: string; children: ReactNode }) => { wire.mount(path); return <button onClick={() => wire.open(path)}>{children}</button>; } }));
vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: ({ turn }: { turn: { id: string } }) => <button data-restore-turn={turn.id}>Restore this turn</button> }));
afterEach(async () => { cleanup(); vi.clearAllMocks(); await setLocale('en'); });
const first = fileChange('first-edit', { turn: 'one', changes: [change('src/a.ts', '@@ -1 +1 @@\n-old\n+new'), change('gone.txt', '-removed', { change: 'delete' })] });
const second = fileChange('second-edit', { turn: 'one', changes: [change('src/a.ts', '@@ -1 +1 @@\n-new\n+final')] });
const other = fileChange('other-turn', { turn: 'two', changes: [change('private-other.ts', '+unrelated')] });
const items = new Map([first, second, other].map((item) => [item.id, item]));
const changes = turnChanges(items, ['first-edit', 'second-edit', 'other-turn'], 'one');

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
  expect(turnChanges(held, order, 'one')).toEqual({ files: [], added: 0, removed: 0 });
  expect(first.kind).toBe('tool_call');
});

it('applies a child streams snapshot without clearing another streams original edits', () => {
  const child = fileChange('child-edit', { turn: 'one', stream: 'child', changes: [change('child-old.ts', '+old')] });
  const snap = { ...snapshotFile('child-diff', [change('child-net.ts', '+new')]), stream: 'child' };
  const held = new Map([...items, [child.id, child], [snap.id, snap]]);
  const result = turnChanges(held, ['first-edit', 'second-edit', 'other-turn'], 'one');
  expect(result.files.map(({ change }) => change.path)).toEqual(['src/a.ts', 'gone.txt', 'child-net.ts']);
  held.set(snap.id, { ...snapshotFile(snap.id, []), stream: 'child' });
  expect(turnChanges(held, ['first-edit', 'second-edit'], 'one')).toEqual(changes);
});

for (const locale of ['en', 'zh-CN'] as const) it(`finds a path in a long aggregate file list without opening another workspace in ${locale}`, async () => {
  await setLocale(locale);
  const user = userEvent.setup();
  const snap = snapshotFile('many-files', Array.from({ length: 12 }, (_, index) => change(`src/path-${index}.ts`, '+new')));
  const reported = turnChanges(new Map([[snap.id, snap]]), [snap.id], 'one');
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'steps', layout: 'one', setLayout: () => undefined,
    recent: new Set(), meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const draw = (held: typeof reported) => <TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={held} /></TimelineContextValue.Provider>;
  const view = show(draw(reported));
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(wire.mount).not.toHaveBeenCalled();
  const disclosure = view.container.querySelector('details');
  if (disclosure === null) throw new Error('The long list begins collapsed');
  disclosure.open = true; fireEvent(disclosure, new Event('toggle', { bubbles: true }));
  await user.type(await screen.findByRole('searchbox', { name: i18n.t('thread.turnFiles.find') }), 'path-11');
  expect(screen.getByText('src/path-11.ts')).toBeVisible();
  expect(screen.queryByText('src/path-0.ts')).not.toBeInTheDocument();
  const scroll = view.container.querySelector('[data-turn-files] .overflow-y-auto');
  expect(scroll).not.toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('inspector.changes.openFile') }));
  expect(wire.open).toHaveBeenCalledExactlyOnceWith('src/path-11.ts');
  view.rerender(draw({ files: reported.files.slice(0, 2), added: 2, removed: 0 }));
  expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
  expect(screen.getByText('src/path-0.ts')).toBeVisible();
  view.rerender(draw({ files: [], added: 0, removed: 0 }));
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
  const context: TimelineContext = { threadId, threadName: 'Work', detail: 'steps', layout: 'one', setLayout: () => undefined,
    recent: new Set(), meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={done.id} /></TimelineContextValue.Provider>);
  expect(await screen.findByText(i18n.t('timeline.footer.changed', { count: 1, added: 2, removed: 1 }))).toBeVisible();
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
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'steps', layout: 'one', setLayout: () => undefined,
    recent: new Set(), meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={reported} /></TimelineContextValue.Provider>);
  const disclosure = view.container.querySelector('details');
  if (disclosure === null) throw new Error('The reported file needs its ordinary disclosure');
  disclosure.open = true; fireEvent(disclosure, new Event('toggle', { bubbles: true }));
  expect(await screen.findByText('only-in-snapshot.ts')).toBeVisible();
});

for (const locale of ['en', 'zh-CN'] as const) for (const detail of ['summary', 'steps', 'everything'] as const) it(`shows only a count in Focused and lazy paths with exact Open in ${locale}/${detail}`, async () => {
  await setLocale(locale);
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail, layout: 'one', setLayout: () => undefined,
    recent: new Set(), meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={changes} /></TimelineContextValue.Provider>);
  expect(screen.getByText(i18n.t('thread.viewModes.files', { count: 2 }))).toBeVisible();
  expect(wire.mount).not.toHaveBeenCalled();
  expect(screen.queryByText('src/a.ts')).not.toBeInTheDocument();
  if (detail === 'summary') { expect(view.container.querySelector('details')).toBeNull(); return; }
  const disclosure = view.container.querySelector('details');
  if (disclosure === null) throw new Error('Full and Live keep the file disclosure');
  disclosure.open = true; fireEvent(disclosure, new Event('toggle', { bubbles: true }));
  await screen.findByText('src/a.ts');
  expect(screen.queryByText('private-other.ts')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('inspector.changes.openFile') }));
  expect(wire.open).toHaveBeenCalledExactlyOnceWith('src/a.ts');
  expect(screen.queryByRole('button', { name: 'Restore this turn' })).not.toBeInTheDocument();
});
