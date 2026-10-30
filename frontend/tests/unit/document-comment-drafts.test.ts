/** Device recovery is independent of comment publication and of presentation snapshots. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { UNSAVED_KEPT_MS } from '@/features/files/Editing';
import { inlineOf } from '@/features/documents/Comments';
import { beginCommentSend, commentDraftGuard, commentDraftKey, commentIntent, commentSlotKey, finishCommentSend,
  discardComment, forgetCommentDraftDeviceData, getCommentDrafts, hasCommentDraftLoss, markCommentConflict, retireCommentDraftSession,
  reviewCommentBase, saveCommentDrafts, selectCommentText, setCommentText, useCommentDraftGuard, type CommentTarget } from '@/features/documents/commentDrafts';
import { useWorkbench } from '@/features/workbench/store';

const first = { kind: 'new', block: 'block-a', quote: 'Exact selected words' } satisfies CommentTarget;
const reply: CommentTarget = { kind: 'reply', discussion: 'discussion-a' };
const edit: CommentTarget = { kind: 'edit', discussion: 'discussion-a', comment: 'comment-a' };
const session = client.session.getState();
let version = 100;
function reader(user = 'draft-person') { client.session.setState({ status: 'signed_in', userId: user, readerVersion: ++version }); }
beforeEach(() => {
  forgetCommentDraftDeviceData('draft-person', null); forgetCommentDraftDeviceData('someone-else', null);
  retireCommentDraftSession(); localStorage.clear(); reader();
  useWorkbench.getState().leave(); useWorkbench.getState().enter('draft-person', 'document');
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks(); forgetCommentDraftDeviceData('draft-person', null); forgetCommentDraftDeviceData('someone-else', null);
  retireCommentDraftSession(); useWorkbench.getState().leave(); localStorage.clear(); client.session.setState(session);
});

it('recovers distinct new/reply/edit identities, caret and compose intent from device data without publishing', () => {
  const draft = getCommentDrafts('draft-person', 'comments-view', 'doc-a');
  commentIntent(draft, { block: 'block-a', quote: 'Exact selected words' }, 'discussion-a');
  for (const [target, text] of [[first, 'New words'], [reply, 'Reply words'], [edit, 'Edited\nwords']] as const) setCommentText(draft, target, text);
  selectCommentText(draft, edit, { start: 1, end: 5, direction: 'backward' });
  expect(draft.store.getState().saved).toBe(true);
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'comments-view', 'doc-a').store.getState().data;
  expect(restored.compose).toEqual({ block: 'block-a', quote: 'Exact selected words' });
  expect(restored.focused).toBe('discussion-a');
  expect(Object.values(restored.slots).map((slot) => [slot.target, slot.text])).toEqual([[first, 'New words'], [reply, 'Reply words'], [edit, 'Edited\nwords']]);
  expect(restored.slots[commentSlotKey(edit)]?.selection).toEqual({ start: 1, end: 5, direction: 'backward' });
});

it('uses the same seven-day boundary on disk reload and same-session reopen, with no timer/poll', () => {
  const now = 1_800_000_000_000;
  const clock = vi.spyOn(Date, 'now').mockReturnValue(now);
  const draft = getCommentDrafts('draft-person', 'comments-view', 'doc-a');
  setCommentText(draft, reply, 'A week at most');
  clock.mockReturnValue(now + UNSAVED_KEPT_MS);
  expect(getCommentDrafts('draft-person', 'comments-view', 'doc-a').store.getState().data.slots[commentSlotKey(reply)]?.text).toBe('A week at most');
  clock.mockReturnValue(now + UNSAVED_KEPT_MS + 1);
  expect(getCommentDrafts('draft-person', 'comments-view', 'doc-a').store.getState().data.slots).toEqual({});
  expect(localStorage.getItem(draft.key)).toBeNull();
  retireCommentDraftSession();
  expect(getCommentDrafts('draft-person', 'comments-view', 'doc-a').store.getState().data.slots).toEqual({});
});

it('never restores another document, another view, another account, or a forged record identity', () => {
  const a = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(a, reply, 'Only this exact owner and target');
  expect(getCommentDrafts('draft-person', 'view', 'doc-b').store.getState().data.slots).toEqual({});
  expect(getCommentDrafts('draft-person', 'other-view', 'doc-a').store.getState().data.slots).toEqual({});
  const raw = localStorage.getItem(a.key);
  if (raw === null) throw new Error('the ordinary device write must exist');
  localStorage.setItem(commentDraftKey('draft-person', 'view', 'doc-b'), raw);
  retireCommentDraftSession();
  expect(getCommentDrafts('draft-person', 'view', 'doc-b').store.getState().data.slots).toEqual({});
  act(() => reader('someone-else'));
  expect(getCommentDrafts('someone-else', 'view', 'doc-a').store.getState().data.slots).toEqual({});
});

it.each([reply, edit])('preserves the exact client ID and mention payload for an unknown-outcome $kind retry after reopen', (target) => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, target, 'Ask @ada', [{ id: 'person-a', username: 'ada' }], target.kind === 'edit' ? 31 : undefined);
  const sent = beginCommentSend(draft, target, 'Ask @ada', [{ type: 'text', text: 'Ask ' }, { type: 'mention', user_id: 'person-a' }]);
  if (sent === null) throw new Error('the explicit first attempt must begin');
  finishCommentSend(draft, sent, false);
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'view', 'doc-a');
  const slot = restored.store.getState().data.slots[commentSlotKey(target)];
  if (slot === undefined) throw new Error('the restored request keeps its own selected mention binding');
  expect(slot.mentions).toEqual([{ id: 'person-a', username: 'ada' }]);
  const retry = beginCommentSend(restored, target, slot.text, inlineOf(slot.text.trim(), slot.mentions));
  expect(retry?.attempt.clientId).toBe(sent.attempt.clientId);
  expect(retry?.attempt.content).toEqual(sent.attempt.content);
  expect(retry?.attempt.content).not.toEqual([{ type: 'text', text: 'Ask @ada' }]);
  expect(retry?.attempt.expectedSeq).toBe(target.kind === 'edit' ? 31 : null);
});

it('retains the authored edit base through newer supplied rows, changed text and reload until explicit review selects a displayed revision', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, edit, 'First draft', [], 31);
  const sent = beginCommentSend(draft, edit, 'First draft', inlineOf('First draft'));
  if (sent === null) throw new Error('an explicitly authored edit has an original base');
  expect(sent.attempt.expectedSeq).toBe(31); finishCommentSend(draft, sent, false);
  setCommentText(draft, edit, 'Changed local words', [], 77);
  expect(draft.store.getState().data.slots[commentSlotKey(edit)]?.baseSeq).toBe(31);
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'view', 'doc-a');
  const retry = beginCommentSend(restored, edit, 'Changed local words', inlineOf('Changed local words'));
  if (retry === null) throw new Error('restored edits retain their original base');
  expect(retry.attempt.expectedSeq).toBe(31);
  expect(retry.attempt.clientId).not.toBe(sent.attempt.clientId);
  finishCommentSend(restored, retry, false);
  expect(reviewCommentBase(restored, { kind: 'edit', discussion: 'discussion-a', comment: 'comment-a' }, 77)).toBe(true);
  const reviewed = beginCommentSend(restored, edit, 'Changed local words', inlineOf('Changed local words'));
  expect(reviewed?.attempt.expectedSeq).toBe(77);
  expect(reviewed?.attempt.clientId).not.toBe(retry.attempt.clientId);
  expect(restored.store.getState().data.slots[commentSlotKey(edit)]?.text).toBe('Changed local words');
});

it('an old recovered edit without a base remains local until an explicit review, rather than adopting the latest row', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, edit, 'Legacy local words', [], 31);
  const data = draft.store.getState().data, slot = data.slots[commentSlotKey(edit)];
  if (slot === undefined) throw new Error('the authored local edit exists');
  const { baseSeq: _oldBase, ...legacy } = slot;
  localStorage.setItem(draft.key, JSON.stringify({ ...data, slots: { [commentSlotKey(edit)]: legacy } }));
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'view', 'doc-a');
  expect(restored.store.getState().data.slots[commentSlotKey(edit)]?.text).toBe('Legacy local words');
  expect(beginCommentSend(restored, edit, 'Legacy local words', inlineOf('Legacy local words'))).toBeNull();
  expect(reviewCommentBase(restored, { kind: 'edit', discussion: 'discussion-a', comment: 'comment-a' }, 77)).toBe(true);
  expect(beginCommentSend(restored, edit, 'Legacy local words', inlineOf('Legacy local words'))?.attempt.expectedSeq).toBe(77);
});

it('a native stale refusal remains a local review requirement across reload without retaining its rich response or changing the base', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, edit, 'Keep these words', [], 31);
  const sent = beginCommentSend(draft, edit, 'Keep these words', inlineOf('Keep these words'));
  if (sent === null) throw new Error('the original modern edit must issue');
  finishCommentSend(draft, sent, false); markCommentConflict(draft, sent, 77);
  expect(draft.store.getState().data.slots[commentSlotKey(edit)]?.baseSeq).toBe(31);
  expect(draft.store.getState().data.slots[commentSlotKey(edit)]?.attempt?.clientId).toBe(sent.attempt.clientId);
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'view', 'doc-a');
  expect(restored.store.getState().data.slots[commentSlotKey(edit)]?.conflictSeq).toBe(77);
  expect(beginCommentSend(restored, edit, 'Keep these words', inlineOf('Keep these words'))).toBeNull();
  expect(localStorage.getItem(restored.key)).not.toContain('current_comment');
  expect(reviewCommentBase(restored, { kind: 'edit', discussion: 'discussion-a', comment: 'comment-a' }, 77)).toBe(true);
  const reviewed = beginCommentSend(restored, edit, 'Keep these words', inlineOf('Keep these words'));
  expect(reviewed?.attempt.clientId).not.toBe(sent.attempt.clientId);
  expect(reviewed?.attempt.expectedSeq).toBe(77);
});

it('keeps the original encoded-attempt identity through temporary edits away and back, including mention casing', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  const mentions = [{ id: 'person-a', username: 'ada' }];
  setCommentText(draft, reply, 'Ask @ada', mentions);
  const sent = beginCommentSend(draft, reply, 'Ask @ada', inlineOf('Ask @ada', mentions));
  if (sent === null) throw new Error('the original explicit request must begin');
  finishCommentSend(draft, sent, false);
  setCommentText(draft, reply, 'Something else', mentions);
  setCommentText(draft, reply, 'Ask @ADA', mentions);
  const retried = beginCommentSend(draft, reply, 'Ask @ADA', inlineOf('Ask @ADA', mentions));
  expect(retried?.attempt.clientId).toBe(sent.attempt.clientId);
  expect(retried?.attempt.content).toEqual(sent.attempt.content);
});

it('keeps addresses and unknown names as text while encoding later selected people', () => {
  const people = [{ id: 'person-a', username: 'ada' }, { id: 'person-b', username: 'bea' }];
  expect(inlineOf('Mail ada@example.org or ada@ada, then @unknown, @ada and @BEA.', people)).toEqual([
    { type: 'text', text: 'Mail ada@example.org or ada@ada, then @unknown, ' },
    { type: 'mention', user_id: 'person-a' },
    { type: 'text', text: ' and ' },
    { type: 'mention', user_id: 'person-b' },
    { type: 'text', text: '.' },
  ]);
});

it('does not let a late successful request erase words typed during its pending interval', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, reply, 'First request');
  const sent = beginCommentSend(draft, reply, 'First request', [{ type: 'text', text: 'First request' }]);
  if (sent === null) throw new Error('the first explicit attempt must begin');
  setCommentText(draft, reply, 'A later draft');
  expect(finishCommentSend(draft, sent, true)).toBe(false);
  expect(draft.store.getState().data.slots[commentSlotKey(reply)]?.text).toBe('A later draft');
  const later = beginCommentSend(draft, reply, 'A later draft', [{ type: 'text', text: 'A later draft' }]);
  expect(later?.attempt.clientId).not.toBe(sent.attempt.clientId);
});

it('fences reader ABA and old controller callbacks without erasing the new reader’s data', () => {
  const old = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(old, reply, 'Old reader');
  const sent = beginCommentSend(old, reply, 'Old reader', [{ type: 'text', text: 'Old reader' }]);
  if (sent === null) throw new Error('the old reader starts an explicit attempt');
  act(() => { reader('someone-else'); reader('draft-person'); });
  const current = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(current, reply, 'New reader');
  expect(finishCommentSend(old, sent, true)).toBe(false);
  setCommentText(old, reply, 'Retired callback');
  expect(current.store.getState().data.slots[commentSlotKey(reply)]?.text).toBe('New reader');
});

it('keeps failed device writes recoverable only by the same person after forced session retirement', () => {
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  const old = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(old, first, 'Only remaining memory copy');
  expect(old.store.getState().saved).toBe(false);
  act(() => reader('someone-else'));
  expect(old.store.getState().data.slots).toEqual({});
  expect(getCommentDrafts('someone-else', 'view', 'doc-a').store.getState().data.slots).toEqual({});
  act(() => reader('draft-person'));
  const back = getCommentDrafts('draft-person', 'view', 'doc-a');
  expect(back.store.getState().data.slots[commentSlotKey(first)]?.text).toBe('Only remaining memory copy');
  expect(back.store.getState().saved).toBe(false);
  write.mockRestore();
  expect(saveCommentDrafts(back)).toBe(true);
});

it('aggregates all affected documents/replies/edits in one view guard, and local Save never consumes them', async () => {
  const view = useWorkbench.getState().openTab('comments', undefined, 'new');
  const a = getCommentDrafts('draft-person', view, 'doc-a'), b = getCommentDrafts('draft-person', view, 'doc-b');
  renderHook(() => { useCommentDraftGuard(view, a); useCommentDraftGuard(view, b); });
  const write = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  act(() => { setCommentText(a, reply, 'First reply'); setCommentText(a, edit, 'Second edit'); setCommentText(b, first, 'Third target'); });
  expect(hasCommentDraftLoss(view)).toBe(true);
  expect(useWorkbench.getState().unsaved.has(view)).toBe(true);
  expect(await commentDraftGuard(view).save()).toBe(false);
  expect(Object.keys(a.store.getState().data.slots)).toHaveLength(2);
  expect(Object.keys(b.store.getState().data.slots)).toHaveLength(1);
  write.mockRestore();
  await act(async () => { expect(await commentDraftGuard(view).save()).toBe(true); });
  expect(useWorkbench.getState().unsaved.has(view)).toBe(false);
  expect(a.store.getState().data.slots[commentSlotKey(reply)]?.text).toBe('First reply');
  expect(a.store.getState().data.slots[commentSlotKey(edit)]?.text).toBe('Second edit');
  expect(b.store.getState().data.slots[commentSlotKey(first)]?.text).toBe('Third target');
});

it('device deletion retires mounted drafts and in-flight acknowledgements without touching another target', () => {
  const a = getCommentDrafts('draft-person', 'view', 'doc-a'), b = getCommentDrafts('draft-person', 'view', 'doc-b');
  setCommentText(a, reply, 'Forget this'); setCommentText(b, reply, 'Keep this');
  const sent = beginCommentSend(a, reply, 'Forget this', [{ type: 'text', text: 'Forget this' }]);
  if (sent === null) throw new Error('the explicit request begins before device data is forgotten');
  localStorage.removeItem(a.key); forgetCommentDraftDeviceData('draft-person', [a.key]);
  expect(a.store.getState().data.slots).toEqual({});
  expect(finishCommentSend(a, sent, true)).toBe(false);
  expect(localStorage.getItem(a.key)).toBeNull();
  expect(b.store.getState().data.slots[commentSlotKey(reply)]?.text).toBe('Keep this');
});

it('a refused local discard keeps its exact body reachable until erasing succeeds', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(draft, edit, 'Keep the only working copy');
  const remove = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => { throw new DOMException('blocked', 'SecurityError'); });
  expect(discardComment(draft, edit)).toBe(false);
  expect(draft.store.getState().data.slots[commentSlotKey(edit)]?.text).toBe('Keep the only working copy');
  remove.mockRestore();
  expect(discardComment(draft, edit)).toBe(true);
  expect(draft.store.getState().data.slots).toEqual({});
});

it('a known successful send whose local erase fails is retired by local Save without another publish attempt', () => {
  const draft = getCommentDrafts('draft-person', 'view', 'doc-a');
  commentIntent(draft, { block: first.block, quote: first.quote }, null);
  setCommentText(draft, first, 'The native mutation succeeded');
  const content = inlineOf('The native mutation succeeded');
  const sent = beginCommentSend(draft, first, 'The native mutation succeeded', content);
  if (sent === null) throw new Error('the explicit request must have issued');
  const remove = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => { throw new DOMException('blocked', 'SecurityError'); });
  expect(finishCommentSend(draft, sent, true)).toBe(false);
  expect(draft.store.getState().saved).toBe(false);
  expect(draft.store.getState().data.slots[commentSlotKey(first)]?.text).toBe('The native mutation succeeded');
  expect(beginCommentSend(draft, first, 'The native mutation succeeded', content)).toBeNull();
  expect(localStorage.getItem(draft.key)).not.toBeNull();
  retireCommentDraftSession();
  const restored = getCommentDrafts('draft-person', 'view', 'doc-a');
  expect(restored.store.getState().data.slots[commentSlotKey(first)]?.text).toBe('The native mutation succeeded');
  expect(restored.store.getState().saved).toBe(false);
  expect(beginCommentSend(restored, first, 'The native mutation succeeded', content)).toBeNull();
  remove.mockRestore();
  expect(saveCommentDrafts(restored)).toBe(true);
  expect(localStorage.getItem(draft.key)).toBeNull();
  expect(restored.store.getState().data.slots).toEqual({});
  expect(restored.store.getState().data.compose).toBeNull();
});

it('cross-window device removal clears retired failed memory even while a different person is signed in', () => {
  const fail = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  const a = getCommentDrafts('draft-person', 'view', 'doc-a');
  setCommentText(a, reply, 'Do not resurrect after device removal');
  act(() => reader('someone-else'));
  window.dispatchEvent(new StorageEvent('storage', { key: a.key, newValue: null }));
  fail.mockRestore();
  act(() => reader('draft-person'));
  expect(getCommentDrafts('draft-person', 'view', 'doc-a').store.getState().data.slots).toEqual({});
});

it('keeps comment bodies out of every presentation-only workspace snapshot', () => {
  const view = useWorkbench.getState().openTab('comments', undefined, 'new');
  const draft = getCommentDrafts('draft-person', view, 'doc-a');
  setCommentText(draft, first, 'This body belongs only in its device recovery record');
  useWorkbench.getState().flush();
  for (let at = 0; at < localStorage.length; at += 1) {
    const key = localStorage.key(at);
    if (key === null || key === draft.key) continue;
    expect(localStorage.getItem(key)).not.toContain('This body belongs only in its device recovery record');
  }
});
