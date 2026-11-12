/** Build document breadcrumbs from typed parents; server arrays are sets and hidden names stay hidden. */
import type { DocumentDirectory, DocumentTreeAncestor, DocumentTreeEntry } from '@/generated/api-types';

export const documentDirectoryIdentity = (at: DocumentDirectory): string => `${at.kind}:${at.id}`;
export const folderPath = (id: string, retained = false): string => retained ? `/inspect/document_container/${encodeURIComponent(id)}` : `/document-folder/${encodeURIComponent(id)}`;
export const documentsPath = (id: string): string => `/documents/${encodeURIComponent(id)}`;

/** Selected folders are included once by the caller; this returns only the ancestors of its parent. */
export function documentAncestors(parent: DocumentDirectory, ancestors: readonly DocumentTreeAncestor[]): DocumentTreeAncestor[] {
  const byId = new Map(ancestors.map((ancestor) => [ancestor.id, ancestor]));
  const seen = new Set<string>();
  const path: DocumentTreeAncestor[] = [];
  let at = parent;
  while (at.kind === 'container' && !seen.has(at.id)) {
    seen.add(at.id);
    const ancestor = byId.get(at.id);
    if (ancestor === undefined) break;
    path.push(ancestor);
    at = ancestor.parent;
  }
  return path.reverse();
}

/** All preview entries are authorized. Resolve their selected-tree paths without hidden ancestors. */
export function deletionPaths(entries: readonly DocumentTreeEntry[]): Map<string, string> {
  const folders = new Map(entries.filter((entry) => entry.item.kind === 'container').map((entry) => [entry.item.id, entry]));
  const result = new Map<string, string>();
  for (const entry of entries) {
    const parts = [entry.basename];
    const seen = new Set([`${entry.item.kind}:${entry.item.id}`]);
    let parent = entry.parent;
    while (parent.kind === 'container') {
      const ancestor = folders.get(parent.id);
      if (ancestor === undefined || seen.has(`container:${parent.id}`)) break;
      seen.add(`container:${parent.id}`); parts.unshift(ancestor.basename); parent = ancestor.parent;
    }
    result.set(`${entry.item.kind}:${entry.item.id}`, parts.join('/'));
  }
  return result;
}
