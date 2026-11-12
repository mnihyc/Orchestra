/** A cross-org document move carries the native complete deletion confirmation into one operation. */
import { accepted, api, repeatable } from '@/api/client';
import { traceJson } from '@/api/trace';
import { documentsPath, folderPath } from '@/features/documents/treeLocation';
import { linkTo } from '@/lib/links';
import type { DocumentDirectory, DocumentTreeDeletePreviewResponse, DocumentTreeItem, DocumentTreeOperationBody, DocumentTreeOperationResponse } from '@/generated/api-types';

// The published native variant arrives in generated aliases with the next contract carrier.
export type DocumentTreeOperationRequest = Omit<DocumentTreeOperationBody, 'command'> & { command: DocumentTreeOperationBody['command'] | 'move_out' };
export type DocumentMoveBody = Omit<DocumentTreeOperationBody, 'command'> & { command: 'move' | 'move_out' };
export function previewDocumentMoveDeletion(items: DocumentTreeItem[], clientId: string): Promise<DocumentTreeDeletePreviewResponse> {
  return accepted(api.POST('/document-tree/delete/preview', { body: { command: 'delete', items, to: null, name: null, client_id: clientId, version: null, deletion_plan: null } }));
}
export function executeDocumentMove(body: DocumentMoveBody): Promise<DocumentTreeOperationResponse> {
  return body.command === 'move'
    ? accepted(api.POST('/document-tree/operations', { body: { ...body, command: 'move' }, ...repeatable(body.client_id) }))
    : traceJson<DocumentTreeOperationResponse>('/document-tree/operations', { method: 'POST', body, key: body.client_id });
}
/** Only a full map or one unambiguous accepted identity names the copied root. */
export function documentMoveOpening(answer: DocumentTreeOperationResponse, source: DocumentTreeItem, destination: DocumentDirectory): string {
  const copied = 'projection' in answer ? answer.entries.length === 1 ? answer.entries[0] : undefined
    : answer.copied.find((pair) => pair.source.kind === source.kind && pair.source.id === source.id)?.destination;
  if (copied?.kind === source.kind) return copied.kind === 'container' ? folderPath(copied.id) : linkTo({ kind: 'document', id: copied.id });
  return destination.kind === 'container' ? folderPath(destination.id) : documentsPath(destination.id);
}
