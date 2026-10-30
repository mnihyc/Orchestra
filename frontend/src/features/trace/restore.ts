/** Deletion identities, structural restore groups and independent guarded outcomes for one reviewed selection. */
import { currentGuard } from './objects';
import { ApiError, api, ok } from '@/api/client';
import { restoreEnvironment, restoreTraceObject } from '@/api/trace';
import type { Document, Job, Node, NodeRef, Node as InspectedNode, Environment as RetainedEnvironment, AuditEntry as TraceEntry } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';

const NODE_KINDS = ['org', 'project', 'topic', 'thread'];
export type RestoreBlock = 'live' | 'purged' | 'stale' | 'refused' | 'unknown' | 'parent_first';
export interface RestoreReview {
  entry: TraceEntry; actKey: string; ancestors: NodeRef[]; block?: RestoreBlock;
  parent?: NodeRef; parentCandidate?: TraceEntry; parentUnconfirmed?: boolean; includedIn?: string; revision?: number; error?: unknown;
}
export interface RestoreResult { state: 'pending' | 'restored' | 'job' | 'failed'; value?: Node | Document | Job; error?: unknown }
/** A refusal may name the actual job that won the volume lock; it confers no new job-reading authority. */
export function refusalJob(error: unknown): string | null {
  if (!(error instanceof ApiError) || !('job' in error.problem)) return null;
  const job = error.problem.job;
  return typeof job === 'object' && job !== null && 'id' in job && typeof job.id === 'string' ? job.id : null;
}
export function selectionKey(entry: TraceEntry): string { return `${entry.object.kind}:${entry.object.id ?? ''}:${entry.event_deletion_id ?? ''}`; }
export function restoreBlock(selected: TraceEntry, current: TraceEntry | undefined): RestoreBlock | undefined {
  if (current?.object.id !== selected.object.id) return 'stale';
  if (current.object.state === 'purged') return 'purged';
  if (current.object.state === 'live') return 'live';
  if (!selected.event_deletion_id || !current.object.deletion_id) return 'unknown';
  if (selected.event_deletion_id !== current.object.deletion_id || current.event_deletion_id !== selected.event_deletion_id) return 'stale';
  if (!currentGuard(current.object.can_restore)) return 'refused';
  return undefined;
}
/** Legacy rows and old events never borrow a current deletion identity to acquire a tick. */
export function bulkRestorable(entry: TraceEntry): boolean {
  return entry.kind === 'deleted' && entry.object.state === 'deleted' && entry.object.id !== null
    && (NODE_KINDS.includes(entry.object.kind) || entry.object.kind === 'document' || entry.object.kind === 'environment')
    && restoreBlock(entry, entry) === undefined;
}
/** Only the selected ordinary node group includes descendants; environments are independently stopped resources. */
export function planGroups(rows: readonly RestoreReview[]): RestoreReview[] {
  const parents = new Map(rows.filter((row) => row.block === undefined && NODE_KINDS.includes(row.entry.object.kind)).map((row) => [row.entry.object.id, row]));
  return rows.map((row) => {
    if (row.block !== undefined || row.entry.object.kind === 'environment') return row;
    // Current paths run from the root inward; the first matching selected ancestor owns this whole group.
    const parent = row.ancestors.map((ref) => parents.get(ref.id)).find((candidate) => candidate?.entry.event_deletion_id === row.entry.event_deletion_id);
    return parent === undefined ? row : { ...row, includedIn: selectionKey(parent.entry) };
  });
}
function failureBlock(error: unknown): RestoreBlock {
  if (error instanceof ApiError) {
    if (error.status === 410) return 'purged';
    if (error.status === 403) return 'refused';
  }
  return 'stale';
}
/** The ordinary metadata readers supply current hierarchy, never guessed event-time parent paths. */
export async function reviewRestores(selected: readonly TraceEntry[], refresh: () => Promise<TraceEntry[]>): Promise<RestoreReview[]> {
  const latest = await refresh(); const current = new Map(latest.map((entry) => [entry.id, entry]));
  const rows = await Promise.all(selected.map(async (entry): Promise<RestoreReview> => {
    const row: RestoreReview = { entry, actKey: newActKey(), ancestors: [] }; const now = current.get(entry.id); row.block = restoreBlock(entry, now);
    if (row.block !== undefined || entry.object.id === null || now === undefined) return row;
    row.entry = { ...entry, object: now.object };
    try {
      if (entry.object.kind === 'environment') {
        const env: RetainedEnvironment = await ok(api.GET('/environments/{id}', { params: { path: { id: entry.object.id }, query: { inspect_deleted: true } } }));
        if (env.removal?.can_restore !== true) row.block = 'refused'; else row.revision = env.removal.revision;
      } else if (entry.object.kind === 'document') {
        const doc: Document = await ok(api.GET('/documents/{id}', { params: { path: { id: entry.object.id }, query: { inspect_deleted: true } } })); row.ancestors = doc.path;
      } else {
        const node: InspectedNode = await ok(api.GET('/nodes/{id}', { params: { path: { id: entry.object.id }, query: { inspect_deleted: true } } })); row.ancestors = node.path.filter((ref) => ref.id !== node.id);
      }
    } catch (error) { row.block = failureBlock(error); row.error = error; }
    return row;
  }));
  // In particular, a new environment deletion between the audit read and removal read must not acquire the new revision.
  const checked = new Map((await refresh()).map((entry) => [entry.id, entry]));
  for (const row of rows) row.block ??= restoreBlock(row.entry, checked.get(row.entry.id));
  const grouped = planGroups(rows);
  await Promise.all(grouped.map(async (row) => {
    if (row.block !== undefined || row.includedIn !== undefined || row.entry.object.kind === 'environment') return;
    const parent = row.ancestors.at(-1); if (parent === undefined) return;
    const selectedParent = grouped.find((candidate) => candidate.block === undefined && candidate.entry.object.id === parent.id);
    if (selectedParent !== undefined) return;
    if (latest.some((entry) => entry.object.id === parent.id && entry.object.state === 'deleted')) { row.block = 'parent_first'; row.parent = parent; row.parentCandidate = latest.find((entry) => entry.object.id === parent.id && bulkRestorable(entry)); return; }
    try { await ok(api.GET('/nodes/{id}', { params: { path: { id: parent.id } } })); }
    catch (normalError) {
      if (normalError instanceof ApiError && normalError.status === 404) {
        try { await ok(api.GET('/nodes/{id}', { params: { path: { id: parent.id }, query: { inspect_deleted: true } } })); row.block = 'parent_first'; row.parent = parent; }
        catch { row.parentUnconfirmed = true; }
      } else row.parentUnconfirmed = true;
      // A child guard grants no parent View or restore right. The mutation still checks its parent atomically.
    }
  }));
  return grouped;
}

/** Each accepted result stands on its own; a later failure cannot roll back an earlier success. */
export async function runRestores(rows: readonly RestoreReview[], report: (key: string, result: RestoreResult) => void): Promise<void> {
  const roots = rows.filter((row) => row.block === undefined && row.includedIn === undefined).sort((a, b) => a.ancestors.length - b.ancestors.length);
  for (const row of roots) {
    const key = selectionKey(row.entry); report(key, { state: 'pending' });
    try {
      const id = row.entry.object.id; if (id === null) throw new Error('no object was named');
      let value: Node | Document | Job;
      if (row.entry.object.kind === 'environment') {
        if (row.revision === undefined) throw new Error('the removal revision was not reviewed');
        value = await restoreEnvironment(id, row.revision, row.actKey);
      } else {
        const deletionId = row.entry.event_deletion_id;
        if (!deletionId) throw new Error('the deletion identity was not recorded');
        value = await restoreTraceObject(row.entry.object.kind, id, deletionId, row.actKey);
      }
      report(key, { state: row.entry.object.kind === 'environment' ? 'job' : 'restored', value });
    } catch (error) { report(key, { state: 'failed', error }); }
  }
}
