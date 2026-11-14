/** Only a displayed conversation may consume its thread notifications. */
import { Activity } from 'react';
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useReadWhenOpened } from '@/features/inbox/Counters';
import { useUserStore } from '@/realtime/stores/user';
import { FrameReadiness } from '@/shell/FrameReadiness';

const mark = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: mark }));
const previous = useUserStore.getState();
function Conversation() { useReadWhenOpened('visible-thread'); return null; }
function Frame({ shown }: { shown: boolean }) { return <Activity mode={shown ? 'visible' : 'hidden'}><Conversation /></Activity>; }
afterEach(() => { cleanup(); useUserStore.setState(previous); mark.mockClear(); });

it('keeps notification state unchanged when a conversation is shown again', async () => {
  useUserStore.setState({ loaded: true, rows: new Map() });
  const view = render(<Frame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<Frame shown />);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<Frame shown={false} />); mark.mockClear();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
});

it('does not write a read mark merely because an admitted thread has unread notifications', () => {
  useUserStore.setState({ loaded: false, rows: new Map(), counters: { unread: 2, approvalsPending: 0 }, reachedEnd: false });
  render(<Frame shown />);
  expect(mark).not.toHaveBeenCalled();
  expect(useUserStore.getState().loaded).toBe(false);
  expect(useUserStore.getState().rows.size).toBe(0);
});

it('waits for frame admission and stops reading when that admission is lost', async () => {
  useUserStore.setState({ loaded: false, rows: new Map(), counters: { unread: 2, approvalsPending: 0 }, reachedEnd: false });
  const view = render(<FrameReadiness.Provider value={{ ready: false, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<FrameReadiness.Provider value={{ ready: true, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<FrameReadiness.Provider value={{ ready: false, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  mark.mockClear();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
});
