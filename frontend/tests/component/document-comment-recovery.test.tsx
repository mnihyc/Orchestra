/** Actual comment composers recover locally; close-plan Save never publishes their contents. */
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { createRef, useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as People from '@/api/people';
import type * as NoticeJobs from '@/api/documentNoticeJobs';
import { ApiError, client } from '@/api/client';
import { CommentsPanel } from '@/features/documents/Comments';
import { commentDraftGuard, commentSlotKey, forgetCommentDraftDeviceData, getCommentDrafts, retireCommentDraftSession, setCommentText } from '@/features/documents/commentDrafts';
import { DocumentMarksProvider, retireDocumentMarks, useDocumentMarksStore, type Marks } from '@/features/documents/marks';
import { CloseViews, requestViewAction } from '@/features/workbench/CloseViews';
import { ViewInstanceContext } from '@/features/workbench/instance';
import { findTab } from '@/features/workbench/layout';
import { setPanelScope } from '@/features/workbench/panelScope';
import { useUnsavedGuard, useWorkbench } from '@/features/workbench/store';
import type { Document, DocumentBlock, DocumentComment, DocumentDiscussion } from '@/generated/api-types';
import type { StoreApi } from 'zustand';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { documentStateNow, documentWriter, useDocumentState } from '@/realtime/stores/document';
import { World } from '../doubles/world';
import { show } from './support';

const calls = vi.hoisted(() => ({ start: vi.fn<typeof Documents.startDiscussion>(), reply: vi.fn<typeof Documents.answerDiscussion>(),
  edit: vi.fn<typeof Documents.editComment>(), readers: vi.fn<() => Promise<People.DocumentMentionPage>>() }));
vi.mock('@/api/documents', async (load) => ({ ...await load<typeof Documents>(), startDiscussion: calls.start, answerDiscussion: calls.reply, editComment: calls.edit }));
vi.mock('@/api/people', async (load) => {
  const actual = await load<typeof People>();
  return { ...actual, documentReadersQuery: (document: string, query: string) => ({ ...actual.documentReadersQuery(document, query), queryFn: calls.readers }) };
});

// These composer-recovery fixtures have no queued notification work.
vi.mock('@/api/documentNoticeJobs', async (load) => {
  const actual = await load<typeof NoticeJobs>();
  return { ...actual, documentNoticeJobsQuery: (id: string, reader = 0) => ({ ...actual.documentNoticeJobsQuery(id, reader), queryFn: () => Promise.resolve({ items: [], next_cursor: null }) }) };
});

let docA: Document, docB: Document, blockA: DocumentBlock, blockB: DocumentBlock, viewId: string;
let marks: StoreApi<Marks> | null = null;
let version = 500, serial = 100;
const originalSession = client.session.getState();
function comment(discussion: string, text: string, id = `comment-${++serial}`, seq = ++serial): DocumentComment {
  return { id, discussion_id: discussion, author: docA.owner, content: [{ type: 'text', text }], reactions: [],
    edited_at: null, deleted: false, created_at: '2026-10-17T09:00:00Z', seq_updated: seq };
}
function discussion(document: Document, block: DocumentBlock, id: string, text: string): DocumentDiscussion {
  const first = comment(id, text);
  return { id, document_id: document.id, block_id: block.id, quote: `Words for ${id}`, anchored: true, resolved: null,
    created_by: document.owner, created_at: '2026-10-17T09:00:00Z', comments: [first], seq_updated: first.seq_updated };
}
function CaptureMarks() { const store = useDocumentMarksStore(); useEffect(() => { marks = store; }, [store]); return null; }
function CaptureOtherMarks({ capture }: { capture: (store: StoreApi<Marks>) => void }) {
  const store = useDocumentMarksStore(); useEffect(() => capture(store), [store, capture]); return null;
}
function Pane({ id, owner = 'main-document' }: { id: string; owner?: string }) {
  const state = useDocumentState(id);
  const live = useWorkbench((workbench) => findTab(workbench.layout, viewId) !== null);
  if (!live || state.document === null) return null;
  return <ViewInstanceContext.Provider value={{ id: viewId, root: createRef(), target: { kind: 'document', id }, headingOwner: owner, content: false, focusedContent: false }}>
    <DocumentMarksProvider owner={owner} documentId={id}><CaptureMarks /><CommentsPanel key={id} doc={state.document} /></DocumentMarksProvider>
  </ViewInstanceContext.Provider>;
}
function Frame({ id, owner }: { id: string; owner?: string }) { useUnsavedGuard(); return <><Pane id={id} owner={owner} /><CloseViews /></>; }
function card(id: string): HTMLElement {
  const found = document.querySelector(`[data-discussion-card="${id}"]`);
  if (!(found instanceof HTMLElement)) throw new Error('the actual discussion card is missing');
  return found;
}
function commentRow(id: string): HTMLElement {
  const found = document.querySelector(`[data-comment="${id}"]`);
  if (!(found instanceof HTMLElement)) throw new Error('the actual comment row is missing');
  return found;
}
function setTarget(block: DocumentBlock, quote: string) {
  if (marks === null) throw new Error('the actual marks provider has not mounted');
  const current = marks;
  act(() => current.setState({ compose: { block: block.id, quote } }));
}
function nativeCount() { expect(calls.start).not.toHaveBeenCalled(); expect(calls.reply).not.toHaveBeenCalled(); expect(calls.edit).not.toHaveBeenCalled(); }
beforeEach(async () => {
  await setLocale('en'); cleanup(); vi.clearAllMocks(); retireCommentDraftSession(); retireDocumentMarks(); localStorage.clear(); documentWriter.clear();
  const world = new World(), person = world.people.get('ada');
  const stored = [...world.documents.docs.values()].find((entry) => entry.owner === 'ada' && !entry.deleted);
  if (person === undefined || stored === undefined) throw new Error('the ordinary readable document fixture is missing');
  const native = world.documents.sync(person, stored, null);
  const paragraph = native.blocks.find((entry) => entry.type === 'paragraph');
  if (paragraph === undefined) throw new Error('the native-shaped document needs a paragraph');
  docA = world.documents.full(person, stored);
  docB = { ...docA, id: 'comment-document-b', title: 'Second document' };
  serial = Math.max(docA.seq, docB.seq) + 100;
  blockA = { ...paragraph, document_id: docA.id, id: 'comment-block-a', parent_id: null, position: '1', content: [{ type: 'text', text: 'Chosen passage' }] };
  blockB = { ...blockA, document_id: docB.id, id: 'comment-block-b' };
  documentWriter.reset(docA.id, { document: docA, blocks: [blockA], discussions: [] }, docA.seq);
  documentWriter.reset(docB.id, { document: docB, blocks: [blockB], discussions: [] }, docB.seq);
  client.session.setState({ status: 'signed_in', userId: docA.owner.id, readerVersion: ++version });
  useWorkbench.getState().leave(); useWorkbench.getState().enter(docA.owner.id, 'document');
  viewId = useWorkbench.getState().openTab('comments', undefined, 'new');
  marks = null;
  calls.readers.mockResolvedValue({ items: [{ user: docA.owner, can_view: true }], next_cursor: null });
  calls.start.mockImplementation((id, body) => {
    const made = discussion(id === docA.id ? docA : docB, id === docA.id ? blockA : blockB, `made-${++serial}`, '');
    return Promise.resolve({ ...made, quote: body.quote ?? '', comments: made.comments.map((entry) => ({ ...entry, content: body.content })) });
  });
  calls.reply.mockImplementation((_id, discussionId, content) => Promise.resolve({ ...comment(discussionId, ''), content }));
  calls.edit.mockImplementation((_id, commentId, content) => Promise.resolve({ ...comment('discussion-one', '', commentId), content, edited_at: '2026-10-17T10:00:00Z' }));
});
afterEach(async () => {
  cleanup(); vi.restoreAllMocks(); forgetCommentDraftDeviceData(docA.owner.id, null); forgetCommentDraftDeviceData('different-reader', null);
  retireCommentDraftSession(); retireDocumentMarks(); useWorkbench.getState().leave(); documentWriter.clear(); localStorage.clear(); client.session.setState(originalSession); await setLocale('en');
});

it.each(['en', 'zh-CN'] as const)('keeps a new anchored draft/caret across close/reopen and a fresh-memory reload; local Save never publishes (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  let pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  const field = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  await user.type(field, 'Keep these words locally');
  if (!(field instanceof HTMLTextAreaElement)) throw new Error('the composer must be its actual textarea');
  field.setSelectionRange(3, 8, 'backward'); fireEvent.select(field);
  await act(async () => { expect(await commentDraftGuard(viewId).save()).toBe(true); });
  nativeCount();
  expect(useWorkbench.getState().unsaved.has(viewId)).toBe(false);
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: viewId }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  act(() => useWorkbench.getState().reopen());
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Keep these words locally');
  pane.unmount(); retireCommentDraftSession(); retireDocumentMarks();
  pane = show(<Frame id={docA.id} />);
  const restored = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  expect(restored).toHaveValue('Keep these words locally');
  if (!(restored instanceof HTMLTextAreaElement)) throw new Error('the restored field must be the actual composer');
  expect([restored.selectionStart, restored.selectionEnd, restored.selectionDirection]).toEqual([3, 8, 'backward']);
  nativeCount();
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.start).toHaveBeenCalledTimes(1);
  const sent = calls.start.mock.calls[0];
  expect(sent?.[0]).toBe(docA.id);
  expect(sent?.[1].client_id).toEqual(expect.any(String));
  expect(sent?.[1]).toEqual({ block_id: blockA.id, quote: 'Chosen passage', content: [{ type: 'text', text: 'Keep these words locally' }], client_id: sent?.[1].client_id });
  pane.unmount();
});

it('a completed Send with failed local erasure stays reachable and local Retry erases it without publishing again', async () => {
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  const field = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  await user.type(field, 'Published exactly once by explicit Send');
  const draft = getCommentDrafts(docA.owner.id, viewId, docA.id);
  const originalRemove = localStorage.removeItem.bind(localStorage);
  const erase = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(function (this: Storage, key: string) {
    if (key === draft.key) throw new DOMException('blocked', 'SecurityError');
    originalRemove(key);
  });
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  await waitFor(() => expect(documentStateNow(docA.id).discussions.size).toBe(1));
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Published exactly once by explicit Send');
  expect(useWorkbench.getState().unsaved.has(viewId)).toBe(true);
  const composer = screen.getByRole('region', { name: i18n.t('documents.comments.new') });
  expect(within(composer).getByRole('button', { name: i18n.t('documents.comments.send') })).toBeDisabled();
  fireEvent.keyDown(field, { key: 'Enter', ctrlKey: true });
  expect(calls.start).toHaveBeenCalledTimes(1);
  erase.mockRestore();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(screen.queryByRole('textbox', { name: i18n.t('documents.comments.new') })).not.toBeInTheDocument();
  expect(localStorage.getItem(draft.key)).toBeNull();
  expect(useWorkbench.getState().unsaved.has(viewId)).toBe(false);
  expect(documentStateNow(docA.id).discussions.size).toBe(1);
  expect(calls.start).toHaveBeenCalledTimes(1);
  expect(calls.reply).not.toHaveBeenCalled(); expect(calls.edit).not.toHaveBeenCalled();
});

it('retargets between documents without sending or clearing the other source and keeps same-target fix/follow text', async () => {
  const user = userEvent.setup();
  const pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'First document draft');
  pane.rerender(<Frame id={docA.id} owner={viewId} />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('First document draft');
  pane.rerender(<Frame id={docA.id} owner="main-document" />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('First document draft');
  pane.rerender(<Frame id={docB.id} />);
  expect(screen.queryByDisplayValue('First document draft')).not.toBeInTheDocument();
  setTarget(blockB, 'Other passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Second document draft');
  nativeCount();
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.start.mock.calls[0]?.[0]).toBe(docB.id);
  expect(calls.start.mock.calls[0]?.[1].content).toEqual([{ type: 'text', text: 'Second document draft' }]);
  pane.rerender(<Frame id={docA.id} />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('First document draft');
});

it('a same-document owner change keeps its draft/caret without overwriting another main pane’s compose selection', async () => {
  const user = userEvent.setup(), pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  const field = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  await user.type(field, 'This panel owns these words');
  if (!(field instanceof HTMLTextAreaElement)) throw new Error('the actual compose textarea must be mounted');
  field.setSelectionRange(2, 6, 'backward'); fireEvent.select(field);
  const captured: { current?: StoreApi<Marks> } = {};
  const capture = (store: StoreApi<Marks>) => { captured.current = store; };
  const foreign = show(<DocumentMarksProvider owner="other-main" documentId={docA.id}><CaptureOtherMarks capture={capture} /></DocumentMarksProvider>);
  if (captured.current === undefined) throw new Error('the other real scoped marks store must mount');
  const otherStore = captured.current;
  const foreignCompose = { block: blockA.id, quote: 'Other selected words' };
  act(() => otherStore.setState({ compose: foreignCompose, focused: 'other-selection' }));
  pane.rerender(<Frame id={docA.id} owner="other-main" />);
  const current = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  expect(current).toHaveValue('This panel owns these words');
  if (!(current instanceof HTMLTextAreaElement)) throw new Error('the same actual field must remain');
  expect([current.selectionStart, current.selectionEnd, current.selectionDirection]).toEqual([2, 6, 'backward']);
  expect(otherStore.getState().compose).toBe(foreignCompose);
  expect(otherStore.getState().focused).toBe('other-selection');
  act(() => otherStore.setState({ focused: 'different-discussion-selection' }));
  expect(screen.getAllByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveLength(1);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('This panel owns these words');
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.start.mock.calls[0]?.[1].quote).toBe('Chosen passage');
  expect(otherStore.getState().compose).toBe(foreignCompose);
  foreign.unmount();
});

it('restores multiple replies and an edit, including line breaks, and aggregates them under one failed local-save guard', async () => {
  const one = discussion(docA, blockA, 'discussion-one', 'Original\ncomment'), two = discussion(docA, blockA, 'discussion-two', 'Other comment');
  one.comments = one.comments.map((entry) => ({ ...entry, content: [{ type: 'text', text: 'Original' }, { type: 'break' }, { type: 'text', text: 'comment' }] }));
  act(() => documentWriter.apply(docA.id, { discussions: [one, two] }, Math.max(one.seq_updated, two.seq_updated)));
  const user = userEvent.setup(), pane = show(<Frame id={docA.id} />);
  await user.type(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') }), 'First reply');
  await user.type(within(card(two.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') }), 'Second reply');
  await user.click(within(card(one.id)).getByRole('button', { name: i18n.t('documents.comments.edit') }));
  const edited = within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') });
  expect(edited).toHaveValue('Original\ncomment');
  await user.clear(edited); await user.type(edited, 'Unsent\nedit');
  pane.unmount(); retireCommentDraftSession(); retireDocumentMarks();
  show(<Frame id={docA.id} />);
  expect(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') })).toHaveValue('Unsent\nedit');
  expect(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') })).toHaveValue('First reply');
  expect(within(card(two.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') })).toHaveValue('Second reply');
  const fail = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  await user.type(within(card(two.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') }), '!');
  expect(useWorkbench.getState().unsaved.has(viewId)).toBe(true);
  const leaving = new Event('beforeunload', { cancelable: true }); window.dispatchEvent(leaving); expect(leaving.defaultPrevented).toBe(true);
  act(() => useWorkbench.getState().close({ kind: 'tab', tab: viewId }));
  const decision = screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveClose') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  expect(findTab(useWorkbench.getState().layout, viewId)).not.toBeNull();
  expect(screen.getByDisplayValue('First reply')).toBeInTheDocument();
  expect(screen.getByDisplayValue('Second reply!')).toBeInTheDocument();
  expect(screen.getByDisplayValue('Unsent\nedit', { normalizer: (value) => value })).toBeInTheDocument();
  nativeCount();
  fail.mockRestore();
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  await waitFor(() => expect(findTab(useWorkbench.getState().layout, viewId)).toBeNull());
  nativeCount();
});

it('reuses the exact unknown-outcome client ID and payload after the actual composer is closed and reopened', async () => {
  const user = userEvent.setup(); let pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  const input = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  await user.type(input, `Ask @${docA.owner.username}`);
  await user.click(await screen.findByRole('option', { name: `${docA.owner.display_name} @${docA.owner.username}` }));
  if (!(input instanceof HTMLTextAreaElement)) throw new Error('the mention belongs to the actual comment composer');
  const words = input.value;
  calls.start.mockRejectedValueOnce(new NetworkError('failed'));
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('documents.comments.send') })).not.toHaveAttribute('aria-busy', 'true'));
  const first = calls.start.mock.calls[0];
  if (first === undefined) throw new Error('explicit Send must have reached its native mutation');
  expect(first[1].content).toEqual([{ type: 'text', text: 'Ask ' }, { type: 'mention', user_id: docA.owner.id }]);
  pane.unmount(); retireCommentDraftSession(); retireDocumentMarks(); pane = show(<Frame id={docA.id} />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue(words);
  expect(calls.start).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.start.mock.calls[1]).toEqual(first);
  pane.unmount();
});

it('reopens a failed reply under its original discussion and repeats only its original request identity', async () => {
  const one = discussion(docA, blockA, 'reply-history', 'Original comment');
  act(() => documentWriter.apply(docA.id, { discussions: [one] }, one.seq_updated));
  const user = userEvent.setup(); let pane = show(<Frame id={docA.id} />);
  await user.type(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') }), 'An exact reply');
  calls.reply.mockRejectedValueOnce(new NetworkError('failed'));
  await user.click(within(card(one.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  await waitFor(() => expect(within(card(one.id)).getByRole('button', { name: i18n.t('documents.comments.send') })).not.toHaveAttribute('aria-busy', 'true'));
  const first = calls.reply.mock.calls[0];
  if (first === undefined) throw new Error('the reply must be an actual explicit mutation');
  pane.unmount(); retireCommentDraftSession(); retireDocumentMarks(); pane = show(<Frame id={docA.id} />);
  expect(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') })).toHaveValue('An exact reply');
  expect(calls.reply).toHaveBeenCalledTimes(1);
  await user.click(within(card(one.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.reply.mock.calls[1]).toEqual(first);
  expect(calls.start).not.toHaveBeenCalled();
  pane.unmount();
});

it('a delayed send cannot clear a retargeted document, and a retired reader cannot publish its late result into stores', async () => {
  const user = userEvent.setup(); const pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Old source');
  const originalMarks = marks;
  if (originalMarks === null) throw new Error('the original reader must have its actual marks store');
  const held: { finish?: (result: DocumentDiscussion) => void } = {};
  calls.start.mockImplementationOnce(() => new Promise((resolve) => { held.finish = resolve; }));
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  pane.rerender(<Frame id={docB.id} />);
  setTarget(blockB, 'Other passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Current source');
  const before = documentStateNow(docA.id).discussions;
  act(() => client.session.setState({ userId: 'different-reader', readerVersion: ++version }));
  expect(originalMarks.getState().compose).toBeNull();
  expect(originalMarks.getState().focused).toBeNull();
  if (held.finish === undefined) throw new Error('the actual first Send must issue its held request');
  const finish = held.finish;
  await act(async () => { finish(discussion(docA, blockA, 'late-old-result', 'Old source')); await Promise.resolve(); });
  expect(documentStateNow(docA.id).discussions).toBe(before);
  expect(screen.queryByDisplayValue('Old source')).not.toBeInTheDocument();
  expect(screen.queryByDisplayValue('Current source')).not.toBeInTheDocument();
  expect(calls.start).toHaveBeenCalledTimes(1);
});

it('a same-reader late success retires only its original source draft while a new document remains untouched', async () => {
  const user = userEvent.setup(), pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Old source');
  const held: { finish?: (result: DocumentDiscussion) => void } = {};
  calls.start.mockImplementationOnce(() => new Promise((resolve) => { held.finish = resolve; }));
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  pane.rerender(<Frame id={docB.id} />);
  setTarget(blockB, 'Other passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Keep current words');
  if (held.finish === undefined) throw new Error('the old source must have issued an actual held request');
  const finish = held.finish;
  await act(async () => { finish(discussion(docA, blockA, 'committed-old-source', 'Old source')); await Promise.resolve(); });
  await waitFor(() => expect(documentStateNow(docA.id).discussions.has('committed-old-source')).toBe(true));
  expect(documentStateNow(docB.id).discussions.size).toBe(0);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Keep current words');
  expect(calls.start).toHaveBeenCalledTimes(1);
  pane.rerender(<Frame id={docA.id} />);
  expect(screen.queryByDisplayValue('Old source')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox', { name: i18n.t('documents.comments.new') })).not.toBeInTheDocument();
});

it('current permission disables restored sends, and explicit device deletion cannot resurrect body/attempt on cleanup', async () => {
  const user = userEvent.setup(); const pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Local only');
  const current = documentStateNow(docA.id);
  act(() => documentWriter.reset(docA.id, { document: { ...docA, access: { ...docA.access, permissions: ['doc.view'] } }, blocks: [...current.blocks.values()], discussions: [] }, current.lastSeq));
  const field = screen.getByRole('textbox', { name: i18n.t('documents.comments.new') });
  expect(field).toHaveAttribute('readonly');
  expect(screen.getByRole('button', { name: i18n.t('documents.comments.send') })).toBeDisabled();
  fireEvent.keyDown(field, { key: 'Enter', ctrlKey: true }); nativeCount();
  const draft = getCommentDrafts(docA.owner.id, viewId, docA.id);
  localStorage.removeItem(draft.key); act(() => forgetCommentDraftDeviceData(docA.owner.id, [draft.key]));
  pane.unmount();
  expect(localStorage.getItem(draft.key)).toBeNull();
  expect(draft.store.getState().data.slots).toEqual({});
});

it('a late refused reply after device erasure cannot restore its body, attempt or old error', async () => {
  const one = discussion(docA, blockA, 'forgotten-reply', 'Original comment');
  act(() => documentWriter.apply(docA.id, { discussions: [one] }, one.seq_updated));
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  const field = within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') });
  await user.type(field, 'Forget this pending reply');
  const held: { reject?: (reason: unknown) => void } = {};
  const outcome = new Promise<DocumentComment>((_resolve, reject) => { held.reject = reject; });
  calls.reply.mockReturnValueOnce(outcome);
  await user.click(within(card(one.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  const draft = getCommentDrafts(docA.owner.id, viewId, docA.id);
  localStorage.removeItem(draft.key); act(() => forgetCommentDraftDeviceData(docA.owner.id, [draft.key]));
  if (held.reject === undefined) throw new Error('the explicit reply must issue its held request');
  const reject = held.reject;
  await act(async () => { reject(new NetworkError('failed')); await outcome.catch(() => undefined); });
  expect(within(card(one.id)).getByRole('textbox', { name: i18n.t('documents.comments.answer') })).toHaveValue('');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(localStorage.getItem(draft.key)).toBeNull();
  expect(draft.store.getState().data.slots).toEqual({});
  expect(documentStateNow(docA.id).discussions.get(one.id)?.comments).toEqual(one.comments);
  expect(calls.reply).toHaveBeenCalledTimes(1); expect(calls.start).not.toHaveBeenCalled(); expect(calls.edit).not.toHaveBeenCalled();
});

it('a wrong-source mutation answer cannot consume the local draft or contaminate another document', async () => {
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Keep the actual source');
  calls.start.mockResolvedValueOnce(discussion(docB, blockB, 'wrong-document-result', 'Wrong source'));
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  await waitFor(() => expect(screen.getByRole('button', { name: i18n.t('documents.comments.send') })).not.toHaveAttribute('aria-busy', 'true'));
  expect(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') })).toHaveValue('Keep the actual source');
  expect(documentStateNow(docA.id).discussions.size).toBe(0);
  expect(documentStateNow(docB.id).discussions.size).toBe(0);
  const first = calls.start.mock.calls[0];
  if (first === undefined) throw new Error('the first explicit request must actually issue');
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.start.mock.calls[1]).toEqual(first);
});

it('one captured retarget decision keeps a failed-storage draft reachable on Cancel or failed Save', async () => {
  const user = userEvent.setup(), pane = show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  const fail = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'The only remaining draft');
  const retarget = vi.fn(() => { pane.rerender(<Frame id={docB.id} />); });
  act(() => requestViewAction([viewId], retarget));
  let decision = screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveContinue') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('common.cancel') }));
  expect(retarget).not.toHaveBeenCalled();
  expect(screen.getByDisplayValue('The only remaining draft')).toBeInTheDocument();
  act(() => requestViewAction([viewId], retarget));
  decision = screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveContinue') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  expect(retarget).not.toHaveBeenCalled(); expect(findTab(useWorkbench.getState().layout, viewId)).not.toBeNull();
  expect(screen.getByDisplayValue('The only remaining draft')).toBeInTheDocument(); nativeCount();
  fail.mockRestore();
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  await waitFor(() => expect(retarget).toHaveBeenCalledTimes(1));
  expect(findTab(useWorkbench.getState().layout, viewId)).not.toBeNull();
  expect(screen.queryByDisplayValue('The only remaining draft')).not.toBeInTheDocument();
  pane.rerender(<Frame id={docA.id} />);
  expect(screen.getByDisplayValue('The only remaining draft')).toBeInTheDocument(); nativeCount();
});

it('ordinary retained drafts retarget without a dialog and a retired reader cannot confirm an old action', async () => {
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  setTarget(blockA, 'Chosen passage');
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Safely retained');
  const immediate = vi.fn(); act(() => requestViewAction([viewId], immediate));
  expect(immediate).toHaveBeenCalledTimes(1); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), ' but now storage failed');
  const retired = vi.fn(); act(() => requestViewAction([viewId], retired));
  expect(screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveContinue') })).toBeInTheDocument();
  act(() => client.session.setState({ userId: 'different-reader', readerVersion: ++version }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(retired).not.toHaveBeenCalled();
  expect(screen.queryByDisplayValue('Safely retained but now storage failed')).not.toBeInTheDocument(); nativeCount();
});

it('the real panel-scope action guards only a changed target and keeps both the view identity and failed draft', async () => {
  function ScopedFrame() {
    const scope = useWorkbench((state) => findTab(state.layout, viewId)?.tab.scope);
    const id = scope?.mode === 'fixed' && scope.target.kind === 'document' ? scope.target.id : docA.id;
    return <Frame id={id} />;
  }
  setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: docA.id } });
  const user = userEvent.setup(); show(<ScopedFrame />); setTarget(blockA, 'Chosen passage');
  const fail = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new DOMException('quota', 'QuotaExceededError'); });
  await user.type(screen.getByRole('textbox', { name: i18n.t('documents.comments.new') }), 'Retarget must keep this');
  act(() => setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: docA.id } }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  act(() => setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: docB.id } }));
  let decision = screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveContinue') });
  expect(findTab(useWorkbench.getState().layout, viewId)?.tab.scope).toEqual({ mode: 'fixed', target: { kind: 'document', id: docA.id } });
  await user.click(within(decision).getByRole('button', { name: i18n.t('common.cancel') }));
  expect(screen.getByDisplayValue('Retarget must keep this')).toBeInTheDocument();
  act(() => setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: docB.id } }));
  decision = screen.getByRole('dialog', { name: i18n.t('workbench.workspace.saveContinue') });
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  expect(findTab(useWorkbench.getState().layout, viewId)?.tab.scope).toEqual({ mode: 'fixed', target: { kind: 'document', id: docA.id } });
  fail.mockRestore();
  await user.click(within(decision).getByRole('button', { name: i18n.t('workbench.workspace.saveAll') }));
  await waitFor(() => expect(findTab(useWorkbench.getState().layout, viewId)?.tab.scope).toEqual({ mode: 'fixed', target: { kind: 'document', id: docB.id } }));
  act(() => setPanelScope(viewId, { mode: 'fixed', target: { kind: 'document', id: docA.id } }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByDisplayValue('Retarget must keep this')).toBeInTheDocument(); nativeCount();
});

it('an unknown-outcome edit reopens with the original payload/key/base even after a newer row and cannot replace that newer row', async () => {
  const one = discussion(docA, blockA, 'discussion-one', 'Original words');
  act(() => documentWriter.apply(docA.id, { discussions: [one] }, one.seq_updated));
  const original = one.comments[0];
  if (original === undefined) throw new Error('the actual authored comment is required');
  const user = userEvent.setup(); let pane = show(<Frame id={docA.id} />);
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.edit') }));
  const field = within(commentRow(original.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') });
  await user.clear(field); await user.type(field, 'Unknown saved edit');
  calls.edit.mockRejectedValueOnce(new NetworkError('failed'));
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  await waitFor(() => expect(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') })).not.toHaveAttribute('aria-busy', 'true'));
  const sent = calls.edit.mock.calls[0];
  if (sent?.[3] === undefined) throw new Error('the shipped edit must use a modern paired attempt');
  expect(sent[3].clientId).toEqual(expect.any(String));
  expect(sent[3].expectedSeq).toBe(original.seq_updated);
  const saved = { ...original, content: [{ type: 'text' as const, text: 'Unknown saved edit' }], edited_at: '2026-10-17T10:01:00Z', seq_updated: ++serial };
  const newer = { ...original, content: [{ type: 'text' as const, text: 'A later native edit' }], edited_at: '2026-10-17T10:02:00Z', seq_updated: ++serial };
  act(() => documentWriter.apply(docA.id, { discussions: [{ ...one, comments: [newer], seq_updated: newer.seq_updated }] }, newer.seq_updated));
  pane.unmount(); retireCommentDraftSession(); retireDocumentMarks(); pane = show(<Frame id={docA.id} />);
  expect(within(commentRow(original.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') })).toHaveValue('Unknown saved edit');
  await act(async () => { expect(await commentDraftGuard(viewId).save()).toBe(true); });
  expect(calls.edit).toHaveBeenCalledTimes(1);
  calls.edit.mockResolvedValueOnce(saved);
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.edit.mock.calls[1]).toEqual(sent);
  await waitFor(() => expect(within(commentRow(original.id)).queryByRole('textbox', { name: i18n.t('documents.comments.edit') })).not.toBeInTheDocument());
  expect(documentStateNow(docA.id).discussions.get(one.id)?.comments).toEqual([newer]);
  pane.unmount();
});

it.each(['en', 'zh-CN'] as const)('a stale edit requires local review of exactly the displayed revision and another explicit Send (%s)', async (locale) => {
  await setLocale(locale);
  const one = discussion(docA, blockA, 'discussion-one', 'Original words');
  act(() => documentWriter.apply(docA.id, { discussions: [one] }, one.seq_updated));
  const original = one.comments[0];
  if (original === undefined) throw new Error('the actual authored comment is required');
  const current = { ...original, content: [{ type: 'text' as const, text: 'Current reviewed words' }], edited_at: '2026-10-17T10:01:00Z', seq_updated: ++serial };
  const newer = { ...original, content: [{ type: 'text' as const, text: 'Changed behind the review' }], edited_at: '2026-10-17T10:02:00Z', seq_updated: ++serial };
  const result = { ...original, content: [{ type: 'text' as const, text: 'Preserved local words' }], edited_at: '2026-10-17T10:03:00Z', seq_updated: ++serial };
  const stale = (row: DocumentComment) => new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision',
    status: 409, title: '', detail: 'The comment changed.', request_id: '', current_comment: row }, null);
  calls.edit.mockRejectedValueOnce(stale(current)).mockRejectedValueOnce(stale(newer)).mockResolvedValueOnce(result);
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.edit') }));
  const field = within(commentRow(original.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') });
  await user.clear(field); await user.type(field, 'Preserved local words');
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  await screen.findByRole('button', { name: i18n.t('documents.comments.reviewEdit') });
  const draft = getCommentDrafts(docA.owner.id, viewId, docA.id), target = { kind: 'edit' as const, discussion: one.id, comment: original.id };
  expect(draft.store.getState().data.slots[commentSlotKey(target)]?.baseSeq).toBe(original.seq_updated);
  expect(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') })).toBeDisabled();
  await act(async () => { expect(await commentDraftGuard(viewId).save()).toBe(true); }); expect(calls.edit).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.reviewEdit') }));
  let review = screen.getByRole('dialog', { name: i18n.t('documents.comments.reviewEdit') });
  expect(within(review).getByText('Current reviewed words')).toBeInTheDocument();
  expect(within(review).getByText('Preserved local words')).toBeInTheDocument();
  act(() => documentWriter.apply(docA.id, { discussions: [{ ...one, comments: [newer], seq_updated: newer.seq_updated }] }, newer.seq_updated));
  expect(within(review).queryByText('Changed behind the review')).not.toBeInTheDocument();
  await user.click(within(review).getByRole('button', { name: i18n.t('documents.comments.keepDraftOnVersion') }));
  expect(draft.store.getState().data.slots[commentSlotKey(target)]?.baseSeq).toBe(current.seq_updated);
  expect(calls.edit).toHaveBeenCalledTimes(1);
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  await screen.findByRole('button', { name: i18n.t('documents.comments.reviewEdit') });
  expect(calls.edit.mock.calls[1]?.[3]?.expectedSeq).toBe(current.seq_updated);
  expect(calls.edit.mock.calls[1]?.[3]?.clientId).not.toBe(calls.edit.mock.calls[0]?.[3]?.clientId);
  expect(draft.store.getState().data.slots[commentSlotKey(target)]?.text).toBe('Preserved local words');
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.reviewEdit') }));
  review = screen.getByRole('dialog', { name: i18n.t('documents.comments.reviewEdit') });
  expect(within(review).getByText('Changed behind the review')).toBeInTheDocument();
  await user.click(within(review).getByRole('button', { name: i18n.t('documents.comments.keepDraftOnVersion') }));
  expect(calls.edit).toHaveBeenCalledTimes(2);
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.edit.mock.calls[2]?.[3]?.expectedSeq).toBe(newer.seq_updated);
  await waitFor(() => expect(within(commentRow(original.id)).queryByRole('textbox', { name: i18n.t('documents.comments.edit') })).not.toBeInTheDocument());
  expect(documentStateNow(docA.id).discussions.get(one.id)?.comments).toEqual([result]);
  expect(calls.start).not.toHaveBeenCalled(); expect(calls.reply).not.toHaveBeenCalled();
});

it('a recovered edit with no authored revision cannot send until explicit local review provides the modern pair', async () => {
  const one = discussion(docA, blockA, 'discussion-one', 'Current native words');
  act(() => documentWriter.apply(docA.id, { discussions: [one] }, one.seq_updated));
  const original = one.comments[0];
  if (original === undefined) throw new Error('the actual authored comment is required');
  const draft = getCommentDrafts(docA.owner.id, viewId, docA.id);
  setCommentText(draft, { kind: 'edit', discussion: one.id, comment: original.id }, 'Recovered local words');
  const user = userEvent.setup(); show(<Frame id={docA.id} />);
  expect(within(commentRow(original.id)).getByRole('textbox', { name: i18n.t('documents.comments.edit') })).toHaveValue('Recovered local words');
  expect(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') })).toBeDisabled();
  nativeCount();
  await user.click(screen.getByRole('button', { name: i18n.t('documents.comments.reviewEdit') }));
  const review = screen.getByRole('dialog', { name: i18n.t('documents.comments.reviewEdit') });
  expect(within(review).getByText('Current native words')).toBeInTheDocument();
  await user.click(within(review).getByRole('button', { name: i18n.t('documents.comments.keepDraftOnVersion') }));
  nativeCount();
  await user.click(within(commentRow(original.id)).getByRole('button', { name: i18n.t('documents.comments.send') }));
  expect(calls.edit.mock.calls[0]?.[3]?.clientId).toEqual(expect.any(String));
  expect(calls.edit.mock.calls[0]?.[3]?.expectedSeq).toBe(original.seq_updated);
  expect(calls.edit.mock.calls[0]?.[2]).toEqual([{ type: 'text', text: 'Recovered local words' }]);
});
