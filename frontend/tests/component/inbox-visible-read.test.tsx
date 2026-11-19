/** Only a displayed conversation may consume its thread notifications. */
import { Activity } from 'react';
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useReadWhenOpened } from '@/features/inbox/Counters';
import { useUserStore } from '@/realtime/stores/user';

const mark = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: mark }));
const previous = useUserStore.getState();
function Conversation() { useReadWhenOpened('visible-thread'); return null; }
function Frame({ shown }: { shown: boolean }) { return <Activity mode={shown ? 'visible' : 'hidden'}><Conversation /></Activity>; }
afterEach(() => { cleanup(); useUserStore.setState(previous); mark.mockClear(); });

it('keeps notifications unread behind Chat and reads them when the actual conversation is shown', async () => {
  useUserStore.setState({ loaded: true, rows: new Map() });
  const view = render(<Frame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<Frame shown />);
  expect(mark).toHaveBeenCalledWith({ place: 'visible-thread' }, { ask: true });
  view.rerender(<Frame shown={false} />); mark.mockClear();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
});
