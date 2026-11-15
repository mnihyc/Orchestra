/** A turn's reported edits and observed end coverage, independent of their display. */
import type { FileChange, Item } from '@/generated/api-types';

export interface TurnChanges {
  files: { change: FileChange | { path: string; change: null; old_path?: string | null }; itemId: string | null }[];
  added: number;
  removed: number;
  partial: boolean;
  coverage: 'observed' | 'tools';
  reported: number;
}

function snapshotChanges(item: Item): FileChange[] | null {
  if (item.kind !== 'marker' || item.payload.marker !== 'harness_changed' || item.payload.data.change !== 'turn_diff') return null;
  const value = item.payload.data.value;
  if (typeof value !== 'object' || value === null || !('changes' in value) || !Array.isArray(value.changes)) return null;
  const changes: FileChange[] = [];
  const reported: unknown[] = value.changes;
  for (const raw of reported) {
    if (typeof raw !== 'object' || raw === null) return null;
    const row = raw as Record<string, unknown>;
    const { path, change, old_path, diff, added, removed, binary, truncated } = row;
    if (typeof path !== 'string' || (change !== 'add' && change !== 'modify' && change !== 'delete' && change !== 'rename')
      || !(old_path === null || typeof old_path === 'string') || !(diff === null || typeof diff === 'string')
      || typeof added !== 'number' || !Number.isFinite(added) || typeof removed !== 'number' || !Number.isFinite(removed)
      || typeof binary !== 'boolean' || typeof truncated !== 'boolean') return null;
    // Keep only the public diff fields; a summary is no substitute for a tool's retained restore images.
    changes.push({ path, change, old_path, diff, added, removed, binary, truncated });
  }
  return changes;
}

/** A native turn/stream snapshot replaces its component edits, including an empty net change. */
export function turnChanges(items: ReadonlyMap<string, Item>, order: readonly string[], turnId: string, observed?: readonly string[] | null): TurnChanges {
  const paths = new Map<string, TurnChanges['files'][number]>();
  const snapshots = new Map<string, FileChange[]>();
  const streams = new Set<string>();
  const scope = (item: Item) => JSON.stringify([item.segment_id, item.stream]);
  const held: Item[] = [];
  const reported = new Set<string>();
  for (const id of new Set([...order, ...items.keys()])) {
    const item = items.get(id);
    if (item?.turn_id !== turnId) continue;
    held.push(item);
    if (item.kind === 'tool_call' && item.payload.tool.kind === 'file_change') {
      for (const change of item.payload.tool.changes) reported.add(change.path);
    }
    const changes = snapshotChanges(item);
    if (changes === null) continue;
    snapshots.set(item.id, changes);
    streams.add(scope(item));
    for (const change of changes) reported.add(change.path);
  }
  held.sort((a, b) => Date.parse(a.updated_at) - Date.parse(b.updated_at) || a.seq_updated - b.seq_updated);
  let added = 0; let removed = 0;
  const net = new Map<string, FileChange>();
  for (const item of held) {
    const snapshot = snapshots.get(item.id);
    const edits = item.kind === 'tool_call' && item.payload.tool.kind === 'file_change' && !streams.has(scope(item)) ? item.payload.tool.changes : [];
    for (const change of snapshot ?? edits) {
      paths.set(change.path, { change, itemId: item.id });
      if (snapshot === undefined) { added += change.added; removed += change.removed; }
      else net.set(change.path, change);
    }
  }
  for (const change of net.values()) { added += change.added; removed += change.removed; }
  const known = observed !== undefined && observed !== null;
  const files = known ? [...new Set(observed)].map((path) => paths.get(path) ?? { change: { path, change: null }, itemId: null }) : [...paths.values()];
  return { files, added, removed, reported: reported.size, coverage: known ? 'observed' : 'tools', partial: !known || files.some((file) => file.itemId === null) };
}
