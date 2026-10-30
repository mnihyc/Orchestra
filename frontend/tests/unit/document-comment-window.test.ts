/** The live store holds a fixed preview window even after a large reset or long feed. */
import { afterEach, expect, it } from 'vitest';
import type { DocumentDiscussion } from '@/api/documents';
import { DOCUMENT_DISCUSSION_WINDOW, documentStateNow, documentWriter } from '@/realtime/stores/document';
import { DORA } from '../doubles/chat-rows';

function discussion(at: number): DocumentDiscussion {
  return { id: `discussion-${at}`, document_id: 'doc', block_id: null, quote: '', anchored: false, resolved: null, created_by: DORA,
    created_at: '2026-01-01T00:00:00Z', seq_updated: at,
    comments: Array.from({ length: 75 }, (_, comment) => ({ id: `comment-${at}-${comment}`, discussion_id: `discussion-${at}`, author: DORA,
      created_at: '2026-01-01T00:00:00Z', content: [], reactions: [], edited_at: null, deleted: false, seq_updated: at })) };
}
afterEach(() => documentWriter.clear());

it('caps reset and live pages while retaining the newest comment versions', () => {
  documentWriter.reset('doc', { discussions: Array.from({ length: 1000 }, (_, i) => discussion(i)) }, 1000);
  expect(documentStateNow('doc').discussions.size).toBe(DOCUMENT_DISCUSSION_WINDOW);
  for (let page = 1; page <= 1000; page += 1) {
    documentWriter.apply('doc', { discussions: Array.from({ length: 50 }, (_, i) => discussion(1000+page*50+i)) }, 1000+page*50+49);
    expect(documentStateNow('doc').discussions.size).toBe(DOCUMENT_DISCUSSION_WINDOW);
    expect([...documentStateNow('doc').discussions.values()].every((row) => row.comments.length === 50)).toBe(true);
  }
  const held = documentStateNow('doc').discussions.get('discussion-51049');
  expect(held?.comments[0]?.id).toBe('comment-51049-25');
  documentWriter.apply('doc', { discussions: [discussion(51049)] }, 51049);
  expect(documentStateNow('doc').discussions.get('discussion-51049')).toBe(held);
});
