/** Per-turn paths remain collapsed until wanted and never borrow another turn's changes. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { TurnFiles, turnChanges } from '@/features/timeline/TurnFiles';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { i18n, setLocale } from '@/i18n';
import { change, fileChange, turn } from '../doubles/thread-rows';
import { show } from './support';

const wire = vi.hoisted(() => ({ open: vi.fn<(path: string) => void>(), mount: vi.fn<(path: string) => void>() }));
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
