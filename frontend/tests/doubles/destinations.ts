/** The fake API's one web-address builder, mirroring the shared Names/Everything producer. */
import type { NodeRef } from '../../src/generated/api-types.ts';

export function webHref({ kind, id, nodeId, orgId, path = [], username }: {
  kind: string; id: string; nodeId?: string | null; orgId?: string | null;
  path?: readonly NodeRef[]; username?: string | null;
}): string | null {
  switch (kind) {
    case 'org': case 'project': case 'topic': case 'thread': return `/${kind}/${id}`;
    case 'document': return `/doc/${id}`;
    case 'chat': {
      const node = path.at(-1);
      return node !== undefined && node.id === nodeId ? `/${node.kind}/${node.id}?panel=chat&chat=${id}`
        : orgId === undefined || orgId === null ? null : `/org/${orgId}/messages?chat=${id}`;
    }
    case 'tag': {
      const project = path.at(-1);
      return project?.kind === 'project' && !project.redacted && project.id === nodeId
        ? `/project/${project.id}?${new URLSearchParams({ view: 'list', tag: JSON.stringify([id]) }).toString()}` : null;
    }
    case 'user': return username === undefined || username === null ? null : `/u/${encodeURIComponent(username)}`;
    case 'environment': return `/resources/environments/${id}`;
    case 'provider': return `/resources/providers/${id}`;
    case 'agent': return `/resources/agents/${id}`;
    case 'host': return `/resources/hosts/${id}`;
    case 'template': return `/resources/templates/${id}`;
    case 'machine': return `/resources/machines/${id}`;
    case 'mcp_server': return `/resources/mcp/${id}`;
    case 'mount': return `/resources/mounts/${id}`;
    case 'skill': return `/resources/skills/${id}`;
    default: return null;
  }
}
