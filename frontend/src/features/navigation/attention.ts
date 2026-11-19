/** What is unread for the person: how far they have read each thread and which places they follow, kept current by their own feed, and the rule that makes a thread unread from the two.
 *
 * A row of the tree says how far a thread has come (the last change worth a reader's attention). The
 * person's own positions say how far they have read, and their own levels say what they follow. A
 * thread is unread when it has come further than they have read, and they either have a position in
 * it or follow it: a thread nobody ever opened or asked to be told of is not shouted at a newcomer.
 *
 * Both lists are read once for the org in view and then kept by the person's own feed, which says
 * each change made on any of their devices. That feed does not repeat what it said while the
 * connection was gone, so after every connection the lists are read again. The same feed says when
 * the person's favorites and what the interface keeps for them changed elsewhere: those are read
 * again too, so that a section folded on one device folds on the other.
 */
import { useEffect } from 'react';
import { create } from 'zustand';

import { applyChanges, userEventChanges } from '@/api/acts';
import { api, client, ok } from '@/api/client';
import { isSavingSidebar } from '@/features/navigation/preferences';
import type { SubscriptionLevel, TreeNode } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import { queryClient } from '@/api/query';

export interface Attention {
  /** The last sequence the person has read, for each thread they have a position in. */
  read: ReadonlyMap<string, number>;
  /** The level the person set themself, for each node they set one on. */
  levels: ReadonlyMap<string, SubscriptionLevel>;
}

export const NO_ATTENTION: Attention = { read: new Map(), levels: new Map() };

/** Whether the person follows a node: the nearest level they set, on it or on a place above it, is not `off`. Nothing set anywhere is `off`. */
export function follows(nodeId: string, nodes: ReadonlyMap<string, Pick<TreeNode, 'parent_id'>>, levels: ReadonlyMap<string, SubscriptionLevel>): boolean {
  if (levels.size === 0) return false;
  let at: string | null = nodeId;
  // A tree is no deeper than its rows are many; the count guards against a row that names itself above.
  for (let steps = 0; at !== null && steps <= nodes.size; steps += 1) {
    const level = levels.get(at);
    if (level !== undefined) return level !== 'off';
    at = nodes.get(at)?.parent_id ?? null;
  }
  return false;
}

/** Whether a thread is unread for the person, from its row, their positions and their levels. */
export function isUnread(thread: TreeNode, nodes: ReadonlyMap<string, Pick<TreeNode, 'parent_id'>>, attention: Attention): boolean {
  if (thread.kind !== 'thread' || thread.thread === null || thread.deleted || thread.archived) return false;
  const read = attention.read.get(thread.id);
  if (read !== undefined) return thread.thread.activity_seq > read;
  return thread.thread.activity_seq > 0 && follows(thread.id, nodes, attention.levels);
}

// ---------------------------------------------------------------- the store

interface AttentionStore extends Attention {
  complete: boolean;
  /** The root org the lists were read for. */
  orgId: string | null;
}

export const useAttentionStore = create<AttentionStore>(() => ({ ...NO_ATTENTION, orgId: null, complete: false }));

/** The writers. Only the reading below and the person's feed call them. */
export const attentionWriter = {
  loadedAll(read: readonly { thread_id: string; last_read_seq: number }[], levels: readonly { node_id: string; level: SubscriptionLevel }[]): void {
    useAttentionStore.setState({ orgId: null, complete: true,
      read: new Map(read.map((state) => [state.thread_id, state.last_read_seq])),
      levels: new Map(levels.map((entry) => [entry.node_id, entry.level])),
    });
  },
  loaded(orgId: string, read: readonly { thread_id: string; last_read_seq: number }[], levels: readonly { node_id: string; level: SubscriptionLevel }[]): void {
    useAttentionStore.setState({
      orgId,
      complete: false,
      read: new Map(read.map((state) => [state.thread_id, state.last_read_seq])),
      levels: new Map(levels.map((entry) => [entry.node_id, entry.level])),
    });
  },

  /** The person read a thread up to a sequence, here or on another device. */
  read(threadId: string, seq: number): void {
    useAttentionStore.setState((state) => {
      if (state.read.get(threadId) === seq) return state;
      return { read: new Map(state.read).set(threadId, seq) };
    });
  },

  /** The person set a level on a node, or took their choice back (`null`). */
  level(nodeId: string, level: SubscriptionLevel | null): void {
    useAttentionStore.setState((state) => {
      if ((state.levels.get(nodeId) ?? null) === level) return state;
      const levels = new Map(state.levels);
      if (level === null) levels.delete(nodeId);
      else levels.set(nodeId, level);
      return { levels };
    });
  },

  clear(): void {
    useAttentionStore.setState({ ...NO_ATTENTION, orgId: null, complete: false });
  },
};

client.onSession((event) => {
  if (event.type === 'signed_out') attentionWriter.clear();
});

/** Reads the person's positions and levels in one tree of orgs. A reading that fails leaves what is held: the next connection reads again. */
async function load(orgId: string): Promise<void> {
  if (useAttentionStore.getState().complete) return;
  try {
    const [read, levels] = await Promise.all([
      queryClient.query({ queryKey: ['read-states', orgId], staleTime: Infinity, queryFn: () => ok(api.GET('/me/read-states', { params: { query: { org_id: orgId } } })) }),
      queryClient.query({ queryKey: ['subscriptions', orgId], staleTime: Infinity, queryFn: () => ok(api.GET('/me/subscriptions', { params: { query: { org_id: orgId } } })) }),
    ]);
    // An answer for an org that is no longer in view is not what the bar shows.
    if (wanted === orgId) attentionWriter.loaded(orgId, read, levels);
  } catch {
    // Nothing unread is shown that is not known to be: what is held stays.
  }
}

/** Reads the person's positions and levels again, for the org in view: after an act that changed many of them, or one that failed part of the way. */
export function reloadAttention(): void {
  if (wanted !== null) void load(wanted);
}

/** The org whose lists are wanted at the moment, and how many views want them. */
let wanted: string | null = null;
let watchers = 0;
let stopFeed: (() => void) | null = null;

/** What the person's own feed says that the left bar follows. Events arrive as `{ what, ... }`. */
function onEvent(data: Record<string, unknown>): void {
  switch (data.what) {
    case 'read_state':
      if (typeof data.thread_id === 'string' && typeof data.last_read_seq === 'number') attentionWriter.read(data.thread_id, data.last_read_seq);
      return;
    case 'subscription':
      if (typeof data.node_id === 'string') attentionWriter.level(data.node_id, typeof data.level === 'string' ? (data.level as SubscriptionLevel) : null);
      return;
    case 'favorites':
      void applyChanges(userEventChanges('favorites', data));
      return;
    case 'state':
      // A change this page is sending is its own, and is already shown: reading now could bring back the value before it.
      void applyChanges(userEventChanges('state', data).filter((change) => !isSavingSidebar() || !('key' in change && change.key[0] === 'me-state')));
      return;
    default:
  }
}

function follow(): () => void {
  const subscription = socket.subscribe('user', {
    // The feed is listened to for its events alone; its rows belong to whoever shows the Inbox.
    since: () => null,
    onFrame: (frame) => {
      if (frame.op === 'subscribed') {
        // Events are not repeated: what they would have said while the connection was gone is read again.
        if (wanted !== null) void load(wanted);
        void applyChanges([...userEventChanges('favorites', {}), ...userEventChanges('state', {})]);
      } else if (frame.op === 'event') {
        onEvent(frame.data);
      }
    },
  });
  return () => subscription.unsubscribe();
}

/**
 * Keeps what is unread in a tree of orgs current for as long as the calling view is shown: reads the
 * person's positions and levels there, and follows their own feed.
 */
export function useAttentionFeed(orgId: string | null): void {
  useEffect(() => {
    if (orgId === null) return;
    watchers += 1;
    stopFeed ??= follow();
    if (wanted !== orgId) {
      wanted = orgId;
      void load(orgId);
    }
    return () => {
      watchers -= 1;
      if (watchers === 0) {
        stopFeed?.();
        stopFeed = null;
        wanted = null;
      }
    };
  }, [orgId]);
}

/** The person's positions and levels in the org in view; nothing while they are those of another org. */
export function useAttention(orgId: string | null): Attention {
  const held = useAttentionStore();
  return orgId !== null && (held.complete || held.orgId === orgId) ? held : NO_ATTENTION;
}
