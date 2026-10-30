/** Merge only currently projected placement and document standing; content remains owned by its document feed. */
import { ApiError } from '@/api/client';
import type { Document, DocumentTreeRead } from '@/generated/api-types';

export function documentMetadataRefused(error: unknown): boolean {
  return error instanceof ApiError && (error.is('authz.denied') || error.is('not_found'));
}

export function documentWithPlacement(doc: Document | null, metadata: DocumentTreeRead | undefined): Document | null {
  if (doc === null || metadata === undefined) return doc;
  const row = metadata.entries.find((entry) => entry.entry.item.kind === 'document' && entry.entry.item.id === doc.id);
  if (row === undefined) return doc;
  // A late cut from an older place cannot undo the newer content reference's move.
  if (row.entry.node !== doc.node.id && row.entry.revision < doc.seq) return doc;
  const node = metadata.place?.node ?? row.document?.node ?? doc.node;
  const path = metadata.place?.path ?? (doc.path.at(-1)?.id === node.id ? doc.path : []);
  const permissions = new Set(doc.access.permissions);
  for (const [permission, allowed] of [['doc.view', row.standing.read], ['doc.edit', row.standing.write], ['doc.manage', row.standing.manage]] as const) {
    if (allowed) permissions.add(permission); else permissions.delete(permission);
  }
  return { ...doc, node, path,
    location: { parent: row.entry.parent, basename: row.entry.basename, ancestors: metadata.ancestors },
    access: { ...doc.access, permissions: [...permissions] } };
}
