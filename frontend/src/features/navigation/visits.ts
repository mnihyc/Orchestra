/** Keeping this device's recent places and their local order without another page-view request.
 *
 * The authorized page read carries its visit to the server. The server answers "which did I go to lately" in
 * the finder's names, and where it does not, the left bar lists what this device saw itself: the
 * last few places the person opened here, kept for them alone on this device.
 */
import { useEffect, useMemo } from 'react';
import { useStore } from 'zustand';
import { create } from 'zustand';

import { client } from '@/api/client';
import { type NameKind, namesQuery } from '@/api/search';
import { useCurrentHeading } from '@/shell/screen';

/** How many recents the left bar lists: the last eight things a person opened in the org, which is what fits under a heading without becoming a second tree. */
export const RECENT_ROWS = 8;

/** What a recent can be: a place of any kind, or a document written beside one. */
const RECENT_KINDS: readonly NameKind[] = ['org', 'project', 'topic', 'thread', 'document'];

/**
 * The things the person opened last in one tree of orgs, the newest first. Two more than are listed are asked
 * for: the org itself and the screen in view are left out of the list, and the list is still to be full.
 */
export const recentsOfOrgQuery = (orgId: string) => namesQuery({ q: '', kinds: RECENT_KINDS, limit: RECENT_ROWS + 2, orgId });

/** How many places the device keeps: the list shows a few, and the rest is for those that are gone from the tree meanwhile. */
const KEPT = RECENT_ROWS * 3;

const storageKey = (userId: string) => `orch.${userId}.recents`;

function read(userId: string | null): string[] {
  if (userId === null) return [];
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(storageKey(userId)) ?? '[]');
    return Array.isArray(stored) ? stored.filter((id): id is string => typeof id === 'string').slice(0, KEPT) : [];
  } catch {
    return [];
  }
}

interface Seen {
  /** The ids of the places this device opened, last first. */
  ids: readonly string[];
  /** Whose they are: a person who signs in after another starts from their own list. */
  userId: string | null;
  /** This tab's visit times, on the same client clock as a query's dataUpdatedAt; not persisted. */
  visitedAt: ReadonlyMap<string, number>;
}

const useSeen = create<Seen>(() => ({ ids: [], userId: null, visitedAt: new Map() }));

/** The places this person opened on this device, last first. */
export function useSeenHere(): readonly string[] {
  const userId = useStore(client.session, (state) => state.userId);
  const seen = useSeen();
  return seen.userId === userId ? seen.ids : read(userId);
}

/** Newly opened places precede an older held recent-visits answer, without refetching that list. */
export function useSeenAfter(snapshotAt: number): readonly string[] {
  const userId = useStore(client.session, (state) => state.userId);
  const seen = useSeen();
  return useMemo(() => seen.userId !== userId ? [] : seen.ids.filter((id) => (seen.visitedAt.get(id) ?? 0) > snapshotAt), [seen, userId, snapshotAt]);
}

function remember(userId: string, id: string): void {
  const before = useSeen.getState();
  const ids = [id, ...(before.userId === userId ? before.ids : read(userId)).filter((other) => other !== id)].slice(0, KEPT);
  const now = Date.now();
  const visitedAt = new Map(ids.map((one) => [one, one === id ? now : before.userId === userId ? before.visitedAt.get(one) ?? 0 : 0]));
  useSeen.setState({ ids, userId, visitedAt });
  try {
    localStorage.setItem(storageKey(userId), JSON.stringify(ids));
  } catch {
    // The authorized page read records the visit even if this device cannot keep its local order.
  }
}

/** Stands once in the frame: remember directly opened nodes here; the authorized read records the server visit. */
export function useVisitRecorder(): void {
  const userId = useStore(client.session, (state) => state.userId);
  const heading = useCurrentHeading();
  const nodeId = heading?.nodeId;
  const kind = heading?.target?.kind;
  const itemId = heading?.target?.id;
  const directNode = kind === undefined || ((kind === 'org' || kind === 'project' || kind === 'topic' || kind === 'thread') && itemId === nodeId);
  useEffect(() => {
    if (userId === null) return;
    if (nodeId !== undefined && directNode) remember(userId, nodeId);
    // The authorized page read carries its server visit; a cached opening only updates this device.
  }, [userId, nodeId, kind, itemId, directNode]);
}
