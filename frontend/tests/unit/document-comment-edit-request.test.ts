/** The real wrapper transmits paired edit identity/CAS fields; this proves no native mutation invariant. */
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { editComment, staleEditedComment } from '@/api/documents';
import type { DocumentComment, Inline } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';

const current: DocumentComment = { id: 'synthetic-comment', discussion_id: 'synthetic-discussion', author: userRef(me(1, 'ada', 'Ada')),
  content: [{ type: 'text', text: 'Current words' }], reactions: [], edited_at: null, deleted: false,
  created_at: '2026-10-17T10:00:00Z', seq_updated: 41 };
afterEach(() => { vi.unstubAllGlobals(); });

it('sends the unchanged paired attempt twice and keeps a legacy content-only caller explicit', async () => {
  const seen: Request[] = [];
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit) => {
    seen.push(input instanceof Request ? input : new Request(input, init));
    return Promise.resolve(Response.json(current));
  });
  const content: Inline[] = [{ type: 'text', text: 'My words' }];
  const attempt = { clientId: '01950000-0000-7000-8001-000000000031', expectedSeq: 31 };
  await editComment('doc-a', current.id, content, attempt);
  await editComment('doc-a', current.id, content, attempt);
  await editComment('doc-a', current.id, content);
  expect(seen).toHaveLength(3);
  const path = 'PATCH /api/v1/documents/doc-a/comments/synthetic-comment';
  expect(seen.map((request) => `${request.method} ${new URL(request.url).pathname}`)).toEqual([path, path, path]);
  const bodies: unknown[] = [];
  for (const request of seen) { const body: unknown = await request.json(); bodies.push(body); }
  expect(bodies).toEqual([{ content, client_id: attempt.clientId, expected_seq: 31 }, { content, client_id: attempt.clientId, expected_seq: 31 }, { content }]);
  expect(seen.every((request) => request.headers.get('Idempotency-Key') === null)).toBe(true);
});

it('recognizes only the current-comment stale cause and leaves other problems alone', () => {
  const problem = { type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision' as const,
    title: 'Changed', detail: 'The comment changed.', status: 409, request_id: '' };
  expect(staleEditedComment(new ApiError({ ...problem, current_comment: current }, null))).toEqual(current);
  expect(staleEditedComment(new ApiError({ ...problem, current: [] }, null))).toBeNull();
  expect(staleEditedComment(new ApiError({ ...problem, current_comment: { id: current.id } }, null))).toBeNull();
});
