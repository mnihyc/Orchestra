/** Build document breadcrumbs from typed parents; server arrays are sets and hidden names stay hidden. */
import type { DocumentDirectory, DocumentTreeAncestor } from '@/generated/api-types';

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
