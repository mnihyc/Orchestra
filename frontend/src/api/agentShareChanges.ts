/** Accepted share rows update their source; independently assessed recipients keep their own suppliers. */
import type { QueryClient, QueryKey } from '@tanstack/react-query';
import type { Act } from '@/api/client';
import type { Change } from '@/api/acts';
import { record } from '@/api/cache';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';

interface Source { object: 'nodes' | 'documents' | 'chats'; id: string }
export interface AgentSharesChange { agentShares: { source?: Source; rows: Record<string, unknown>[]; ids: string[]; removed: boolean } }
const idOf = (value: unknown): string | undefined => record(value) && typeof value.id === 'string' ? value.id : undefined;
function sourceOf(value: unknown): Source | undefined {
  if (!record(value)) return undefined;
  for (const [field, object] of [['node_id', 'nodes'], ['document_id', 'documents'], ['chat_id', 'chats']] as const) {
    if (typeof value[field] === 'string') return { object, id: value[field] };
  }
  const id = idOf(value);
  if (id === undefined) return undefined;
  if (['org', 'project', 'topic', 'thread'].includes(String(value.kind))) return { object: 'nodes', id };
  if (typeof value.title === 'string') return { object: 'documents', id };
  if (['node', 'group', 'direct'].includes(String(value.kind))) return { object: 'chats', id };
  return undefined;
}
function complete(row: Record<string, unknown>): boolean {
  return idOf(row) !== undefined && row.projection !== 'receipt' && idOf(row.what) !== undefined && idOf(row.to) !== undefined && Array.isArray(row.entries)
    && record(row.parts) && typeof row.at === 'string' && typeof row.inside === 'boolean'
    && typeof row.download === 'boolean' && typeof row.read_first === 'boolean'
    && (row.mode === 'live' || row.mode === 'snapshot');
}
/** Restricted receipts never become content rows or resource identities. */
export function agentShareAnswer(act: Act): AgentSharesChange[] | null {
  const parts = act.path.split('/').filter(Boolean).map(decodeURIComponent);
  if (parts[0] !== 'share' || parts[1] !== 'agents' || parts.length > 3) return null;
  const rows = record(act.answer) && Array.isArray(act.answer.shares) ? act.answer.shares.filter(record)
    : record(act.answer) ? [act.answer] : [];
  const ids = [...new Set([...rows.flatMap((row) => idOf(row) ?? []), ...(parts[2] === undefined ? [] : [parts[2]])])];
  return [{ agentShares: { source: sourceOf(record(act.input) ? act.input.what : undefined), rows, ids, removed: act.method === 'DELETE' } }];
}
function below(cache: QueryClient, id: string, ancestor: string): boolean {
  if (id === ancestor) return true;
  const node = cache.getQueryData(['node', id]) ?? cache.getQueryData(['thread', id]);
  if (record(node) && Array.isArray(node.path) && node.path.some((part: unknown) => idOf(part) === ancestor)) return true;
  for (const tree of useTreeStore.getState().byOrg.values()) if (pathTo(tree, id).some((part) => part.id === ancestor)) return true;
  return false;
}
function sharesIn(data: unknown): Record<string, unknown>[] {
  const rows = Array.isArray(data) ? data : record(data) && Array.isArray(data.shares) ? data.shares : [];
  return rows.filter(record);
}
/** Select only loaded owners using accepted identities, existing rows and native ancestry; never discover a recipient. */
export function agentSharePatches(cache: QueryClient, change: AgentSharesChange['agentShares']): Change[] {
  const queries = cache.getQueryCache().getAll();
  const held = queries.filter((query) => query.queryKey[0] === 'agent-shares').flatMap((query) => sharesIn(query.state.data))
    .filter((row) => typeof row.id === 'string' && change.ids.includes(row.id));
  const rows = [...change.rows, ...held];
  const sources = new Map<string, Source>();
  if (change.source !== undefined) sources.set(`${change.source.object}/${change.source.id}`, change.source);
  for (const row of rows) { const source = sourceOf(row.what); if (source !== undefined) sources.set(`${source.object}/${source.id}`, source); }
  for (const query of queries) if (query.queryKey[0] === 'agent-shares' && query.queryKey[3] === 'of'
    && sharesIn(query.state.data).some((row) => typeof row.id === 'string' && change.ids.includes(row.id))) {
    const [, object, id] = query.queryKey;
    if ((object === 'nodes' || object === 'documents' || object === 'chats') && typeof id === 'string') sources.set(`${object}/${id}`, { object, id });
  }
  const patches: Change[] = [], repairs = new Map<string, QueryKey>();
  for (const source of sources.values()) {
    const key = ['agent-shares', source.object, source.id, 'of'];
    const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
    if (owner !== undefined) {
      // One accepted row cannot stand in for an as-yet unread collection.
      if (!Array.isArray(owner.state.data) || change.ids.length === 0) repairs.set(JSON.stringify(key), key);
      else if (change.removed) for (const id of change.ids) patches.push({ collection: { key, identity: 'id', id, data: null }, cancelPending: true });
      else {
        const supplied = change.rows.filter((row) => complete(row) && idOf(row.what) === source.id);
        if (supplied.length !== change.ids.length) repairs.set(JSON.stringify(key), key);
        else for (const row of supplied) patches.push({ collection: { key, identity: 'id', id: String(row.id), data: row }, cancelPending: true });
      }
    }
    // The share answer omits permission-page and defaults projections, even when it supplies its source list row.
    for (const query of queries) {
      const [head, object, id] = query.queryKey;
      if (['page', 'page-history', 'page-effective', 'page-people'].includes(String(head)) && object === source.object && id === source.id
        || head === 'defaults' && source.object === 'nodes' && object === source.id) repairs.set(query.queryHash, query.queryKey);
    }
  }
  const recipients = rows.flatMap((row) => idOf(row.to) ?? []);
  for (const query of queries) {
    const [head, object, id, side] = query.queryKey;
    const recipient = head === 'agent-shares' && object === 'nodes' && side === 'to' ? id
      : head === 'thread-shared' || head === 'thread-shared-tree' ? object : undefined;
    if (typeof recipient !== 'string') continue;
    if (sharesIn(query.state.data).some((row) => typeof row.id === 'string' && change.ids.includes(row.id))
      || recipients.some((target) => below(cache, recipient, target))) repairs.set(query.queryHash, query.queryKey);
  }
  for (const key of repairs.values()) {
    const [head, ...tail] = key;
    if (typeof head === 'string') patches.push({ missing: [{ key: [head, ...tail], exact: true }], cancelPending: true });
  }
  return patches;
}
