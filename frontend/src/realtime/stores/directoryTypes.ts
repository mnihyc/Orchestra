/** Nonvisual directory pages: a whole selected window, its source proof and explicit backpressure. */
import type { Door } from '@/api/workspace';
import type { DirEntry, DirectoryWatchError, FilesSource } from '@/generated/api-types';
import type { DirectoryTotals } from './directoryBudget';

export type DirectoryCapacityReason = 'directories' | 'rows' | 'bytes' | 'records' | 'metadata' | 'reads' | 'binding' | 'parent';
export interface DirectoryCapacity { ok: false; reason: DirectoryCapacityReason; usage: Readonly<DirectoryTotals> }
export type DirectoryAdmission = { ok: true } | DirectoryCapacity;
export type DirectoryNavigation = 'done' | 'busy' | 'capacity' | 'restart' | 'released';
export interface DirectoryState {
  status: 'loading' | 'ready' | 'failed';
  entries: readonly DirEntry[];
  source: FilesSource | null;
  asOf: string | null;
  observedFrom?: string | null;
  truncated: boolean;
  offline: boolean;
  watch?: 'waiting' | 'connecting' | 'live' | 'offline' | 'refused' | 'failed' | 'stale' | undefined;
  revision?: string | null;
  nextCursor?: string | null;
  expiresAt?: string | null;
  invalidated?: boolean;
  watchError?: DirectoryWatchError | null;
  error: unknown;
}
export interface DirectoryWindow extends DirectoryState {
  generation: number;
  page: { incoming: string | null; revision: string | null; ordinal: number } | null;
  navigation: 'idle' | 'loading';
  canNext: boolean;
  canPrevious: boolean;
  restartRequired: boolean;
  capacity: DirectoryCapacity | null;
  /** Accepted intent for this resident page; callers need no unbounded parallel expansion set. */
  expanded: readonly string[];
}
export interface DirectoryLease {
  readonly id: number;
  readonly read: () => DirectoryWindow;
  readonly subscribe: (listener: () => void) => () => void;
  first(): Promise<DirectoryNavigation>;
  next(): Promise<DirectoryNavigation>;
  previous(): Promise<DirectoryNavigation>;
  restart(): Promise<DirectoryNavigation>;
  retry(): Promise<DirectoryNavigation>;
  expanded(paths: readonly string[]): DirectoryAdmission;
  release(): Promise<void>;
}
export interface DirectoryOpen { pane: string; binding: string; parent?: DirectoryLease }
export type DirectoryOpened = { ok: true; lease: DirectoryLease } | DirectoryCapacity;
export const EMPTY: DirectoryWindow = Object.freeze({ status: 'loading', entries: [], source: null, asOf: null,
  truncated: false, offline: false, error: null, generation: 0, page: null, navigation: 'idle', canNext: false,
  canPrevious: false, restartRequired: false, capacity: null, expanded: [] });

export const directoryKey = (environmentId: string, path: string): string => `${environmentId}\u0000${path}`;
export const doorKey = (door: Door): string => JSON.stringify([door.environmentId, door.threadId]);
/** Unicode scalar ordering, matching native lowercase/exact-name ordering rather than a locale. */
function names(one: string, other: string): number {
  const left = one[Symbol.iterator](), right = other[Symbol.iterator]();
  for (;;) {
    const a = left.next(), b = right.next();
    if (a.done === true || b.done === true) return a.done === b.done ? 0 : a.done === true ? -1 : 1;
    const difference = (a.value.codePointAt(0) ?? 0) - (b.value.codePointAt(0) ?? 0);
    if (difference !== 0) return difference;
  }
}
export function sortEntries(entries: readonly DirEntry[]): DirEntry[] {
  return [...entries].sort((one, other) => {
    if ((one.type === 'dir') !== (other.type === 'dir')) return one.type === 'dir' ? -1 : 1;
    return names(one.name.toLowerCase(), other.name.toLowerCase()) || names(one.name, other.name);
  });
}
export function feedPath(path: string): string {
  let binary = '';
  for (const byte of new TextEncoder().encode(path)) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
}
export function directoryFeed(door: Door, path: string): string { return `env:${door.environmentId}:dir:${feedPath(path)}`; }
