/** The person's eight most recent distinct platform emoji, carried by their existing state and feed. */
import { create } from 'zustand';

import { meStateQuery, setMeState } from '@/api/personal';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { onUserEvent } from '@/realtime/stores/user';
import { EMOJI } from '@/ui/emoji-data';

/** The chosen size of the often-used row; empty history starts with these four familiar reactions. */
export const RECENT_REACTIONS = 8;
export const DEFAULT_REACTIONS = ['👍', '✅', '👀', '🎉'];
const platform = new Set(Object.values(EMOJI).flat().map((entry) => entry[0]));
export function normalizeReactions(value: unknown): string[] {
  return Array.isArray(value) ? [...new Set(value.filter((emoji): emoji is string => typeof emoji === 'string' && platform.has(emoji)))].slice(0, RECENT_REACTIONS) : [];
}
export function recentAfter(current: readonly string[], chosen: string): string[] {
  return normalizeReactions([chosen, ...current]);
}
export const useReactionHistory = create<{ user: string | null; recent: string[] }>(() => ({ user: null, recent: [] }));
export async function loadReactionHistory(): Promise<void> {
  const user = client.session.getState().userId;
  if (user === null) return;
  if (useReactionHistory.getState().user !== user) useReactionHistory.setState({ user, recent: [] });
  const state = await queryClient.query({ ...meStateQuery, staleTime: Infinity });
  if (client.session.getState().userId === user) useReactionHistory.setState({ user, recent: normalizeReactions(state.reactions) });
}
let writes = Promise.resolve();
let pendingWrites = 0;
/** Selection stays immediate; serial writes cannot put an older row over a newer one on this device. */
export function rememberReaction(chosen: string): void {
  const user = client.session.getState().userId;
  if (user === null) return;
  const held = useReactionHistory.getState();
  const recent = recentAfter(held.user === user ? held.recent : [], chosen);
  useReactionHistory.setState({ user, recent });
  pendingWrites += 1;
  writes = writes.catch(() => undefined).then(async () => {
    if (client.session.getState().userId !== user) return;
    await setMeState('reactions', recent);
    if (client.session.getState().userId === user) queryClient.setQueryData<Record<string, unknown>>(meStateQuery.queryKey, (state) => ({ ...state, reactions: recent }));
  }).catch(() => undefined).finally(() => {
    pendingWrites -= 1;
    if (pendingWrites === 0) void loadReactionHistory().catch(() => undefined);
  });
}
onUserEvent('state', (data) => {
  if (data.key !== 'reactions' || pendingWrites > 0) return;
  if ('value' in data) {
    const user = client.session.getState().userId;
    if (user !== null) useReactionHistory.setState({ user, recent: normalizeReactions(data.value) });
  }
});
