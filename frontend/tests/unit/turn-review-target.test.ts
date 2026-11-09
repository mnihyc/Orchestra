/** Review chooses the existing review layout panel rather than adding a second changes reader. */
import { afterEach, expect, it, vi } from 'vitest';
import { openTurnReview } from '@/features/inspector/reviewTarget';
import { allTabs, makeGroup } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import type * as Reveal from '@/features/workbench/openView';

const wire = vi.hoisted(() => ({ reveal: vi.fn(), fallback: vi.fn() }));
vi.mock('@/features/workbench/openView', async (original) => ({ ...await original<typeof Reveal>(), revealTab: wire.reveal, revealView: wire.fallback }));
const before = useWorkbench.getState();
afterEach(() => { useWorkbench.setState(before); vi.clearAllMocks(); });

it('retains the panel identity, unsaved file and exact selected turn while choosing Review', () => {
  const choose = vi.fn();
  useWorkbench.setState({ choose, layout: { ...before.layout, main: makeGroup([
    { id: 'review-native', view: 'changes', scope: { mode: 'follow' } }, { id: 'unsaved-file', view: 'file', of: { path: 'work.ts', env: 'environment' } },
  ]) } });
  openTurnReview('actual-thread', 'actual-turn');
  expect(choose).toHaveBeenCalledExactlyOnceWith('review');
  const tabs = allTabs(useWorkbench.getState().layout);
  expect(tabs.filter((tab) => tab.view === 'changes')).toEqual([{ id: 'review-native', view: 'changes', of: { thread: 'actual-thread', scope: 'turn', turn: 'actual-turn' }, scope: { mode: 'fixed', target: { kind: 'thread', id: 'actual-thread' } } }]);
  expect(tabs.some((tab) => tab.id === 'unsaved-file')).toBe(true);
  expect(wire.reveal).toHaveBeenCalledExactlyOnceWith('review-native'); expect(wire.fallback).not.toHaveBeenCalled();
});
