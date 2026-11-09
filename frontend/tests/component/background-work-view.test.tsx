/** A background-work view uses the admitted thread, suspends while hidden and retires an old reader's review. */
import { act, cleanup, screen } from '@testing-library/react';
import { useEffect, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import type * as Store from '@/realtime/stores/thread';
import { BackgroundWorkView } from '@/features/running/View';
import type { Thread } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';
const supplied = vi.hoisted(() => ({ shown: true, passive: false, mount: vi.fn(), unmount: vi.fn() }));
vi.mock('@/features/workbench/viewOpening', () => ({ useViewShown: () => supplied.shown }));
vi.mock('@/features/inspector/ThreadPanel', () => ({ ThreadPanel: ({ threadId, children }: { threadId?: string; children: (thread: Thread) => ReactNode }) => children({ ...snapshot(null).thread, id: threadId ?? 'route-thread', environment: null }) }));
vi.mock('@/realtime/stores/thread', async (original) => ({ ...await original<typeof Store>(), useThreadPart: () => supplied.passive }));
vi.mock('@/features/running/Running', () => ({ RunningWorkPanel: ({ thread }: { thread: string }) => {
  useEffect(() => { supplied.mount(thread); return () => supplied.unmount(thread); }, [thread]);
  return <output aria-label="Reviewed thread">{thread}</output>;
} }));
const before = client.session.getState();
const content = (id = 'one') => <BackgroundWorkView area="right" tab={{ id: 'background-tab', view: 'background-work', of: { thread: id } }} />;
beforeEach(() => { vi.clearAllMocks(); supplied.shown = true; supplied.passive = false; });
afterEach(() => { cleanup(); client.session.setState(before); });
it('opens the exact thread even without an environment and remounts on a new reader', () => {
  const view = show(content());
  expect(screen.getByLabelText('Reviewed thread')).toHaveTextContent('one');
  expect(supplied.mount).toHaveBeenCalledTimes(1);
  act(() => client.session.setState({ readerVersion: (before.readerVersion ?? 0) + 1 }));
  expect(supplied.unmount).toHaveBeenCalledWith('one');
  expect(supplied.mount).toHaveBeenCalledTimes(2);
  view.rerender(content('two'));
  expect(screen.getByLabelText('Reviewed thread')).toHaveTextContent('two');
  expect(supplied.unmount).toHaveBeenCalledTimes(2);
});
it('does not open a native review for stored inspection or a hidden view', () => {
  supplied.passive = true;
  const view = show(content());
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('composer.row.readOnly'));
  expect(supplied.mount).not.toHaveBeenCalled();
  supplied.passive = false; supplied.shown = false;
  view.rerender(content()); expect(screen.queryByLabelText('Reviewed thread')).toBeNull();
  expect(supplied.mount).not.toHaveBeenCalled();
  supplied.shown = true; view.rerender(content()); expect(supplied.mount).toHaveBeenCalledOnce();
  supplied.shown = false; view.rerender(content()); expect(supplied.unmount).toHaveBeenCalledOnce();
});
