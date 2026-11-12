/** A successful schedule clears only its exact current-reader snapshot; later edits remain drafts. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Threads from '@/api/threads';
import { composerWriter, useComposerStore } from '@/features/composer/Draft';
import type { ContentPart } from '@/generated/api-types';
const calls = vi.hoisted(() => ({ save: vi.fn<typeof Threads.saveDraft>() }));
vi.mock('@/api/threads', async (original) => ({ ...await original<typeof Threads>(), saveDraft: calls.save }));
const parts: ContentPart[] = [{ type: 'file', name: 'kept.txt', path: '.orch/attachments/kept.txt', blob: 'b'.repeat(64), size: 3 }];
beforeEach(() => {
  vi.clearAllMocks(); useComposerStore.setState({ text: { thread: 'Accepted words' }, parts: { thread: parts }, nativeSelection: { thread: { action_id: 'native', revision: 'catalogue', arguments: {} } } });
  calls.save.mockResolvedValue({ content: [], native_action: null, review_comments: [], delivery: 'queue', updated_at: '2030-01-01T00:00:00Z' });
});
it('clears accepted words, parts and local native selection through the serialized draft writer', async () => {
  expect(composerWriter.clearScheduledDraft('thread', 'Accepted words', parts, () => true)).toBe(true);
  expect(useComposerStore.getState().text.thread).toBe(''); expect(useComposerStore.getState().parts.thread).toEqual([]); expect(useComposerStore.getState().nativeSelection.thread).toBeNull();
  await vi.waitFor(() => expect(calls.save).toHaveBeenCalledWith('thread', [], 'queue', null));
});
it('keeps text typed after the captured snapshot', () => {
  composerWriter.setText('thread', 'Newer words');
  expect(composerWriter.clearScheduledDraft('thread', 'Accepted words', parts, () => true)).toBe(false);
  expect(useComposerStore.getState().text.thread).toBe('Newer words'); expect(calls.save).not.toHaveBeenCalled();
});
it('keeps attachments added after the captured snapshot', () => {
  composerWriter.addPart('thread', { type: 'text', text: 'A newer part' });
  expect(composerWriter.clearScheduledDraft('thread', 'Accepted words', parts, () => true)).toBe(false);
  expect(useComposerStore.getState().parts.thread).toHaveLength(2); expect(calls.save).not.toHaveBeenCalled();
});
it('a replacement reader cannot clear the old snapshot', () => {
  expect(composerWriter.clearScheduledDraft('thread', 'Accepted words', parts, () => false)).toBe(false);
  expect(useComposerStore.getState().text.thread).toBe('Accepted words'); expect(calls.save).not.toHaveBeenCalled();
});
