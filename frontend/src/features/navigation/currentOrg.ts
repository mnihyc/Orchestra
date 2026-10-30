/** The global tree stays in its chosen org while a page's own navigation stays inside that page. */
import { useQuery } from '@tanstack/react-query';
import { useRouterState } from '@tanstack/react-router';
import { nodeQuery, orgBySlugQuery, orgsQuery } from '@/api/nodes';
import type { NodeRef, Org } from '@/generated/api-types';
import { useNodePath, useTreeStore } from '@/realtime/stores/tree';
import { useThreadRow } from '@/realtime/stores/thread';
import { useSelectedOrg } from '@/shell/org';
import { useCurrentHeading } from '@/shell/screen';

export function navigationRoot(pathname: string, orgs: readonly Org[], selected: string | null, known: { thread?: string | null; node?: string | null; org?: string | null; heading?: string | null; retained?: string | null }): string | null {
  const chosen = orgs.some((org) => org.id === selected || org.org_id === selected) ? selected : null;
  const match = /^\/(org|project|topic|thread|doc)\/([^/]+)/.exec(pathname);
  if (match === null) return chosen;
  let address: string;
  try { address = decodeURIComponent(match[2] ?? ''); } catch { return null; }
  if (match[1] === 'org') return orgs.find((org) => org.id === address || org.address === address)?.org_id ?? known.org ?? known.heading ?? known.retained ?? null;
  if (match[1] === 'thread') return known.thread ?? known.heading ?? known.retained ?? null;
  return known.node ?? known.heading ?? known.retained ?? null;
}

/** An identity in a redacted breadcrumb does not grant a browsable organization. */
export function readableNavigationRoot(path: readonly NodeRef[] | undefined, fallback: string | null = null): string | null {
  const first = path?.[0];
  return first === undefined ? fallback : first.redacted ? null : first.id;
}

/** Every query here is cache-only: the screen owns its data read and its permission check. */
export function useNavigationOrg(): string | null {
  const location = useRouterState({ select: (state) => state.location });
  const pathname = location.pathname;
  const selected = useSelectedOrg((state) => state.orgId);
  const retained = useTreeStore((state) => selected !== null && state.byOrg.get(selected)?.loaded === true ? selected : null);
  const heading = useCurrentHeading();
  const match = /^\/(org|project|topic|thread|doc)\/([^/]+)/.exec(pathname);
  const kind = match?.[1];
  let id = '';
  try { id = decodeURIComponent(match?.[2] ?? ''); } catch { /* A malformed address has no navigation target. */ }
  const headingMatches = kind === 'doc' ? heading?.target?.kind === 'document' && heading.target.id === id : heading?.nodeId === id;
  const nodeId = kind === 'project' || kind === 'topic' ? id : headingMatches ? heading?.nodeId ?? '' : '';
  const node = useQuery({ ...nodeQuery(nodeId), enabled: false }).data;
  const org = useQuery({ ...orgBySlugQuery(kind === 'org' ? id : ''), enabled: false }).data;
  const orgs = useQuery({ ...orgsQuery, enabled: false }).data ?? [];
  const thread = useThreadRow(kind === 'thread' ? id : '');
  const path = useNodePath(headingMatches ? heading?.nodeId : undefined);
  return navigationRoot(pathname, orgs, selected, {
    thread: readableNavigationRoot(thread?.path), node: readableNavigationRoot(node?.path), org: readableNavigationRoot(org?.path), retained,
    heading: headingMatches ? readableNavigationRoot(heading?.path, path.orgId) : null,
  });
}
