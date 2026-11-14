/** One declared shortcut follows the focused content's native thread and appears in the common key book. */
import { cleanup } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Router from '@tanstack/react-router';
import { Shortcuts } from '@/shell/Shortcuts';
import { keyBook } from '@/shell/keys';
import { isApplePlatform } from '@/lib/shortcuts';
import { makeGroup, type Tab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { show } from './support';

const wire = vi.hoisted(() => ({ open: vi.fn(), navigate: vi.fn() }));
vi.mock('@/features/inspector/reviewTarget', async (loadOriginal) => ({ ...await loadOriginal<object>(), openThreadChanges: wire.open }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => wire.navigate }));
const before = useWorkbench.getState();
afterEach(() => { cleanup(); useWorkbench.setState(before); vi.clearAllMocks(); });

it.each(['thread', 'file'] as const)('shows one key and opens the actual %s content’s thread without navigation', (kind) => {
  const tab: Tab = kind === 'thread' ? { id: 'current-content', view: 'page', of: { href: `/thread/${ACME.mainThread}` } }
    : { id: 'current-content', view: 'file', of: { env: RUNS_ON.workspace, thread: ACME.mainThread, path: 'web/a.ts' } };
  useWorkbench.setState({ activeContent: tab.id, layout: { ...before.layout, main: makeGroup([tab]) } });
  show(<Shortcuts listOpen={false} onListOpenChange={vi.fn()} />);
  expect(keyBook.list().filter((binding) => binding.id === 'thread-changes')).toMatchObject([{ text: 'Mod+Alt+G', what: 'thread.changeScopes.shortcut' }]);
  const apple = isApplePlatform();
  expect(keyBook.handle(new KeyboardEvent('keydown', { key: 'g', code: 'KeyG', ctrlKey: !apple, metaKey: apple, altKey: true }))).toBe(true);
  expect(wire.open).toHaveBeenCalledExactlyOnceWith(ACME.mainThread); expect(wire.navigate).not.toHaveBeenCalled();
  expect(keyBook.handle(new KeyboardEvent('keydown', { key: 'g', code: 'KeyG', ctrlKey: !apple, metaKey: apple, altKey: true, isComposing: true }))).toBe(false);
  expect(wire.open).toHaveBeenCalledTimes(1);
});
