/** What a thing with a page is called and where it is: the address of its screen, for a link that is passed on.
 *
 * The permission page is opened for a node, a document, a chat or one of the seven kinds of resource.
 * Each has a screen of its own at an address; a chat has none, so there is no link to copy for it.
 */
import { type AccessKind, type AccessTarget } from '@/api/access';
import { placePath } from '@/features/navigation/places';
import { folderPath } from '@/features/documents/treeLocation';

/** Where the screen of a resource lies, by kind. */
const RESOURCE_PATHS: Partial<Record<AccessKind, string>> = {
  provider: '/resources/providers',
  agent: '/resources/agents',
  environment: '/resources/environments',
  host: '/resources/hosts',
  template: '/resources/templates',
  mcp_server: '/resources/mcp',
  mount: '/resources/mounts',
  skill: '/resources/skills',
};

/** The path of a thing's screen inside the application; `null` for what has none. */
export function pathFor(target: Pick<AccessTarget, 'kind' | 'id'>): string | null {
  switch (target.kind) {
    case 'org':
    case 'project':
    case 'topic':
    case 'thread':
      return placePath({ id: target.id, kind: target.kind });
    case 'document':
      return `/doc/${target.id}`;
    case 'document_container':
      return folderPath(target.id);
    case 'chat':
      return null;
    default: {
      const base = RESOURCE_PATHS[target.kind];
      return base === undefined ? null : `${base}/${target.id}`;
    }
  }
}

/** The full address of a thing's screen, for a link that is handed on. */
export function linkOf(target: Pick<AccessTarget, 'kind' | 'id'>): string | null {
  const path = pathFor(target);
  return path === null ? null : new URL(path, location.origin).href;
}
