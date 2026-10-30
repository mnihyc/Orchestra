/** A context-free person card can open a chat and retry a lost answer safely. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import type * as Chats from '@/api/chats';
import type * as Nodes from '@/api/nodes';
import { afterEach, expect, it, vi } from 'vitest';
import type { UserProfile } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';
import { DORA } from '../doubles/chat-rows';
import { show } from './support';

const calls = vi.hoisted(() => ({ create: vi.fn<typeof Chats.createChat>(), navigate: vi.fn() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/api/chats', async (original) => ({ ...await original<typeof Chats>(), createChat: calls.create }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<typeof Nodes>(), nodeQuery: (id: string) => ({ queryKey: ['node-test', id], queryFn: () => Promise.resolve({ id, org_id: id, kind: 'org', address: 'acme' }) }) }));
vi.mock('@/api/profile', () => ({ profileQuery: (username: string) => ({ queryKey: ['profile-test', username], queryFn: () => Promise.resolve(profile) }) }));
vi.mock('@/features/profile/CardDetails', () => ({ CardDetails: () => <div>Full profile</div> }));

const profile: UserProfile = { user: DORA, bio: '', pronouns: '', links: [], local_time: null,
  member_of: [{ id: 'root', kind: 'org', name: 'Acme', redacted: false }], personal_org: null, pins: [], activity: false,
  is_me: false, can_message: true, inspected: null, joined_at: '2026-10-01T00:00:00Z', settings: null };
afterEach(() => { vi.unstubAllGlobals(); calls.create.mockReset(); calls.navigate.mockReset(); });

it('offers Message without a thing and reuses the key after a lost response', async () => {
  vi.stubGlobal('crypto', { getRandomValues: (bytes: Uint8Array) => { bytes.fill(7); return bytes; } });
  calls.create.mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce({ id: 'chat' } as Awaited<ReturnType<typeof Chats.createChat>>);
  const input = userEvent.setup(); show(<UserChip user={DORA} />);
  await input.tab();
  const message = await screen.findByRole('button', { name: 'Message' });
  expect(screen.queryByRole('button', { name: 'Notify' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Give access/ })).not.toBeInTheDocument();
  await input.click(message);
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(message).not.toHaveAttribute('aria-busy', 'true'));
  await input.click(message);
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(2));
  const first = calls.create.mock.calls[0], second = calls.create.mock.calls[1];
  expect(first?.[2]).toMatch(/^[0-9a-f-]{36}$/);
  expect(second?.[2]).toBe(first?.[2]);
  await waitFor(() => expect(calls.navigate).toHaveBeenCalled());
});
