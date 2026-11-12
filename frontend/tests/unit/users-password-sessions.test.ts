/** Password acceptance retires other cached sessions only for the reader who made the change. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { changePassword, sessionsKey } from '@/api/users';
import type { SessionInfo } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ post: vi.fn(), done: vi.fn<() => Promise<void>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: calls.post }, done: () => calls.done() };
});
// Session identity and the server's current marker are the complete projection needed here.
const sessions: Pick<SessionInfo, 'id' | 'current'>[] = [{ id: 'current', current: true }, { id: 'other', current: false }];
const original = client.session.getState();
beforeEach(() => {
  calls.post.mockReset(); calls.done.mockReset();
  calls.post.mockReturnValue(Promise.resolve(undefined)); calls.done.mockResolvedValue(undefined);
  client.session.setState({ userId: 'password-reader', readerVersion: 1 });
  queryClient.clear(); queryClient.setQueryData(sessionsKey, sessions);
});
afterEach(() => { queryClient.clear(); client.session.setState(original); });

it('removes every other cached session after password acceptance without another read', async () => {
  const fetch = vi.spyOn(globalThis, 'fetch');
  try {
    await changePassword('old fixture password', 'new fixture password');
    expect(queryClient.getQueryData(sessionsKey)).toEqual([sessions[0]]);
    expect(calls.post).toHaveBeenCalledTimes(1);
    expect(fetch).not.toHaveBeenCalled();
  } finally { fetch.mockRestore(); }
});
it('preserves the current list when the password change is refused', async () => {
  calls.done.mockRejectedValue(new Error('refused'));
  await expect(changePassword('old fixture password', 'new fixture password')).rejects.toThrow('refused');
  expect(queryClient.getQueryData(sessionsKey)).toEqual(sessions);
});
it.each(['another-reader', 'password-reader'])('does not apply a late acceptance to %s at a new authority version', async (userId) => {
  let accept!: () => void;
  calls.done.mockImplementation(() => new Promise<void>((resolve) => { accept = resolve; }));
  const pending = changePassword('old fixture password', 'new fixture password');
  const replacement = [{ id: 'new-current', current: true }, { id: 'new-other', current: false }];
  client.session.setState({ userId, readerVersion: 2 });
  queryClient.setQueryData(sessionsKey, replacement);
  accept(); await pending;
  expect(queryClient.getQueryData(sessionsKey)).toEqual(replacement);
});
