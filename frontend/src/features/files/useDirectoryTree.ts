/** One Files pane owns only its resident directory pages and reachable expanded branches. */
import { useEffect, useId, useState, useSyncExternalStore } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { Door } from '@/api/workspace';
import { useViewShown } from '@/features/workbench/viewOpening';
import { directoryKey, openDirectory, type DirectoryCapacity, type DirectoryLease, type DirectoryWindow } from '@/realtime/stores/environment';

interface Held { lease: DirectoryLease; parent: string | null; unsubscribe: () => void }
interface Snapshot {
  identity: string;
  directories: ReadonlyMap<string, DirectoryWindow>;
  expanded: ReadonlySet<string>;
  capacity: { path: string; refusal: DirectoryCapacity } | null;
}
const EMPTY: Snapshot = { identity: '', directories: new Map(), expanded: new Set(), capacity: null };

/** The ledger owns payloads and expansion intent. This controller holds only admitted leases. */
export class DirectoryTree {
  private readonly held = new Map<string, Held>();
  private listener: (() => void) | null = null;
  private snapshot = EMPTY;
  private door: Door | null = null;
  private root = '';
  private pane = '';
  private binding = '';
  private identity = '';
  private queued = false;
  private capacity: Snapshot['capacity'] = null;
  readonly read = (): Snapshot => this.snapshot;
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listener = listener;
    return () => { if (this.listener === listener) this.listener = null; };
  };
  start(door: Door, root: string, pane: string, binding: string, identity: string): void {
    this.stop(); this.door = door; this.root = root; this.pane = pane; this.binding = binding; this.identity = identity;
    this.acquire(root, null); this.synchronize();
  }
  stop(): void {
    for (const one of this.held.values()) { one.unsubscribe(); void one.lease.release(); }
    this.held.clear(); this.door = null; this.identity = ''; this.capacity = null;
    this.snapshot = EMPTY; this.listener?.();
  }
  private schedule = (): void => {
    if (this.queued) return;
    this.queued = true;
    queueMicrotask(() => { this.queued = false; this.synchronize(); });
  };
  private acquire(path: string, parent: string | null): boolean {
    if (this.door === null) return false;
    const ancestor = parent === null ? undefined : this.held.get(parent)?.lease;
    if (parent !== null && ancestor === undefined) return false;
    const opened = openDirectory(this.door, path, { pane: this.pane, binding: this.binding, ...(ancestor === undefined ? {} : { parent: ancestor }) });
    if (!opened.ok) { this.capacity = { path, refusal: opened }; return false; }
    const lease = opened.lease;
    this.held.set(path, { lease, parent, unsubscribe: lease.subscribe(this.schedule) });
    this.capacity = null;
    void lease.first();
    return true;
  }
  private release(path: string): void {
    for (const [child, one] of this.held) if (one.parent === path) this.release(child);
    const one = this.held.get(path);
    if (one === undefined) return;
    one.unsubscribe(); this.held.delete(path); void one.lease.release();
  }
  private synchronize(): void {
    if (this.door === null) return;
    for (const [path, one] of this.held) if (one.parent !== null) {
      const parent = this.held.get(one.parent)?.lease.read();
      if (parent === undefined || !parent.expanded.includes(path)
        || !parent.entries.some((entry) => entry.type === 'dir' && entry.path === path)) this.release(path);
    }
    const directories = new Map<string, DirectoryWindow>(), expanded = new Set<string>();
    for (const [path, one] of this.held) {
      const window = one.lease.read(); directories.set(directoryKey(this.door.environmentId, path), window);
      for (const child of window.expanded) if (this.held.has(child)) expanded.add(child);
    }
    this.snapshot = { identity: this.identity, directories, expanded, capacity: this.capacity };
    this.listener?.();
  }
  toggle(path: string, expanded: boolean): void {
    const parent = [...this.held].find(([, one]) => one.lease.read().entries.some((entry) => entry.type === 'dir' && entry.path === path));
    if (parent === undefined) return;
    const [parentPath, one] = parent, before = one.lease.read().expanded;
    if (expanded === before.includes(path)) return;
    if (!expanded) {
      one.lease.expanded(before.filter((value) => value !== path)); this.release(path); this.capacity = null;
    } else {
      const accepted = one.lease.expanded([...before, path]);
      if (!accepted.ok) this.capacity = { path, refusal: accepted };
      else if (!this.acquire(path, parentPath)) one.lease.expanded(before);
    }
    this.synchronize();
  }
  collapse(): void {
    const root = this.held.get(this.root);
    root?.lease.expanded([]);
    for (const path of [...this.held.keys()]) if (path !== this.root) this.release(path);
    this.capacity = null; this.synchronize();
  }
  navigate(path: string, action: 'next' | 'previous' | 'restart' | 'retry'): void {
    const one = this.held.get(path);
    if (one !== undefined) {
      this.capacity = null;
      const state = one.lease.read();
      if (action === 'restart' && (state.watch === 'failed' || state.watch === 'refused')) {
        void one.lease.retry().then((result) => { if (result === 'restart') return one.lease.restart(); });
      } else void one.lease[action]();
    }
    else if (path === this.root) { this.acquire(path, null); this.synchronize(); }
  }
  refresh(): void {
    this.capacity = null;
    for (const path of this.held.keys()) this.navigate(path, 'restart');
    if (this.held.size === 0) { this.acquire(this.root, null); this.synchronize(); }
  }
}

export function useDirectoryTree(door: Door, root: string, binding: string | null, enabled = true) {
  const pane = useId(), shown = useViewShown();
  const reader = useStore(client.session, (state) => state.readerVersion);
  const [tree] = useState(() => new DirectoryTree());
  const identity = JSON.stringify([reader, door.environmentId, door.threadId, root, binding]);
  const snapshot = useSyncExternalStore(tree.subscribe, tree.read, () => EMPTY);
  useEffect(() => {
    if (!shown || !enabled || binding === null) return;
    tree.start({ environmentId: door.environmentId, threadId: door.threadId }, root, pane, binding, identity);
    return () => tree.stop();
  }, [binding, door.environmentId, door.threadId, enabled, identity, pane, root, shown, tree]);
  return { ...(snapshot.identity === identity && shown && enabled ? snapshot : EMPTY), tree };
}
