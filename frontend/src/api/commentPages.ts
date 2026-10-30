/** One replaceable window of comments or discussions; old pages leave the cache immediately. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { DocumentComment, DocumentDiscussion } from '@/api/documents';

interface Page<T> { items: T[]; next_cursor: string | null }
export interface CommentPage extends Page<DocumentComment> { wrote: boolean }
interface Answer<T> { data?: T; error?: unknown; response: Response }
type CommentRead = (path: '/documents/{id}/discussions/{discussion_id}/comments', options: {
  params: { path: { id: string; discussion_id: string }; query: { limit: number; cursor?: string } }; signal: AbortSignal;
}) => Promise<Answer<CommentPage>>;

export const commentPageQuery = (document: string, discussion: string, cursor: string | null, seq: number) => queryOptions({
  queryKey: ['document-comment-page', document, discussion, cursor, seq],
  queryFn: ({ signal }): Promise<CommentPage> => ok((api.GET as unknown as CommentRead)('/documents/{id}/discussions/{discussion_id}/comments', {
    params: { path: { id: document, discussion_id: discussion }, query: { limit: 50, ...(cursor === null ? {} : { cursor }) } }, signal,
  })),
  gcTime: 0,
});

export const discussionPageQuery = (document: string, show: 'open' | 'resolved' | 'all', cursor: string | null, seq: number) => queryOptions({
  queryKey: ['document-discussion-page', document, show, cursor, seq],
  queryFn: ({ signal }): Promise<Page<DocumentDiscussion>> => ok(api.GET('/documents/{id}/discussions', {
    params: { path: { id: document }, query: { limit: 50, resolved: show === 'all' ? 'include' : show === 'resolved' ? 'only' : 'exclude', ...(cursor === null ? {} : { cursor }) } }, signal,
  })),
  gcTime: 0,
});
