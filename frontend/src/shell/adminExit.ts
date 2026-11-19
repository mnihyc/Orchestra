/** A confirmed end to administrator reach returns a private page to its nearest ordinary place. */
import type { QueryClient } from '@tanstack/react-query';
import { meKey } from '@/api/auth';
import { documentKey } from '@/api/documents';
import { nodeKey, orgBySlugKey, orgsKey } from '@/api/nodes';
import type { Me, NodeKind, NodeRef, Org } from '@/generated/api-types';
import { documentStateNow } from '@/realtime/stores/document';
import { useThreadStore } from '@/realtime/stores/thread';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';
import { currentHeading } from '@/shell/screen';
import { adminAccessOf } from '@/ui/AsAdminMark';

export interface ExitPlace { href: string; id?: string; kind?: 'node' | 'org' | 'doc'; access: 'yes' | 'no' | 'unknown' }
export interface AdminExitPlan { origin: string; person: string | null; current: ExitPlace | null; parents: readonly ExitPlace[]; fallback: string }

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const nodeKind = (value: unknown): value is NodeKind => value === 'org' || value === 'project' || value === 'topic' || value === 'thread';

/** The API records when View itself needed reach, separately from administrative actions. */
export function ordinaryView(value: unknown, permission?: string): ExitPlace['access'] {
  if (!record(value)) return 'unknown';
  if (adminAccessOf(value) !== null) return 'no';
  const access = record(value.access) ? value.access : record(value.can) ? value.can : value;
  if (access === value && !('admin_access' in value) && !('admin_only' in value)) return 'unknown';
  if (permission !== undefined && Array.isArray(access.permissions) && !access.permissions.includes(permission)) return 'no';
  if (permission !== undefined && Array.isArray(access.admin_only) && access.admin_only.some((key: unknown) => key === permission)) return 'no';
  return 'yes';
}

function pathOf(value: unknown): NodeRef[] {
  if (!record(value) || !Array.isArray(value.path)) return [];
  return value.path.flatMap((part: unknown): NodeRef[] => !record(part) || typeof part.id !== 'string' || !nodeKind(part.kind) ? [] : [{
    id: part.id, kind: part.kind, name: typeof part.name === 'string' ? part.name : null,
    redacted: part.redacted === true || typeof part.name !== 'string',
  }]);
}

function cachedNode(cache: QueryClient, id: string): unknown {
  const direct = cache.getQueryData(nodeKey(id));
  if (direct !== undefined) return direct;
  const thread = useThreadStore.getState().byThread.get(id)?.thread;
  if (thread !== null && thread !== undefined) return thread;
  for (const tree of useTreeStore.getState().byOrg.values()) {
    const row = tree.nodes.get(id);
    if (row !== undefined) return row;
  }
  return cache.getQueryData<Org[]>(orgsKey)?.find((org) => org.id === id);
}

/** Captured before authority-derived caches are dropped; it never fetches a page or guesses a grant. */
export function adminExitPlan(cache: QueryClient, href: string, lastOrdinary?: string): AdminExitPlan {
  const pathname = new URL(href, window.location.origin).pathname;
  const person = cache.getQueryData<Me>(meKey);
  const orgs = cache.getQueryData<Org[]>(orgsKey) ?? [];
  const own = orgs.find((org) => org.id === person?.personal_org_id);
  const fallback = lastOrdinary ?? (own === undefined ? '/settings/profile' : `/org/${encodeURIComponent(own.address)}`);
  const base: AdminExitPlan = { origin: href, person: person?.id ?? null, current: null, parents: [], fallback };
  if (/^\/admin(?:\/|$)/.test(pathname)) return { ...base, current: { href, access: 'no' } };
  const route = /^\/(org|project|topic|thread|doc)\/([^/]+)/.exec(pathname);
  const resource = /^\/resources\/(providers|agents|machines|hosts|environments|templates|mounts|mcp)\/([^/]+)/.exec(pathname);
  if (route === null && resource === null) return base;
  let id: string;
  try { id = decodeURIComponent(route?.[2] ?? resource?.[2] ?? ''); } catch { return base; }
  const kind = route?.[1];
  let value: unknown;
  if (kind === 'org') value = cache.getQueryData(orgBySlugKey(id)) ?? orgs.find((org) => org.id === id || org.address === id);
  else if (kind === 'doc') value = documentStateNow(id).document ?? cache.getQueryData(documentKey(id));
  else if (route !== null) value = cachedNode(cache, id);
  else {
    const names: Record<string, string> = { providers: 'provider', agents: 'agent', machines: 'machine', hosts: 'host', environments: 'environment', templates: 'template', mounts: 'mount', mcp: 'mcp-server', skills: 'skill' };
    value = cache.getQueryData([names[resource?.[1] ?? ''] ?? '', id]);
  }
  const actualId = record(value) && typeof value.id === 'string' ? value.id : id;
  const permission = kind === 'doc' ? 'doc.view' : kind === undefined ? undefined : `${kind}.view`;
  const access = ordinaryView(value, permission);
  // A resource list is an ordinary signed-in page; its rows are already cut to the viewer.
  if (resource !== null) return { ...base, current: { href, access }, parents: [{ href: `/resources/${resource[1] ?? ''}`, access: 'yes' }] };
  const heading = currentHeading();
  const sameHeading = kind === 'doc' ? heading?.target?.kind === 'document' && heading.target.id === actualId : heading?.nodeId === actualId;
  let ownPath = pathOf(value);
  if (ownPath.length === 0 && kind === 'doc' && record(value) && record(value.node) && typeof value.node.id === 'string') {
    const node = value.node;
    const nodeId = value.node.id;
    ownPath = pathOf(cachedNode(cache, nodeId));
    if (!ownPath.some((step) => step.id === nodeId) && nodeKind(node.kind)) ownPath.push({ id: nodeId, kind: node.kind,
      name: typeof node.name === 'string' ? node.name : null, redacted: node.redacted === true || typeof node.name !== 'string' });
  }
  if (ownPath.length === 0) for (const tree of useTreeStore.getState().byOrg.values()) {
    const chain = pathTo(tree, actualId);
    if (chain.length > 0) { ownPath = chain.map((node) => ({ id: node.id, kind: node.kind, name: node.name, redacted: false })); break; }
  }
  const path = ownPath.length > 0 ? ownPath : sameHeading ? heading?.path ?? [] : [];
  const subscreen = route !== null && pathname !== `/${kind ?? ''}/${encodeURIComponent(id)}`;
  const parents = [...path].reverse().filter((part) => part.id !== actualId || kind === 'doc' || subscreen).map((part): ExitPlace => ({
    id: part.id, kind: 'node', href: `/${part.kind}/${encodeURIComponent(part.id)}`, access: ordinaryView(cachedNode(cache, part.id), `${part.kind}.view`),
  }));
  return { ...base, current: { href, id: actualId, kind: kind === 'doc' ? 'doc' : kind === 'org' && !record(value) ? 'org' : 'node', access }, parents };
}

/** Unknown parents get an exact ordinary-session read; known native-only parents are skipped. */
export async function ordinaryDestination(plan: AdminExitPlan, probe: (place: ExitPlace) => Promise<boolean>): Promise<string | null> {
  const allowed = async (place: ExitPlace) => place.access === 'yes' || (place.access === 'unknown' && place.id !== undefined && await probe(place));
  if (plan.current === null || await allowed(plan.current)) return null;
  const tried = new Set<string>();
  for (const parent of plan.parents) {
    if (parent.id !== undefined && tried.has(parent.id)) continue;
    if (parent.id !== undefined) tried.add(parent.id);
    if (await allowed(parent)) return parent.href;
  }
  return plan.fallback;
}
