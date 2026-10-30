/** Current readers, deliberate read positions, document copies and imports use the generated authenticated API. */
import { queryOptions } from '@tanstack/react-query';
import { api, done, ok } from '@/api/client';
import type { ChatPart, UploadedBlob, UserRef, NodeDocumentCopyBody as CopyBody, DocumentDiscussionLevel as DiscussionLevel, DocumentImportRequest as ImportBody } from '@/generated/api-types';
import type { GuardedOpsBody } from '@/features/people/pending';

export interface DocumentMentionCandidate { user: UserRef; can_view: boolean }
export interface DocumentMentionPage { items: DocumentMentionCandidate[]; next_cursor: string | null }
interface MentionReply { data?: DocumentMentionPage; error?: unknown; response: Response }
type MentionRead = (path: '/documents/{id}/mentions', options: { params: { path: { id: string }; query: { q: string; cursor?: string } }; signal?: AbortSignal }) => Promise<MentionReply>;
export const documentReadersQuery = (id: string, q = '', cursor: string | null = null) => queryOptions({
  queryKey: ['document-readers', id, q, cursor], retry: false, staleTime: 0, gcTime: 0,
  queryFn: ({ signal }): Promise<DocumentMentionPage> => ok((api.GET as unknown as MentionRead)('/documents/{id}/mentions', { params: { path: { id }, query: { q, ...(cursor === null ? {} : { cursor }) } }, signal })),
});
export const mentionChoicesQuery = (id: string, q = '') => queryOptions({
  queryKey: ['chat-mentions', id, q], retry: false, staleTime: 0,
  queryFn: ({ signal }) => ok(api.GET('/chats/{id}/mentions', { params: { path: { id }, query: { q } }, signal })),
});
export const mentionAudienceQuery = (id: string, parts: ChatPart[], replyTo: string | null = null) => {
  return queryOptions({ queryKey: ['chat-mention-audience', id, parts, replyTo], retry: false, staleTime: 0,
    queryFn: () => ok(api.POST('/chats/{id}/mentions/preview', { params: { path: { id } }, body: { parts, ...(replyTo === null ? {} : { reply_to: replyTo }) } })),
  });
};
export const setDiscussionLevel = (id: string, level: DiscussionLevel) => done(api.PUT('/documents/{id}/subscription', { params: { path: { id } }, body: { level } }));
export const setReadPosition = (id: string, seq: number, read_revision: number, unread = false) => done(api.POST('/chats/{id}/read', { params: { path: { id } }, body: { seq, read_revision, unread_from: unread } }));
export const setAgentPosting = (id: string, thread_id: string, stopped: boolean) => done(api.PUT('/chats/{id}/agents/{thread_id}/posting', { params: { path: { id, thread_id } }, body: { stopped } }));
export const putBackBlocks = (id: string, body: GuardedOpsBody) => ok(api.POST('/documents/{id}/ops', { params: { path: { id } }, body }));
export const copyDocument = (id: string, body: CopyBody) => ok(api.POST('/nodes/{id}/documents/copy', { params: { path: { id } }, body }));
export const importDocuments = (id: string, body: ImportBody) => ok(api.POST('/nodes/{id}/documents/import', { params: { path: { id } }, body }));

/** The ordinary upload-session protocol owns chunk size, status and progress. */
export async function uploadImport(file: File, progress: (received: number) => void): Promise<UploadedBlob> {
  const session = await ok(api.POST('/uploads/sessions', { body: { name: file.name, size: file.size, media_type: file.type || 'application/octet-stream' } }));
  let offset = 0;
  while (offset < file.size) {
    const piece = file.slice(offset, offset + session.chunk_bytes);
    const reply = await ok(api.PUT('/uploads/sessions/{upload_id}', { params: { path: { upload_id: session.upload_id }, query: { offset } }, body: '', bodySerializer: () => piece, headers: { 'Content-Type': 'application/octet-stream' } }));
    if (reply.received <= offset) throw new Error('Upload did not advance');
    offset = reply.received; progress(offset);
  }
  return ok(api.POST('/uploads/sessions/{upload_id}/complete', { params: { path: { upload_id: session.upload_id } } }));
}
