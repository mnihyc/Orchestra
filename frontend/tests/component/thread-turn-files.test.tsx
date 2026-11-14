/** A turn's changed files are one card, closed until wanted, that never borrows another turn's changes. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnFiles, turnChanges } from '@/features/timeline/TurnFiles';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { i18n, setLocale } from '@/i18n';
import { change, fileChange, turn } from '../doubles/thread-rows';
import { show } from './support';

const wire = vi.hoisted(() => ({ open: vi.fn<(path: string) => void>(), mount: vi.fn<(path: string) => void>(), review: vi.fn<(thread: string, turn: string) => void>() }));
vi.mock('@/features/inspector/reviewTarget', () => ({ openTurnReview: wire.review }));
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

for (const locale of ['en', 'zh-CN'] as const) it(`draws one card with the counts, lazy paths with exact Open, and Review in ${locale}`, async () => {
  await setLocale(locale);
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', detail: 'normal', layout: 'one', setLayout: () => undefined, meId: null, canInteract: true, canManage: true, live: true, lit: null, selection: NOT_PICKING };
  const view = show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={changes} /></TimelineContextValue.Provider>);
  const card = view.container.querySelector<HTMLElement>('[data-turn-files="one"]');
  if (card === null) throw new Error('A turn that changed files has its card');
  expect(card).toHaveTextContent(i18n.t('timeline.footer.changed', { count: 2, added: 2, removed: 3 }));
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
  const none = show(<TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={{ files: [], added: 0, removed: 0 }} />);
  expect(none.container.querySelector('[data-turn-files]')).toBeNull();
  none.unmount();
  const context: TimelineContext = { threadId: 'thread', threadName: 'Work', inspection: { threadId: 'thread' }, detail: 'normal', layout: 'one', setLayout: () => undefined, meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  show(<TimelineContextValue.Provider value={context}><TurnFiles turn={turn('one', { ordinal: 1, state: 'completed', inputId: 'input' })} changes={changes} /></TimelineContextValue.Provider>);
  expect(screen.queryByRole('button', { name: i18n.t('thread.turnFiles.review') })).not.toBeInTheDocument();
});
